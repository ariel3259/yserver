use crate::kms::render::{
    scene::{
        CopiedSubmitOutcome, FailedSubmitBo, FrameSubmitMode, InFlightStage, OutputCursorMode,
        OutputSceneState, OutputSink, RenderCompletion, RenderNotification, SceneCompositor,
        SceneCompositorInner,
        cursor::{
            apply_cursor_transition_on_retire, force_cursor_retry_repaint,
            mark_cursor_transition_structure_dirty, resolve_retired_cursor_state,
            update_force_show_retry_version,
        },
        tick::tick_skip_log_enabled,
    },
    store::DrawableStore,
};
use std::collections::VecDeque;

/// Exact scene-side identity of a deferred unflip output frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeferredFrameIdentity {
    pub output_idx: usize,
    pub bo_idx: usize,
    pub render_job_id: u64,
    pub recording_complete: bool,
    pub topology_generation: u64,
}

impl InFlightStage {
    fn matches_render_completion(self, render_job_id: u64) -> bool {
        self == Self::WaitingForRenderCompletion { render_job_id }
    }

    fn matches_kms_event(self, event_id: Option<u64>) -> bool {
        matches!(self, Self::KmsFlipPending { event_id: expected } if expected == event_id)
    }
}

pub(super) fn copied_render_completion_matches(
    stage: InFlightStage,
    pending_bo_idx: usize,
    completion_render_job_id: u64,
    completion_bo_idx: usize,
) -> bool {
    pending_bo_idx == completion_bo_idx && stage.matches_render_completion(completion_render_job_id)
}

pub(super) fn kms_retirement_matches(
    stage: InFlightStage,
    pending_bo_idx: usize,
    presented_bo_idx: usize,
    event_id: Option<u64>,
) -> bool {
    pending_bo_idx == presented_bo_idx && stage.matches_kms_event(event_id)
}

impl SceneCompositor {
    /// True while any scene frame awaits either copied readiness or KMS
    /// retirement. Present completion pacing treats all such work as visible.
    pub fn has_pending_page_flips(&self) -> bool {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(v) = self.test_flip_in_flight_override {
            return v;
        }
        self.inner
            .as_ref()
            .is_some_and(|inner| inner.outputs.iter().any(|o| !o.pending_acks.is_empty()))
    }

    /// True while an accepted scene KMS commit awaits retirement on one
    /// output. Render-only handoffs are not page-flip obligations.
    pub fn has_pending_page_flip(&self, output_idx: usize) -> bool {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(v) = self.test_flip_in_flight_override {
            return v;
        }
        self.inner
            .as_ref()
            .and_then(|inner| inner.outputs.get(output_idx))
            .is_some_and(|output| {
                output
                    .pending_acks
                    .iter()
                    .any(|ack| matches!(ack.stage, InFlightStage::KmsFlipPending { .. }))
            })
    }

    /// True while any scene acknowledgement is pending on one output,
    /// including copied readiness and deferred-unflip preparation.
    pub fn has_pending_scene_frame(&self, output_idx: usize) -> bool {
        self.inner
            .as_ref()
            .and_then(|inner| inner.outputs.get(output_idx))
            .is_some_and(|output| !output.pending_acks.is_empty())
    }

    /// Identity of the accepted scene commit awaiting retirement on one
    /// output. A copied frame has none until its KMS submit accepts.
    pub fn pending_page_flip_event_id(&self, output_idx: usize) -> Option<u64> {
        let inner = self.inner.as_ref()?;
        let output = inner.outputs.get(output_idx)?;
        match output.pending_acks.front()?.stage {
            InFlightStage::KmsFlipPending { event_id } => event_id,
            InFlightStage::WaitingForRenderCompletion { .. }
            | InFlightStage::UnflipPrepared { .. }
            | InFlightStage::UnflipQuarantined { .. } => None,
        }
    }

