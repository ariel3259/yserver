use crate::kms::render::{
    cursor_save::CursorSaveTarget,
    scene::{
        CursorAcceptedMode, CursorAssignment, CursorCommitReceipt, CursorFrameAssignment,
        CursorPrimaryContent, OutputCursorMode, OutputLayout, OutputSceneState, SceneCompositor,
        SceneCompositorInner,
    },
};
use ash::vk;

/// Pure classifier for the per-output accepted cursor modes.
pub(super) fn classify_cursor_mode_from_per_output(
    modes: impl IntoIterator<Item = OutputCursorMode>,
) -> crate::kms::render::scene::CursorPlaneMode {
    use crate::kms::render::scene::CursorPlaneMode;

    let mut any_hw = false;
    let mut any_sw = false;
    let mut any_output = false;
    for mode in modes {
        any_output = true;
        match mode {
            OutputCursorMode::Hw => any_hw = true,
            OutputCursorMode::Sw { .. } => any_sw = true,
            OutputCursorMode::Hidden => {}
        }
    }
    match (any_hw, any_sw, any_output) {
        (true, false, _) => CursorPlaneMode::Hw,
        (false, true, _) | (false, false, false) => CursorPlaneMode::Sw,
        (false, false, true) => CursorPlaneMode::Hidden,
        (true, true, _) => CursorPlaneMode::Mixed,
    }
}

pub(super) fn accepted_mode_from_output_mode(mode: OutputCursorMode) -> CursorAcceptedMode {
    match mode {
        OutputCursorMode::Hw => CursorAcceptedMode::Hardware,
        OutputCursorMode::Sw { .. } => CursorAcceptedMode::Software,
        OutputCursorMode::Hidden => CursorAcceptedMode::Hidden,
    }
}

fn accepted_mode_from_frame_assignment(assignment: CursorFrameAssignment) -> CursorAcceptedMode {
    match assignment {
        CursorFrameAssignment::Hardware => CursorAcceptedMode::Hardware,
        CursorFrameAssignment::Software { .. } => CursorAcceptedMode::Software,
        CursorFrameAssignment::Hidden => CursorAcceptedMode::Hidden,
        CursorFrameAssignment::CursorlessReplacement => CursorAcceptedMode::Hardware,
    }
}

/// A pending scene frame still owns a C1 probation boundary when its accepted
/// receipt (or, for a copied render not yet submitted, its captured assignment)
/// changes the accepted cursor mode.
pub(super) fn pending_ack_changes_cursor_mode(ack: &super::PendingAck) -> bool {
    pending_ack_mode_after(ack) != ack.cursor_mode_before
}

fn pending_ack_mode_after(ack: &super::PendingAck) -> CursorAcceptedMode {
    ack.cursor_receipt
        .filter(|receipt| receipt.frame == Some(ack.cursor_frame.frame))
        .map_or_else(
            || accepted_mode_from_frame_assignment(ack.cursor_frame.assignment),
            |receipt| receipt.mode,
        )
}

pub(super) fn assignment_changes_cursor_mode(
    previous: OutputCursorMode,
    assignment: CursorAssignment,
) -> bool {
    let next = match assignment {
        CursorAssignment::Hw { .. } => CursorAcceptedMode::Hardware,
        CursorAssignment::Sw { .. } => CursorAcceptedMode::Software,
        CursorAssignment::Hidden => CursorAcceptedMode::Hidden,
        CursorAssignment::CursorlessReplacement => CursorAcceptedMode::Hardware,
    };
    accepted_mode_from_output_mode(previous) != next
}

/// Reconcile scene bookkeeping with the output's actual accepted binding.
/// A failed atomic detach keeps HW authoritative; a failed rollback that
/// already detached it makes a later HW assignment a fresh attach.
pub(super) fn effective_cursor_prev_mode(
    scene_prev: OutputCursorMode,
    platform_visible: bool,
    assignment: CursorAssignment,
) -> OutputCursorMode {
    if !platform_visible && matches!(scene_prev, OutputCursorMode::Hw) {
        OutputCursorMode::Hidden
    } else if platform_visible
        && !matches!(scene_prev, OutputCursorMode::Hw)
        && matches!(
            assignment,
            CursorAssignment::Sw { .. }
                | CursorAssignment::Hidden
                | CursorAssignment::CursorlessReplacement
        )
    {
        OutputCursorMode::Hw
    } else {
        scene_prev
    }
}

pub(super) fn force_cursor_retry_repaint(state: &mut OutputSceneState) {
    state.scene_structure_damage.add(vk::Rect2D {
        offset: vk::Offset2D::default(),
        extent: state.output_extent,
    });
}