    /// Deferred render handoffs that still own a scene acknowledgement,
    /// including copied outputs waiting for their source completion.
    pub fn pending_unflip_frame_identities(&self) -> Vec<DeferredFrameIdentity> {
        let Some(inner) = self.inner.as_ref() else {
            return Vec::new();
        };
        inner
            .outputs
            .iter()
            .enumerate()
            .flat_map(|(output_idx, output)| {
                output.pending_acks.iter().filter_map(move |ack| {
                    if ack.mode != FrameSubmitMode::DeferredUnflip
                        || !matches!(
                            ack.stage,
                            InFlightStage::WaitingForRenderCompletion { .. }
                                | InFlightStage::UnflipPrepared { .. }
                                | InFlightStage::UnflipQuarantined { .. }
                        )
                    {
                        return None;
                    }
                    Some(DeferredFrameIdentity {
                        output_idx,
                        bo_idx: ack.bo_idx,
                        render_job_id: ack.render_job_id,
                        recording_complete: ack.recording_complete,
                        topology_generation: ack.topology_generation?,
                    })
                })
            })
            .collect()
    }

    pub fn unflip_frame_is_quarantined(
        &self,
        output_idx: usize,
        bo_idx: usize,
        render_job_id: u64,
    ) -> bool {
        self.inner
            .as_ref()
            .and_then(|inner| inner.outputs.get(output_idx))
            .and_then(|output| output.pending_acks.front())
            .is_some_and(|ack| {
                ack.mode == FrameSubmitMode::DeferredUnflip
                    && ack.bo_idx == bo_idx
                    && ack.render_job_id == render_job_id
                    && ack.stage == (InFlightStage::UnflipQuarantined { render_job_id })
            })
    }

    /// Validate every deferred scene acknowledgement before submitting one
    /// all-output unflip transaction. Render readiness alone never creates a
    /// KMS retirement obligation.
    pub fn validate_grouped_unflip(
        &self,
        expected: &[DeferredFrameIdentity],
    ) -> Result<(), super::SceneError> {
        let Some(inner) = self.inner.as_ref() else {
            return Err(super::SceneError::InvalidGroupedUnflip(
                "scene is in stub mode".to_owned(),
            ));
        };
        if expected.len() != inner.outputs.len() {
            return Err(super::SceneError::InvalidGroupedUnflip(format!(
                "expected {} outputs but received {} identities",
                inner.outputs.len(),
                expected.len()
            )));
        }
        for (index, frame) in expected.iter().enumerate() {
            if expected[..index]
                .iter()
                .any(|earlier| earlier.output_idx == frame.output_idx)
            {
                return Err(super::SceneError::InvalidGroupedUnflip(format!(
                    "output {} appears more than once",
                    frame.output_idx
                )));
            }
            let ack = inner
                .outputs
                .get(frame.output_idx)
                .and_then(|output| output.pending_acks.front())
                .ok_or_else(|| {
                    super::SceneError::InvalidGroupedUnflip(format!(
                        "output {} has no pending acknowledgement",
                        frame.output_idx
                    ))
                })?;
            if ack.mode != FrameSubmitMode::DeferredUnflip
                || ack.bo_idx != frame.bo_idx
                || ack.render_job_id != frame.render_job_id
                || ack.recording_complete != frame.recording_complete
                || !ack.recording_complete
                || ack.topology_generation != Some(frame.topology_generation)
                || ack.stage
                    != (InFlightStage::UnflipPrepared {
                        render_job_id: frame.render_job_id,
                    })
            {
                return Err(super::SceneError::InvalidGroupedUnflip(format!(
                    "output {} acknowledgement does not match prepared frame {frame:?}",
                    frame.output_idx
                )));
            }
        }
        Ok(())
    }