pub(super) fn cursor_damage_for_frame(
    last_present_cursor_rect: Option<vk::Rect2D>,
    last_present_cursor_version: Option<u64>,
    new_cursor_rect: Option<vk::Rect2D>,
    new_cursor_version: Option<u64>,
    cursor_mode_changed: bool,
) -> crate::kms::render::store::RegionSet {
    let mut damage = crate::kms::render::store::RegionSet::new();
    let cursor_changed = new_cursor_rect != last_present_cursor_rect
        || cursor_mode_changed
        || new_cursor_version != last_present_cursor_version;
    if !cursor_changed {
        return damage;
    }
    if let Some(rect) = last_present_cursor_rect {
        damage.add(rect);
    }
    if let Some(rect) = new_cursor_rect
        && Some(rect) != last_present_cursor_rect
    {
        damage.add(rect);
    }
    damage
}

/// Xorg refuses the hardware cursor while any CRTC is transformed, so the
/// sprite scales with content and never vanishes on an identity output.
pub(super) fn hw_cursor_allowed(outputs: &[OutputLayout]) -> bool {
    !outputs.iter().any(|output| output.transform.is_some())
}

pub(super) fn cursor_footprint_rect(
    dx: i32,
    dy: i32,
    cursor_w: i32,
    cursor_h: i32,
    layout_w: i32,
    layout_h: i32,
) -> Option<vk::Rect2D> {
    let x0 = dx.max(0);
    let y0 = dy.max(0);
    let x1 = (dx + cursor_w).min(layout_w);
    let y1 = (dy + cursor_h).min(layout_h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(vk::Rect2D {
        offset: vk::Offset2D { x: x0, y: y0 },
        extent: vk::Extent2D {
            width: u32::try_from(x1 - x0).unwrap_or(0),
            height: u32::try_from(y1 - y0).unwrap_or(0),
        },
    })
}

/// Copy the rect under the software cursor into the root-read save buffer,
/// leaving the compose image in `COLOR_ATTACHMENT_OPTIMAL` for cursor draw.
///
/// # Safety
///
/// `cb` is recording, outside a rendering scope.
pub(super) unsafe fn record_cursor_save(
    vk: &crate::kms::vk::device::VkContext,
    cb: vk::CommandBuffer,
    image: vk::Image,
    save: CursorSaveTarget,
) {
    let device = &vk.device;
    let color = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1);
    let to_src = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
        .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::COPY)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)
        .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
        .image(image)
        .subresource_range(color)];
    let buffer_waw = [vk::BufferMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::COPY)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .buffer(save.buffer)
        .size(vk::WHOLE_SIZE)];
    let region = [vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_offset(vk::Offset3D {
            x: save.rect.offset.x,
            y: save.rect.offset.y,
            z: 0,
        })
        .image_extent(vk::Extent3D {
            width: save.rect.extent.width,
            height: save.rect.extent.height,
            depth: 1,
        })];
    let to_color = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_READ)
        .dst_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE | vk::AccessFlags2::COLOR_ATTACHMENT_READ,
        )
        .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
        .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .image(image)
        .subresource_range(color)];
    let to_host = [vk::BufferMemoryBarrier2::default()
        .src_stage_mask(vk::PipelineStageFlags2::COPY)
        .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .dst_stage_mask(vk::PipelineStageFlags2::HOST)
        .dst_access_mask(vk::AccessFlags2::HOST_READ)
        .buffer(save.buffer)
        .size(vk::WHOLE_SIZE)];
    unsafe {
        crate::vk_count!(cmd_pipeline_barrier2);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default()
                .image_memory_barriers(&to_src)
                .buffer_memory_barriers(&buffer_waw),
        );
        crate::vk_count!(cmd_copy_image_to_buffer);
        device.cmd_copy_image_to_buffer(
            cb,
            image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            save.buffer,
            &region,
        );
        crate::vk_count!(cmd_pipeline_barrier2);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default()
                .image_memory_barriers(&to_color)
                .buffer_memory_barriers(&to_host),
        );
    }
}

pub(super) fn note_cursor_commit_accepted_inner(
    inner: &mut SceneCompositorInner,
    output_idx: usize,
    receipt: CursorCommitReceipt,
) {
    let Some(output) = inner.outputs.get_mut(output_idx) else {
        return;
    };
    output.last_frame_cursor_mode = output_cursor_mode_from_receipt(receipt);
    output.cursor_commit_receipt = Some(receipt);
}

pub(super) fn output_cursor_mode_from_receipt(receipt: CursorCommitReceipt) -> OutputCursorMode {
    match receipt.mode {
        CursorAcceptedMode::Hardware => OutputCursorMode::Hw,
        CursorAcceptedMode::Software => {
            let prev = match receipt.primary_cursor {
                CursorPrimaryContent::Software { pos, .. } => Some(pos),
                CursorPrimaryContent::Cursorless => None,
            };
            OutputCursorMode::Sw { prev }
        }
        CursorAcceptedMode::Hidden => OutputCursorMode::Hidden,
    }
}