    /// Publish the accepted all-output commit identity into exactly the
    /// acknowledgements validated before ioctl submission.
    pub fn accept_grouped_unflip(
        &mut self,
        expected: &[DeferredFrameIdentity],
        event_id: u64,
    ) -> Result<(), super::SceneError> {
        self.validate_grouped_unflip(expected)?;
        let needs_post_unflip_cursor_repaint = self.inner.as_ref().is_some_and(|inner| {
            expected.iter().any(|frame| {
                inner.outputs[frame.output_idx]
                    .pending_acks
                    .front()
                    .is_some_and(|ack| {
                        matches!(
                            ack.cursor_transition,
                            Some(super::CursorTransition::HideOnRetire {
                                reveal_sw_after: true
                            })
                        )
                    })
            })
        });
        let Some(inner) = self.inner.as_mut() else {
            return Err(super::SceneError::InvalidGroupedUnflip(
                "scene is in stub mode".to_owned(),
            ));
        };
        for frame in expected {
            let ack = inner.outputs[frame.output_idx]
                .pending_acks
                .front_mut()
                .expect("grouped unflip validation checked the acknowledgement");
            ack.stage = InFlightStage::KmsFlipPending {
                event_id: Some(event_id),
            };
        }
        if needs_post_unflip_cursor_repaint {
            // The accepted fresh frame is deliberately cursorless during a
            // HW→SW handoff. Keep the phase-two software-cursor frame queued
            // until this grouped replacement retires.
            self.scene_structure_dirty = true;
        }
        Ok(())
    }

    /// Remove one deferred acknowledgement after the backend proves its BO's
    /// final writer dependency safe, preserving the work for a full repaint.
    pub fn cancel_prepared_unflip(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
        render_job_id: u64,
        store: &mut DrawableStore,
    ) -> Result<(), super::SceneError> {
        let Some(inner) = self.inner.as_mut() else {
            return Err(super::SceneError::InvalidGroupedUnflip(
                "scene is in stub mode".to_owned(),
            ));
        };
        cancel_prepared_unflip_inner(inner, output_idx, bo_idx, render_job_id, store)?;
        self.scene_structure_dirty = true;
        Ok(())
    }

    /// Handle a DRM page-flip-complete event for `output_idx`.
    /// `event_id` is the identity the backend matched for the event; it must
    /// equal the pending frame's accepted identity before the sink's BO
    /// pool is touched. Pops the matching pending-ack, ack's its damage snapshots
    /// against the store, releases the descriptor-pool slot,
    /// then advances the platform's BO state machine via
    /// `on_page_flip_complete`. Engine retirement happens after
    /// (driven by the backend wrapper to keep the borrows clean).
    pub fn handle_page_flip_complete(
        &mut self,
        output_idx: usize,
        event_id: Option<u64>,
        store: &mut DrawableStore,
        sink: &mut dyn OutputSink,
    ) -> bool {
        let Some(inner) = self.inner.as_mut() else {
            return false;
        };
        let expected = inner
            .outputs
            .get(output_idx)
            .and_then(|state| state.pending_acks.front())
            .map(|ack| (ack.stage, ack.bo_idx));
        // Identity gate: a wrong or missing ID must leave the sink's BO
        // phases and pool untouched.
        if !expected.is_some_and(|(stage, _)| stage.matches_kms_event(event_id)) {
            return false;
        }
        let Some(retire) = sink.on_page_flip_complete(output_idx) else {
            return false;
        };
        let Some(state) = inner.outputs.get_mut(output_idx) else {
            return false;
        };
        if !expected.is_some_and(|(stage, bo_idx)| {
            kms_retirement_matches(stage, bo_idx, retire.presented_bo_idx, event_id)
        }) {
            log::warn!(
                "render scene: page-flip-complete on output {output_idx} presented bo {} \
                 but the pending scene frame was {expected:?}",
                retire.presented_bo_idx,
            );
            // `sink.on_page_flip_complete` above already advanced the BO
            // phase machine (previous OnScreen -> Free, Pending -> OnScreen), and
            // we are about to return without popping the ack. Platform and scene
            // have diverged, so neither `missing` nor any staged frame can be
            // trusted: fall back to "the whole output is stale", which costs one
            // full repaint and cannot show a stale pixel.
            //
            // The retained ack itself is a pre-existing wedge (the tick's
            // flip-pending gate will skip this output until another flip event
            // arrives); this is deliberately not the change that addresses it.
            state.damage.invalidate();
            return false;
        }
        if let Some(ack) = state.pending_acks.pop_front() {
            // Ack each per-drawable damage snapshot. Snapshots
            // from paint that landed after the tick's peek
            // survive (per I5 epoch semantics).
            for snap in ack.drawable_snapshots {
                if tick_skip_log_enabled() {
                    log::info!(
                        "ack-diag: out{output_idx} acked drawable={:?} epoch={} rects={}",
                        snap.id,
                        snap.epoch,
                        snap.region.rects().len(),
                    );
                }
                store.ack_presentation_damage(snap);
            }
            // Subtract the submitted output-damage snapshots
            // from live state (codex round 2 point 1). Damage
            // that arrived between submit and retirement
            // (map/unmap/cursor-move while flip in flight) is
            // NOT in the snapshots and therefore survives,
            // driving the next tick.
            state
                .scene_structure_damage
                .subtract(&ack.submitted_scene_structure_damage);
            state
                .pending_repaint_after_failed_submit
                .subtract(&ack.submitted_failed_repaint);
            // Push this frame's output damage onto the
            // buffer-age history ring keyed by its generation.
            state
                .damage_history
                .push(ack.generation, ack.submitted_output_damage);
            // Step 3 — the staged frame reached the screen: its damage is now
            // stale in every OTHER BO, and what it painted is no longer missing
            // from the one it painted into. A no-op when nothing was staged,
            // which is what makes it safe to call on a copied output too.
            state.damage.retire_success();
            // Step 2 — the frame reached the screen, so it becomes the baseline
            // the next diff runs against.
            state.prev_presented = ack.submitted_participants;
            // Release the matching pool slot — but only after the
            // compose CB's Vulkan fence has signaled. Pageflip
            // retirement is driven by KMS VBLANK, not by GPU
            // completion; if the GPU is still executing the compose
            // CB when this pageflip lands, releasing the pool slot
            // immediately calls vkResetDescriptorPool on a pool
            // whose descriptors are still bound to that CB
            // (VUID-vkResetDescriptorPool-descriptorPool-00313).
            // Fence-gate: if signaled now, release immediately;
            // otherwise defer to `pending_pool_releases` for the
            // drain pass to handle on a later tick.
            if let Some(slot) = state.pool_slots.pop_front() {
                match &ack.ticket {
                    None => state.pool_ring.release(slot),
                    Some(t) => match t.poll_signaled_result(&inner.vk) {
                        Ok(true) => state.pool_ring.release(slot),
                        Ok(false) => {
                            state.pending_pool_releases.push_back((slot, t.clone()));
                        }
                        Err(error) => {
                            log::error!(
                                "render scene: compose fence status failed at pageflip \
                                 retirement: {error:?}"
                            );
                            sink.gpu_mut().renderer_failed = true;
                            state.pending_pool_releases.push_back((slot, t.clone()));
                        }
                    },
                }
            }
            // Commit the BO's new last_present_generation in the
            // platform (the buffer-age pick uses this on next
            // acquire).
            sink.commit_slot_present(output_idx, retire.presented_bo_idx, ack.generation);

            // The KMS frame retired, but the cursor transition itself is a
            // second transaction. Record what actually happened: never claim
            // Hw after a rolled-back show and never claim Sw/Hidden while a
            // failed hide or rollback left the hardware sprite visible.
            let retained_mode = inner.outputs[output_idx].last_frame_cursor_mode;
            let cursor_result = apply_cursor_transition_on_retire(
                inner.cursor.as_ref(),
                output_idx,
                sink,
                ack.cursor_transition,
            );
            let state = inner
                .outputs
                .get_mut(output_idx)
                .expect("range checked above");
            let resolution = resolve_retired_cursor_state(
                cursor_result,
                ack.cursor_mode_after_retire,
                retained_mode,
            );
            state.force_show_retry_version = update_force_show_retry_version(
                state.force_show_retry_version,
                ack.cursor_transition,
                cursor_result,
                ack.cursor_mode_after_retire,
            );
            state.last_frame_cursor_mode = resolution.actual_mode;
            if resolution.commit_desired_metadata {
                if let Some(new_prev) = ack.cursor_prev_pos_after_retire {
                    state.cursor_prev_pos = new_prev;
                }
                state.last_present_cursor_rect = ack.last_present_cursor_rect_after_retire;
                state.last_present_cursor_version = ack.last_present_cursor_version_after_retire;
            } else {
                state.cursor_prev_pos = None;
                if resolution.clear_presented_metadata {
                    state.last_present_cursor_rect = None;
                    state.last_present_cursor_version = None;
                }
                // Otherwise preserve last-known metadata: the requested bind
                // did not fully land, so claiming the desired rect/version
                // could make later same-position damage incorrectly skip.
            }
            if resolution.force_repaint {
                force_cursor_retry_repaint(state);
                mark_cursor_transition_structure_dirty(
                    &mut self.scene_structure_dirty,
                    cursor_result,
                );
            }
            let actually_sw_composed =
                matches!(ack.cursor_mode_after_retire, OutputCursorMode::Sw { .. });
            if actually_sw_composed && sink.cursor_plane_mut().note_composed_retirement(output_idx)
            {
                // Consume this output/device's own EINVAL backoff one
                // composed retirement at a time. Damage keeps the sequence
                // alive until the retry becomes eligible.
                force_cursor_retry_repaint(state);
            }
            true
        } else {
            log::debug!(
                "render scene: page-flip-complete on output {output_idx} \
                 with no pending ack — startup flush or spurious event",
            );
            false
        }
    }