impl SceneCompositor {
    /// Publish a cursor state only after its owning output commit was accepted.
    pub fn note_cursor_commit_accepted(&mut self, output_idx: usize, receipt: CursorCommitReceipt) {
        if receipt.needs_recompose {
            self.wake_for_cursor_damage();
        }
        if let Some(inner) = self.inner.as_mut() {
            note_cursor_commit_accepted_inner(inner, output_idx, receipt);
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn for_tests_cursor_commit_receipt(
        &self,
        output_idx: usize,
    ) -> Option<CursorCommitReceipt> {
        self.inner
            .as_ref()?
            .outputs
            .get(output_idx)?
            .cursor_commit_receipt
    }

    /// C1 waits for a mode-changing scene receipt to retire. Captured copied
    /// frames count before KMS acceptance; accepted frames use their receipt.
    pub fn cursor_transition_pending(&self) -> bool {
        self.inner.as_ref().is_some_and(|inner| {
            inner.outputs.iter().any(|output| {
                output
                    .pending_acks
                    .iter()
                    .any(pending_ack_changes_cursor_mode)
            })
        })
    }

    /// Whether a submitted or copied scene frame currently owns a transition
    /// to Hardware on this output. Cursor intents may queue behind that frame
    /// so an intervening hide or newer position is reconciled at its event.
    pub fn output_cursor_transition_targets_hardware(&self, output_idx: usize) -> bool {
        self.inner
            .as_ref()
            .and_then(|inner| inner.outputs.get(output_idx))
            .is_some_and(|output| {
                output.pending_acks.iter().any(|ack| {
                    pending_ack_changes_cursor_mode(ack)
                        && pending_ack_mode_after(ack) == CursorAcceptedMode::Hardware
                })
            })
    }

    /// Direct primary ownership can start only while each retained composed
    /// primary is cursorless and every scene acknowledgement has retired.
    pub fn direct_scanout_primary_ready(&self) -> bool {
        self.inner.as_ref().is_some_and(|inner| {
            !inner.outputs.is_empty()
                && inner.outputs.iter().all(|output| {
                    output.pending_acks.is_empty()
                        && output.cursor_commit_receipt.is_some_and(|receipt| {
                            !receipt.needs_recompose
                                && receipt.primary_cursor == CursorPrimaryContent::Cursorless
                        })
                })
        })
    }

    /// Mixed HW/Hidden outputs retain cursorless primaries; software outputs
    /// need scene composition.
    pub fn any_output_uses_software_cursor(&self) -> bool {
        self.inner.as_ref().is_some_and(|inner| {
            inner
                .outputs
                .iter()
                .any(|output| matches!(output.last_frame_cursor_mode, OutputCursorMode::Sw { .. }))
        })
    }

    /// Put pixels covered by a software cursor back into `bytes`.
    pub fn restore_under_cursor(&self, image: vk::Image, read: vk::Rect2D, bytes: &mut [u8]) {
        if let Some(inner) = self.inner.as_ref() {
            for output in &inner.outputs {
                output.cursor_saves.restore(image, read, bytes);
            }
        }
    }

    /// Register the software cursor sprite after its pixel data was uploaded.
    pub fn register_cursor(&mut self, entry: crate::kms::render::scene::CursorEntry) {
        if let Some(inner) = self.inner.as_mut() {
            inner.cursor = Some(entry);
            self.note_structure_change();
        }
    }

    /// Drop the cursor entry. The next scene frame clears software pixels and
    /// carries any required atomic plane detach.
    pub fn clear_cursor(&mut self) {
        if let Some(inner) = self.inner.as_mut()
            && inner.cursor.take().is_some()
        {
            self.note_structure_change();
        }
    }

    /// Return the aggregate mode used by the pointer fast path.
    pub fn cursor_mode(&self) -> crate::kms::render::scene::CursorPlaneMode {
        let Some(inner) = self.inner.as_ref() else {
            return crate::kms::render::scene::CursorPlaneMode::Sw;
        };
        if inner.outputs.iter().any(|output| {
            output
                .pending_acks
                .iter()
                .any(pending_ack_changes_cursor_mode)
        }) {
            return crate::kms::render::scene::CursorPlaneMode::Mixed;
        }
        classify_cursor_mode_from_per_output(
            inner
                .outputs
                .iter()
                .map(|output| output.last_frame_cursor_mode),
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn for_tests_cursor_footprint_was_presented(&self, output_idx: usize) -> bool {
        self.inner
            .as_ref()
            .and_then(|inner| inner.outputs.get(output_idx))
            .is_some_and(|output| output.last_present_cursor_rect.is_some())
    }

    /// Set accepted hardware mode for direct Present test fixtures.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn for_tests_set_hw_cursor_mode(&mut self) {
        if let Some(inner) = self.inner.as_mut() {
            for output in &mut inner.outputs {
                output.last_frame_cursor_mode = OutputCursorMode::Hw;
                output.cursor_commit_receipt = Some(CursorCommitReceipt {
                    frame: None,
                    mode: CursorAcceptedMode::Hardware,
                    primary_cursor: CursorPrimaryContent::Cursorless,
                    needs_recompose: false,
                });
            }
        }
    }
}