    /// Advance a copied source completion or safely cancel a deferred unflip
    /// target. Stale identities never retarget a rebuilt output vector.
    pub fn handle_render_notification(
        &mut self,
        notification: RenderNotification,
        store: &mut DrawableStore,
        sink: &mut dyn OutputSink,
    ) -> bool {
        let Some(inner) = self.inner.as_mut() else {
            return false;
        };
        handle_render_notification_inner(inner, notification, store, sink)
    }
}

pub(super) fn handle_render_notification_inner(
    inner: &mut SceneCompositorInner,
    notification: RenderNotification,
    store: &mut DrawableStore,
    sink: &mut dyn OutputSink,
) -> bool {
    match notification {
        RenderNotification::CopiedSourceReady(completion) => {
            handle_scanout_render_completion_inner(inner, completion, store, sink)
        }
        RenderNotification::UnflipDependencySafe {
            output,
            bo_idx,
            render_job_id,
            topology_generation,
        } => handle_unflip_dependency_safe_inner(
            inner,
            output,
            bo_idx,
            render_job_id,
            topology_generation,
            store,
            sink,
        ),
    }
}

pub(super) fn handle_scanout_render_completion_inner(
    inner: &mut SceneCompositorInner,
    completion: RenderCompletion,
    store: &mut DrawableStore,
    sink: &mut dyn OutputSink,
) -> bool {
    if sink.gpu().renderer_failed {
        return false;
    }
    let RenderCompletion {
        render_job_id,
        output,
        bo_idx,
        mode,
        recording_complete,
        topology_generation,
        fd,
    } = completion;
    let output_idx = match output {
        Ok(output_idx) => output_idx,
        Err(output_key) => {
            log::debug!(
                "render copied scanout: completion job {render_job_id} targeted removed output \
                     {output_key}",
            );
            return false;
        }
    };
    let expected = inner
        .outputs
        .get(output_idx)
        .and_then(|state| state.pending_acks.front())
        .is_some_and(|ack| {
            copied_render_completion_matches(ack.stage, ack.bo_idx, render_job_id, bo_idx)
                && ack.render_job_id == render_job_id
                && ack.mode == mode
                && ack.recording_complete == recording_complete
                && ack.topology_generation == topology_generation
        });
    if !expected {
        log::warn!(
            "render copied scanout: stale completion job {render_job_id} for output \
                 {output_idx} bo {bo_idx}",
        );
        // A live output retaining a different ledger entry means the
        // completed source can no longer be associated safely. Fail-stop
        // rather than guessing a pool slot or making either device's
        // allocation reusable while GPU ownership is uncertain.
        sink.gpu_mut().renderer_failed = true;
        return false;
    }

    if mode == FrameSubmitMode::DeferredUnflip && !recording_complete {
        // The source render has completed, so its destination is safe to
        // recycle. A truncated scene must never be copied or accepted as a
        // replacement frame.
        if let Err(error) = sink.recycle_unflip_target_after_dependency_safe(output_idx, bo_idx) {
            log::error!(
                "render unflip: incomplete copied output {output_idx} BO {bo_idx} job {render_job_id} could not be recycled: {error}"
            );
            sink.gpu_mut().renderer_failed = true;
            return false;
        }
        if let Err(error) =
            cancel_prepared_unflip_inner(inner, output_idx, bo_idx, render_job_id, store)
        {
            log::error!("render unflip: incomplete copied frame cancellation failed: {error}");
            sink.gpu_mut().renderer_failed = true;
            return false;
        }
        return true;
    }

    match sink.submit_copied(
        output_idx,
        bo_idx,
        crate::kms::render::scene::RenderJobIdentity {
            render_job_id,
            topology_generation,
        },
        recording_complete,
        mode,
        fd,
    ) {
        Ok(CopiedSubmitOutcome::FlipAccepted { event_id })
            if mode == FrameSubmitMode::Immediate =>
        {
            if let Some(ack) = inner.outputs[output_idx].pending_acks.front_mut() {
                ack.stage = InFlightStage::KmsFlipPending {
                    event_id: Some(event_id),
                };
            }
            true
        }
        Ok(CopiedSubmitOutcome::PreparedForGroup) if mode == FrameSubmitMode::DeferredUnflip => {
            if let Some(ack) = inner.outputs[output_idx].pending_acks.front_mut() {
                ack.stage = InFlightStage::UnflipPrepared { render_job_id };
            }
            true
        }
        Ok(CopiedSubmitOutcome::QuarantinedWithoutFinalDependency)
            if mode == FrameSubmitMode::DeferredUnflip =>
        {
            // Keep the exact render identity pending for a later safe
            // dependency notification, but do not advertise a prepared plane.
            if let Some(ack) = inner.outputs[output_idx].pending_acks.front_mut() {
                ack.stage = InFlightStage::UnflipQuarantined { render_job_id };
            }
            true
        }
        Ok(outcome) => {
            log::error!(
                "render copied scanout: invalid {mode:?} submit outcome {outcome:?} for output {output_idx} bo {bo_idx}"
            );
            sink.gpu_mut().renderer_failed = true;
            false
        }
        Err(error) => {
            log::warn!(
                "render copied scanout: sink copy/KMS submit failed for output \
                     {output_idx} bo {bo_idx}: {error}",
            );
            // The platform/resource layer synchronously idles B before
            // recovery and quarantines the pair if quiescence fails.
            // The scene may retire only A-local descriptor resources, and
            // only behind A's compose fence; damage/cursor metadata remain
            // live for a later repaint because no frame reached the screen.
            sink.invalidate_slot(output_idx, bo_idx);
            let state = &mut inner.outputs[output_idx];
            let ack = state
                .pending_acks
                .pop_front()
                .expect("completion identity was checked against the front ack");
            state.current_generation = ack.generation.saturating_sub(1);
            if let Some(rect) = ack.submitted_output_damage.bounding_rect() {
                state.pending_repaint_after_failed_submit.add(rect);
            }
            if let Some(slot) = state.pool_slots.pop_front() {
                match ack.ticket {
                    None => match sink.recycle_failed_slot(output_idx, bo_idx) {
                        Ok(()) => state.pool_ring.release(slot),
                        Err(recovery_error) => {
                            log::error!(
                                "render copied scanout: renderer recovery failed for output \
                                     {output_idx} bo {bo_idx}: {recovery_error}"
                            );
                            sink.gpu_mut().renderer_failed = true;
                        }
                    },
                    Some(ticket) => {
                        // Even if the fence is already signalled, route
                        // all post-A-submit failures through the same
                        // deferred recycler. That recycler rearms dirty
                        // export semaphores, retires the consumed prior
                        // B->A wait, and only then frees the paired slot.
                        state.failed_submit_bos.push_back(FailedSubmitBo {
                            bo_idx,
                            pool_slot: slot,
                            ticket,
                        });
                    }
                }
            } else {
                log::error!(
                    "render copied scanout: missing descriptor slot for failed output \
                         {output_idx} bo {bo_idx}"
                );
                sink.gpu_mut().renderer_failed = true;
            }
            state.next_submit_retry_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(100));
            false
        }
    }
}

fn handle_unflip_dependency_safe_inner(
    inner: &mut SceneCompositorInner,
    output: Result<usize, String>,
    bo_idx: usize,
    render_job_id: u64,
    topology_generation: u64,
    store: &mut DrawableStore,
    sink: &mut dyn OutputSink,
) -> bool {
    let output_idx = match output {
        Ok(output_idx) => output_idx,
        Err(output_key) => {
            log::debug!(
                "render unflip: safe-dependency notification for removed output {output_key} job {render_job_id}"
            );
            return false;
        }
    };
    let matches = inner
        .outputs
        .get(output_idx)
        .and_then(|state| state.pending_acks.front())
        .is_some_and(|ack| {
            ack.mode == FrameSubmitMode::DeferredUnflip
                && ack.bo_idx == bo_idx
                && ack.render_job_id == render_job_id
                && ack.topology_generation == Some(topology_generation)
                && matches!(
                    ack.stage,
                    InFlightStage::WaitingForRenderCompletion { render_job_id: pending }
                        | InFlightStage::UnflipPrepared { render_job_id: pending }
                        if pending == render_job_id
                )
        });
    if !matches {
        log::warn!(
            "render unflip: stale safe-dependency notification job {render_job_id} for output {output_idx} bo {bo_idx}"
        );
        return false;
    }
    if let Err(error) = sink.recycle_unflip_target_after_dependency_safe(output_idx, bo_idx) {
        log::error!(
            "render unflip: failed to recycle safe output {output_idx} bo {bo_idx}: {error}"
        );
        sink.gpu_mut().renderer_failed = true;
        return false;
    }
    sink.invalidate_slot(output_idx, bo_idx);
    match cancel_prepared_unflip_inner(inner, output_idx, bo_idx, render_job_id, store) {
        Ok(()) => true,
        Err(error) => {
            log::error!("render unflip: safe dependency cancellation failed: {error}");
            sink.gpu_mut().renderer_failed = true;
            false
        }
    }
}

fn cancel_prepared_unflip_inner(
    inner: &mut SceneCompositorInner,
    output_idx: usize,
    bo_idx: usize,
    render_job_id: u64,
    _store: &mut DrawableStore,
) -> Result<(), super::SceneError> {
    let Some(state) = inner.outputs.get_mut(output_idx) else {
        return Err(super::SceneError::InvalidGroupedUnflip(format!(
            "output {output_idx} no longer exists"
        )));
    };
    let matches = state.pending_acks.front().is_some_and(|ack| {
        ack.mode == FrameSubmitMode::DeferredUnflip
            && ack.bo_idx == bo_idx
            && ack.render_job_id == render_job_id
            && matches!(
                ack.stage,
                InFlightStage::WaitingForRenderCompletion { render_job_id: pending }
                    | InFlightStage::UnflipPrepared { render_job_id: pending }
                    if pending == render_job_id
            )
    });
    if !matches {
        return Err(super::SceneError::InvalidGroupedUnflip(format!(
            "output {output_idx} has no deferred acknowledgement for BO {bo_idx} job {render_job_id}"
        )));
    }
    let ack = state
        .pending_acks
        .pop_front()
        .expect("the matching deferred acknowledgement was checked");
    if let Some(rect) = ack.submitted_output_damage.bounding_rect() {
        state.pending_repaint_after_failed_submit.add(rect);
    }
    state.current_generation = ack.generation.saturating_sub(1);
    state.damage.invalidate();
    state.cursor_prev_pos = None;
    if let Some(slot) = state.pool_slots.pop_front() {
        match ack.ticket {
            None => state.pool_ring.release(slot),
            Some(ticket) => state.pending_pool_releases.push_back((slot, ticket)),
        }
    } else {
        return Err(super::SceneError::InvalidGroupedUnflip(format!(
            "output {output_idx} has no descriptor slot for cancelled BO {bo_idx}"
        )));
    }
    // Drawable snapshots are deliberately not acked on cancellation. Their
    // damage remains in the store for the replacement full repaint.
    Ok(())
}

pub(super) fn retire_failed_submit_bos(
    state: &mut OutputSceneState,
    output_idx: usize,
    sink: &mut dyn OutputSink,
    vk: &crate::kms::vk::device::VkContext,
) {
    let mut remaining = VecDeque::with_capacity(state.failed_submit_bos.len());
    while let Some(failed) = state.failed_submit_bos.pop_front() {
        match failed.ticket.poll_signaled_result(vk) {
            Ok(true) => match sink.recycle_failed_slot(output_idx, failed.bo_idx) {
                Ok(()) => {
                    state.pool_ring.release(failed.pool_slot);
                    log::debug!(
                        "render scene: recycled failed-submit output {output_idx} bo {} pool slot {}",
                        failed.bo_idx,
                        failed.pool_slot,
                    );
                }
                Err(error) => {
                    log::error!(
                        "render scene: failed-submit recovery failed for output {output_idx} \
                         bo {}: {error}",
                        failed.bo_idx,
                    );
                    sink.gpu_mut().renderer_failed = true;
                    remaining.push_back(failed);
                }
            },
            Ok(false) => remaining.push_back(failed),
            Err(error) => {
                log::error!(
                    "render scene: failed-submit fence status failed for output {output_idx} \
                     bo {}: {error:?}",
                    failed.bo_idx,
                );
                sink.gpu_mut().renderer_failed = true;
                remaining.push_back(failed);
            }
        }
    }
    state.failed_submit_bos = remaining;
}

/// Drain the deferred descriptor-pool slot releases queued by
/// `handle_page_flip_complete` when the compose fence hadn't yet
/// signaled at pageflip-retirement time. Mirrors
/// `retire_failed_submit_bos`'s walk-once-poll-or-defer shape.
/// Slots whose fence has now signaled are returned to the ring;
/// the rest stay queued for the next drain.
pub(super) fn drain_pending_pool_releases(
    state: &mut OutputSceneState,
    vk: &crate::kms::vk::device::VkContext,
    sink: &mut dyn OutputSink,
) {
    if state.pending_pool_releases.is_empty() {
        return;
    }
    let mut remaining = VecDeque::with_capacity(state.pending_pool_releases.len());
    while let Some((slot, ticket)) = state.pending_pool_releases.pop_front() {
        match ticket.poll_signaled_result(vk) {
            Ok(true) => state.pool_ring.release(slot),
            Ok(false) => remaining.push_back((slot, ticket)),
            Err(error) => {
                log::error!("render scene: deferred pool fence status failed: {error:?}");
                sink.gpu_mut().renderer_failed = true;
                remaining.push_back((slot, ticket));
            }
        }
    }
    state.pending_pool_releases = remaining;
}
