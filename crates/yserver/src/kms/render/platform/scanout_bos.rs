use crate::kms::{
    render::{platform::PlatformBackend, scene::SlotToken},
    vk::scanout::{BoPhase, BoState, OutputScanout},
};
use ash::vk;
use std::{
    io,
    os::fd::{AsRawFd as _, FromRawFd, IntoRawFd, OwnedFd, RawFd},
    rc::Rc,
};

/// The topology identity captured when one deferred-unflip render batch
/// starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UnflipBatchContext {
    pub(crate) topology_generation: u64,
}

/// A scene acknowledgement paired with its current output snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExpectedUnflipPlane {
    pub(crate) output_idx: usize,
    pub(crate) output_key: crate::kms::backend::OutputKey,
    pub(crate) bo_idx: usize,
    pub(crate) render_job_id: u64,
    pub(crate) recording_complete: bool,
    pub(crate) topology_generation: u64,
}

/// A scanout BO whose final render or sink-copy dependency is available for a
/// grouped atomic replacement. The descriptor remains owned by `BoState`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedUnflipPlane {
    pub(crate) output_idx: usize,
    pub(crate) output_key: crate::kms::backend::OutputKey,
    pub(crate) bo_idx: usize,
    pub(crate) render_job_id: u64,
    pub(crate) recording_complete: bool,
    pub(crate) topology_generation: u64,
    pub(crate) final_in_fence: Option<RawFd>,
}

/// Readiness boundary for prepared planes. The production implementation
/// publishes every row immediately; recording fixtures can delay publication
/// without changing the BO submission or the Backend path.
pub(crate) trait PreparedUnflipReadiness {
    fn take_ready(&self, prepared: &mut Vec<PreparedUnflipPlane>) -> Vec<PreparedUnflipPlane>;
}

pub(crate) struct ImmediatePreparedUnflipReadiness;

impl PreparedUnflipReadiness for ImmediatePreparedUnflipReadiness {
    fn take_ready(&self, prepared: &mut Vec<PreparedUnflipPlane>) -> Vec<PreparedUnflipPlane> {
        std::mem::take(prepared)
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingPreparedUnflipReadiness {
    held_outputs: std::cell::RefCell<std::collections::HashSet<usize>>,
}

#[cfg(test)]
impl RecordingPreparedUnflipReadiness {
    pub(crate) fn hold_output(&self, output_idx: usize) {
        self.held_outputs.borrow_mut().insert(output_idx);
    }

    pub(crate) fn release_output(&self, output_idx: usize) {
        self.held_outputs.borrow_mut().remove(&output_idx);
    }
}

#[cfg(test)]
impl PreparedUnflipReadiness for RecordingPreparedUnflipReadiness {
    fn take_ready(&self, prepared: &mut Vec<PreparedUnflipPlane>) -> Vec<PreparedUnflipPlane> {
        let held = self.held_outputs.borrow();
        let mut ready = Vec::new();
        let mut retained = Vec::new();
        for plane in prepared.drain(..) {
            if held.contains(&plane.output_idx) {
                retained.push(plane);
            } else {
                ready.push(plane);
            }
        }
        *prepared = retained;
        ready
    }
}

pub(crate) fn prepared_unflip_plane(
    output_idx: usize,
    output_key: crate::kms::backend::OutputKey,
    bo_idx: usize,
    render_job_id: u64,
    recording_complete: bool,
    topology_generation: u64,
    final_in_fence: Option<RawFd>,
) -> PreparedUnflipPlane {
    PreparedUnflipPlane {
        output_idx,
        output_key,
        bo_idx,
        render_job_id,
        recording_complete,
        topology_generation,
        final_in_fence,
    }
}

/// Pure readiness decision for one complete all-output replacement.
pub(crate) fn grouped_unflip_ready(
    expected_outputs: &[crate::kms::backend::OutputKey],
    expected: &[ExpectedUnflipPlane],
    prepared: &[PreparedUnflipPlane],
) -> bool {
    if expected_outputs.is_empty()
        || expected.len() != expected_outputs.len()
        || prepared.len() != expected_outputs.len()
        || expected
            .iter()
            .any(|plane| plane.topology_generation != expected[0].topology_generation)
    {
        return false;
    }

    for (output_idx, output_key) in expected_outputs.iter().enumerate() {
        if expected_outputs[..output_idx].contains(output_key) {
            return false;
        }
        let mut expected_matches = expected
            .iter()
            .filter(|plane| plane.output_key == *output_key);
        let Some(authoritative) = expected_matches.next() else {
            return false;
        };
        if expected_matches.next().is_some()
            || authoritative.output_idx != output_idx
            || !authoritative.recording_complete
        {
            return false;
        }
        let mut prepared_matches = prepared
            .iter()
            .filter(|plane| plane.output_key == *output_key);
        let Some(plane) = prepared_matches.next() else {
            return false;
        };
        if prepared_matches.next().is_some()
            || plane.output_idx != authoritative.output_idx
            || plane.bo_idx != authoritative.bo_idx
            || plane.render_job_id != authoritative.render_job_id
            || !plane.recording_complete
            || plane.recording_complete != authoritative.recording_complete
            || plane.topology_generation != authoritative.topology_generation
        {
            return false;
        }
    }

    expected.iter().all(|plane| {
        expected_outputs
            .get(plane.output_idx)
            .is_some_and(|output_key| output_key == &plane.output_key)
    }) && prepared.iter().all(|plane| {
        expected_outputs
            .get(plane.output_idx)
            .is_some_and(|output_key| output_key == &plane.output_key)
    })
}

impl PlatformBackend {
    #[cfg(test)]
    pub(crate) fn for_tests_set_prepared_unflip_readiness(
        &mut self,
        readiness: Rc<dyn PreparedUnflipReadiness>,
    ) {
        self.prepared_unflip_readiness = readiness;
    }

    pub(crate) fn install_unflip_batch_context(&mut self, topology_generation: u64) {
        self.unflip_batch_context = Some(UnflipBatchContext {
            topology_generation,
        });
    }

    pub(crate) fn clear_unflip_batch_context(&mut self) {
        self.unflip_batch_context = None;
    }

    pub(crate) fn prepare_unflip_plane(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
        render_job_id: u64,
        recording_complete: bool,
        topology_generation: u64,
        final_in_fence: Option<RawFd>,
    ) -> io::Result<()> {
        self.debug_assert_scanout_pool_route(output_idx);
        let output_key = self
            .outputs
            .get(output_idx)
            .map(|output| output.key.clone())
            .ok_or_else(|| io::Error::other("unflip output index is out of range"))?;
        let bo = self
            .scanout_pools
            .get(output_idx)
            .and_then(Option::as_ref)
            .and_then(|pool| pool.display_pool().bos.get(bo_idx))
            .ok_or_else(|| io::Error::other("unflip scanout BO index is out of range"))?;
        if bo.state.phase != BoPhase::Submitted {
            return Err(io::Error::other(format!(
                "unflip output {output_idx} BO {bo_idx} is {:?}, expected Submitted",
                bo.state.phase
            )));
        }
        if bo.state.in_fence_fd.map(|fd| fd as RawFd) != final_in_fence {
            return Err(io::Error::other(format!(
                "unflip output {output_idx} BO {bo_idx} final dependency does not match its owned fence"
            )));
        }
        if self.prepared_unflip_planes.iter().any(|prepared| {
            prepared.output_idx == output_idx
                && prepared.bo_idx == bo_idx
                && prepared.render_job_id == render_job_id
        }) {
            return Err(io::Error::other(format!(
                "unflip output {output_idx} BO {bo_idx} job {render_job_id} was prepared twice"
            )));
        }
        self.prepared_unflip_planes.push(prepared_unflip_plane(
            output_idx,
            output_key,
            bo_idx,
            render_job_id,
            recording_complete,
            topology_generation,
            final_in_fence,
        ));
        Ok(())
    }

    /// Record the copied route's final dependency after its sink queue submit.
    /// The identity is captured before scene recording, so replacing the
    /// installed batch context while the source render is pending cannot
    /// retag this plane.
    pub(crate) fn prepare_copied_unflip_plane(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
        identity: crate::kms::render::scene::RenderJobIdentity,
        recording_complete: bool,
        mode: crate::kms::render::scene::FrameSubmitMode,
        source_render_dependency: Option<RawFd>,
    ) -> io::Result<()> {
        if mode != crate::kms::render::scene::FrameSubmitMode::DeferredUnflip {
            return Err(io::Error::other(
                "copied unflip preparation requires deferred frame mode",
            ));
        }
        let topology_generation = identity.topology_generation.ok_or_else(|| {
            io::Error::other("copied unflip completion has no captured topology generation")
        })?;
        let sink_copy_dependency = self
            .scanout_pools
            .get(output_idx)
            .and_then(Option::as_ref)
            .and_then(OutputScanout::copied)
            .and_then(|pool| pool.destinations.bos.get(bo_idx))
            .filter(|bo| bo.state.phase == BoPhase::Submitted)
            .map(|bo| bo.state.in_fence_fd.map(|fd| fd as RawFd))
            .ok_or_else(|| io::Error::other("copied unflip destination is not submitted"))?;
        let final_in_fence = crate::kms::vk::scanout::copied_final_dependency(
            source_render_dependency,
            sink_copy_dependency,
        );
        self.prepare_unflip_plane(
            output_idx,
            bo_idx,
            identity.render_job_id,
            recording_complete,
            topology_generation,
            final_in_fence,
        )
    }

    fn discard_prepared_unflip_plane(&mut self, output_idx: usize, bo_idx: usize) {
        self.prepared_unflip_planes
            .retain(|plane| plane.output_idx != output_idx || plane.bo_idx != bo_idx);
    }

    pub(crate) fn take_prepared_unflip_planes(&mut self) -> Vec<PreparedUnflipPlane> {
        self.prepared_unflip_readiness
            .take_ready(&mut self.prepared_unflip_planes)
    }

    #[cfg(test)]
    pub(crate) fn for_tests_unflip_ready_state_is_empty(&self) -> bool {
        self.unflip_batch_context.is_none() && self.prepared_unflip_planes.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn for_tests_unflip_owned_fd_count(&self) -> usize {
        let prepared = self
            .prepared_unflip_planes
            .iter()
            .filter(|plane| plane.final_in_fence.is_some())
            .count();
        let render_completions = self
            .pending_scanout_render_completions
            .iter()
            .filter(|pending| {
                pending.mode == crate::kms::render::scene::FrameSubmitMode::DeferredUnflip
            })
            .filter(|pending| pending.fd.is_some())
            .count();
        let safe_dependencies = self.pending_scanout_unflip_dependencies.len();
        let bo_fences = self
            .scanout_pools
            .iter()
            .filter_map(Option::as_ref)
            .flat_map(|pool| pool.display_pool().bos.iter())
            .filter(|bo| bo.state.in_fence_fd.is_some())
            .count();
        prepared + render_completions + safe_dependencies + bo_fences
    }

    /// Called after grouped atomic acceptance; only now may the userspace
    /// input-fence descriptor be closed and the BO become flip-pending.
    pub(crate) fn accept_prepared_unflip_plane(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
    ) -> io::Result<()> {
        let bo = self
            .scanout_pools
            .get_mut(output_idx)
            .and_then(Option::as_mut)
            .and_then(|pool| pool.display_pool_mut().bos.get_mut(bo_idx))
            .ok_or_else(|| io::Error::other("accepted unflip scanout BO disappeared"))?;
        if bo.state.phase != BoPhase::Submitted {
            return Err(io::Error::other(format!(
                "accepted unflip output {output_idx} BO {bo_idx} is {:?}, expected Submitted",
                bo.state.phase
            )));
        }
        if let Some(fd) = bo.state.transition_to_pending(-1) {
            // SAFETY: transition_to_pending transferred the BO-owned
            // userspace IN_FENCE_FD after the accepted atomic commit.
            drop(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        self.discard_prepared_unflip_plane(output_idx, bo_idx);
        Ok(())
    }

    // ── I6b: scanout BO management ──────────────────────────────

    pub(super) fn debug_assert_scanout_pool_route(&self, output_idx: usize) {
        if let Some(pool) = self.scanout_pools.get(output_idx).and_then(Option::as_ref) {
            debug_assert_eq!(
                self.outputs
                    .get(output_idx)
                    .map(|output| output.scanout_route),
                Some(pool.route()),
                "scanout pool must stay paired with its output's renderer-to-KMS route"
            );
        }
    }

    /// Pick the next BO to render into for `output_idx`, or
    /// `None` if all BOs are still in flight (the SceneCompositor
    /// should retry next core-loop iteration).
    ///
    /// The token carries `last_present_generation` and
    /// `content_invalidated` so the buffer-age algorithm in
    /// SceneCompositor doesn't need to reach into the pool.
    pub(crate) fn acquire_scanout_bo(&mut self, output_idx: usize) -> Option<SlotToken> {
        self.debug_assert_scanout_pool_route(output_idx);
        let scanout = self.scanout_pools.get_mut(output_idx)?.as_mut()?;
        let gens = self.bo_generations.get(output_idx)?;
        for (bo_idx, bo) in scanout.display_pool().bos.iter().enumerate() {
            if bo.state.phase == BoPhase::Free {
                let entry = gens.get(bo_idx).copied().unwrap_or_default();
                return Some(SlotToken {
                    output_idx,
                    bo_idx,
                    extent: vk::Extent2D {
                        width: bo.width,
                        height: bo.height,
                    },
                    last_present_generation: entry.last_present_generation,
                    content_invalidated: entry.content_invalidated,
                });
            }
        }
        None
    }

    /// Advance one copied frame from a completed source-render submission to
    /// the sink copy and KMS page flip.  Poll readiness is only a scheduling
    /// boundary: the source `sync_file` is still imported and waited by the
    /// sink Vulkan submission, whose exported completion becomes KMS's
    /// `IN_FENCE_FD`.
    pub(crate) fn submit_copied_scanout(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
        identity: crate::kms::render::scene::RenderJobIdentity,
        recording_complete: bool,
        mode: crate::kms::render::scene::FrameSubmitMode,
        render_completion: Option<OwnedFd>,
    ) -> io::Result<crate::kms::render::scene::CopiedSubmitOutcome> {
        let _topology_generation = match mode {
            crate::kms::render::scene::FrameSubmitMode::Immediate
                if identity.topology_generation.is_none() =>
            {
                None
            }
            crate::kms::render::scene::FrameSubmitMode::DeferredUnflip
                if identity.topology_generation.is_some() =>
            {
                identity.topology_generation
            }
            _ => {
                return Err(io::Error::other(
                    "copied completion topology generation does not match frame mode",
                ));
            }
        };
        let render_job_id = identity.render_job_id;
        let source_dependency = render_completion.as_ref().map(|fd| fd.as_raw_fd());
        let readiness_observer = Rc::clone(&self.render_completion_readiness);
        self.debug_assert_scanout_pool_route(output_idx);
        let output_key = self
            .outputs
            .get(output_idx)
            .map(|output| output.key.clone())
            .ok_or_else(|| io::Error::other("copied scanout output index out of range"))?;
        let device = self
            .device_for_output(&output_key)
            .map(|device| Rc::clone(&device.device))
            .ok_or_else(|| io::Error::other("copied scanout KMS device disappeared"))?;
        let atomic_commit_sink = self
            .device_for_output(&output_key)
            .map(|device| Rc::clone(&device.atomic_commit_sink))
            .ok_or_else(|| io::Error::other("copied scanout atomic commit sink disappeared"))?;
        let cursor_positions = self.cursor_plane_atomic_positions_for_outputs(&[output_idx]);

        let mut recovery_failed = false;
        let mut quarantined_dependency = None;
        let mut prepared_copied_handoff = None;
        let result = (|| {
            // `Output` deliberately is not Clone: its DRM handles belong to
            // the owning open fd. Borrow the output and its independently
            // indexed scanout pool through disjoint PlatformBackend fields.
            let output = &self
                .outputs
                .get(output_idx)
                .ok_or_else(|| io::Error::other("copied scanout output disappeared"))?
                .output;
            let copied = self
                .scanout_pools
                .get_mut(output_idx)
                .and_then(Option::as_mut)
                .and_then(OutputScanout::copied_mut)
                .ok_or_else(|| io::Error::other("render completion targeted a shared output"))?;
            let framebuffer = copied
                .destinations
                .bos
                .get(bo_idx)
                .ok_or_else(|| io::Error::other("copied destination index out of range"))?
                .fb_handle
                .ok_or_else(|| io::Error::other("copied destination has no framebuffer"))?;

            let copy_completion = match mode {
                crate::kms::render::scene::FrameSubmitMode::Immediate => {
                    match copied.submit_copy(bo_idx, render_completion) {
                        Ok(fd) => fd,
                        Err(error) => {
                            if let Err(recovery_error) = copied.recover_copy_failure(bo_idx) {
                                recovery_failed = true;
                                log::error!(
                                    "copied scanout submission failed ({error}) and the sink could not be quiesced: {recovery_error}"
                                );
                                return Err(io::Error::new(recovery_error.kind(), recovery_error));
                            }
                            return Err(error);
                        }
                    }
                }
                crate::kms::render::scene::FrameSubmitMode::DeferredUnflip => {
                    match copied.submit_copy_deferred(bo_idx, render_completion) {
                        Ok(fd) => fd,
                        Err(crate::kms::vk::scanout::DeferredCopiedSubmitError::BeforeSubmit(
                            error,
                        )) => {
                            if let Err(recovery_error) =
                                copied.recover_deferred_copy_after_dependency_safe(bo_idx)
                            {
                                recovery_failed = true;
                                log::error!(
                                    "deferred copied scanout failed before queue submit ({error}) and local recovery failed: {recovery_error}"
                                );
                                return Err(io::Error::new(recovery_error.kind(), recovery_error));
                            }
                            return Err(error);
                        }
                        Err(crate::kms::vk::scanout::DeferredCopiedSubmitError::Submitted(
                            error,
                        )) => {
                            readiness_observer.note_sink_copy_submitted(
                                output_idx,
                                bo_idx,
                                render_job_id,
                                mode,
                            );
                            let destination = copied
                                .destinations
                                .bos
                                .get_mut(bo_idx)
                                .expect("copied destination was checked before submission");
                            destination.state.transition_to_submitted(-1);
                            let Some((sink_vk, fence)) = copied.quarantined_dependency(bo_idx)
                            else {
                                recovery_failed = true;
                                log::error!(
                                    "deferred copied scanout failed after submit ({error}) without its quarantine fence"
                                );
                                return Ok(crate::kms::render::scene::CopiedSubmitOutcome::QuarantinedWithoutFinalDependency);
                            };
                            quarantined_dependency = Some((output_key.clone(), sink_vk, fence));
                            log::warn!(
                                "deferred copied scanout job {} has no final dependency ({error}); waiting asynchronously before recycling",
                                render_job_id
                            );
                            return Ok(crate::kms::render::scene::CopiedSubmitOutcome::QuarantinedWithoutFinalDependency);
                        }
                        Err(
                            crate::kms::vk::scanout::DeferredCopiedSubmitError::PreviousSubmissionPending(
                                error,
                            ),
                        ) => {
                            recovery_failed = true;
                            log::error!(
                                "deferred copied scanout job {render_job_id} cannot reuse its target: {error}"
                            );
                            return Ok(crate::kms::render::scene::CopiedSubmitOutcome::QuarantinedWithoutFinalDependency);
                        }
                    }
                }
            };
            readiness_observer.note_sink_copy_submitted(output_idx, bo_idx, render_job_id, mode);
            let destination = copied
                .destinations
                .bos
                .get_mut(bo_idx)
                .expect("copied destination was checked before copy submission");
            let in_fence_fd = copy_completion.map_or(-1, IntoRawFd::into_raw_fd);
            destination.state.transition_to_submitted(in_fence_fd);
            if mode == crate::kms::render::scene::FrameSubmitMode::DeferredUnflip {
                prepared_copied_handoff = Some(source_dependency);
                return Ok(crate::kms::render::scene::CopiedSubmitOutcome::PreparedForGroup);
            }
            let mut out_fence_fd = -1;
            match crate::drm::page_flip::submit_scene_flip_with_fences(
                &device,
                atomic_commit_sink.as_ref(),
                output,
                framebuffer,
                in_fence_fd,
                &mut out_fence_fd,
                &cursor_positions,
                crate::drm::async_flip::FlipTiming::Vsync,
            ) {
                Ok(event_id) => {
                    if let Some(fd) = destination.state.transition_to_pending(out_fence_fd) {
                        // SAFETY: transition_to_pending transfers the uniquely
                        // owned input-fence fd back to this caller.
                        unsafe { libc::close(fd) };
                    }
                    Ok(crate::kms::render::scene::CopiedSubmitOutcome::FlipAccepted { event_id })
                }
                Err(error) => {
                    if let Some(fd) = destination
                        .state
                        .transition_to_recording_after_atomic_reject()
                    {
                        // SAFETY: the state transition returns unique fd
                        // ownership after the rejected atomic commit.
                        unsafe { libc::close(fd) };
                    }
                    if out_fence_fd >= 0 {
                        // SAFETY: the kernel wrote a uniquely-owned fd into
                        // our out-fence slot even though the commit failed.
                        unsafe { libc::close(out_fence_fd) };
                    }
                    if let Err(recovery_error) = copied.recover_copy_failure(bo_idx) {
                        recovery_failed = true;
                        log::error!(
                            "copied scanout atomic commit failed ({error}) and the sink could not be quiesced: {recovery_error}"
                        );
                        Err(io::Error::new(recovery_error.kind(), recovery_error))
                    } else {
                        Err(error)
                    }
                }
            }
        })();

        if let Some((_output_key, _sink_vk, _fence)) = quarantined_dependency {
            // A successful queue submit with no exportable final dependency
            // cannot be represented as render readiness. Keep the copied pair
            // quarantined until the existing full lifecycle/device quiescence
            // boundary; never synthesize an UnflipDependencySafe event here.
            log::error!(
                "deferred copied scanout job {render_job_id} remains quarantined without a final dependency"
            );
        }

        if mode == crate::kms::render::scene::FrameSubmitMode::Immediate
            && atomic_commit_sink.take_last_cursor_rejection()
            && let Err(error) = &result
        {
            let rejected = self.reject_atomic_cursor_commit(&cursor_positions, &error.to_string());
            self.defer_cursor_move_outcome(rejected);
        }

        if matches!(
            result,
            Ok(crate::kms::render::scene::CopiedSubmitOutcome::FlipAccepted { .. })
        ) {
            self.cursor_plane_scene_commit_submitted(&cursor_positions);
        }

        if recovery_failed
            || result
                .as_ref()
                .is_err_and(crate::kms::vk::scanout::scanout_error_is_device_lost)
        {
            // The failing operation used a live renderer or sink transfer
            // device.  Neither uncertain image state may be reused.
            self.gpu.renderer_failed = true;
        }
        if let Some(source_render_dependency) = prepared_copied_handoff {
            self.prepare_copied_unflip_plane(
                output_idx,
                bo_idx,
                identity,
                recording_complete,
                mode,
                source_render_dependency,
            )?;
        }
        result
    }

    /// A deferred frame target may only be reused after its final render or
    /// sink-copy dependency is safe. Resolve copied ownership and close the
    /// retained in-fence before returning the BO to the free pool.
    pub(crate) fn recycle_unflip_scanout_bo_after_dependency_safe(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
    ) -> io::Result<()> {
        self.debug_assert_scanout_pool_route(output_idx);
        let Some(scanout) = self.scanout_pools.get(output_idx).and_then(Option::as_ref) else {
            return Err(io::Error::other(
                "unflip output no longer has a scanout pool",
            ));
        };
        let Some(bo) = scanout.display_pool().bos.get(bo_idx) else {
            return Err(io::Error::other("unflip BO index is out of range"));
        };
        if !matches!(bo.state.phase, BoPhase::Recording | BoPhase::Submitted) {
            return Err(io::Error::other(format!(
                "unflip target output {output_idx} BO {bo_idx} is {:?}, expected Recording or Submitted",
                bo.state.phase
            )));
        }
        let scanout = self
            .scanout_pools
            .get_mut(output_idx)
            .and_then(Option::as_mut)
            .ok_or_else(|| io::Error::other("unflip output no longer has a scanout pool"))?;
        match scanout {
            OutputScanout::Shared(pool) => {
                let bo = pool
                    .bos
                    .get_mut(bo_idx)
                    .ok_or_else(|| io::Error::other("unflip BO index is out of range"))?;
                bo.rearm_export_semaphore_after_quiescence()
                    .map_err(|result| {
                        io::Error::other(format!(
                            "rearm shared scanout export semaphore after unflip cancellation: {result:?}"
                        ))
                    })?;
                if let Some(fd) = bo.state.in_fence_fd.take() {
                    // SAFETY: the dependency-safe notification proves this
                    // BO's uniquely-owned final input fence is no longer in
                    // use, and `take` prevents a second close on reset.
                    drop(unsafe { OwnedFd::from_raw_fd(fd) });
                }
                bo.state.reset_after_dependency_safe();
            }
            OutputScanout::Copied(pool) => {
                pool.recover_deferred_copy_after_dependency_safe(bo_idx)?;
                pool.sources
                    .get_mut(bo_idx)
                    .ok_or_else(|| io::Error::other("copied source index out of range"))?
                    .recover_failed_cycle_after_renderer_quiescence()?;
                let destination_state = &mut pool
                    .destinations
                    .bos
                    .get_mut(bo_idx)
                    .ok_or_else(|| io::Error::other("copied destination index out of range"))?
                    .state;
                if let Some(fd) = destination_state.in_fence_fd.take() {
                    // SAFETY: the notification proves the copied sink
                    // dependency completed, so the BO-owned fd is safe to
                    // close before resetting the state.
                    drop(unsafe { OwnedFd::from_raw_fd(fd) });
                }
                destination_state.reset_after_dependency_safe();
            }
        }
        self.discard_prepared_unflip_plane(output_idx, bo_idx);
        Ok(())
    }

    /// Framebuffer last presented by the scene compositor for this output.
    /// During M2 direct scanout the pool deliberately keeps this BO marked
    /// `OnScreen`: it is the known-good per-output target for one atomic
    /// transition back from the shared client framebuffer.
    pub(crate) fn retained_composed_framebuffer(
        &self,
        output_idx: usize,
    ) -> Option<::drm::control::framebuffer::Handle> {
        self.debug_assert_scanout_pool_route(output_idx);
        self.scanout_pools
            .get(output_idx)?
            .as_ref()?
            .display_pool()
            .bos
            .iter()
            .find(|bo| bo.state.phase == BoPhase::OnScreen)?
            .fb_handle
    }

    /// Mark a BO's content tracking as invalidated. Called by
    /// SceneCompositor on the 9b atomic-commit-failed path —
    /// the GPU rendered into the BO but KMS rejected the flip,
    /// so the BO contents are indeterminate.
    pub(crate) fn invalidate_bo(&mut self, output_idx: usize, bo_idx: usize) {
        if let Some(gens) = self.bo_generations.get_mut(output_idx)
            && let Some(g) = gens.get_mut(bo_idx)
        {
            g.content_invalidated = true;
            g.last_present_generation = None;
        }
    }

    /// Recycle a scanout BO whose GPU work was submitted but whose
    /// atomic commit was rejected. The caller must only invoke this
    /// after the compose fence has signaled, otherwise the BO could
    /// be rendered into again while the previous command buffer is
    /// still writing it.
    pub(crate) fn recycle_failed_submit_bo(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
    ) -> io::Result<()> {
        self.debug_assert_scanout_pool_route(output_idx);
        let Some(scanout) = self
            .scanout_pools
            .get_mut(output_idx)
            .and_then(Option::as_mut)
        else {
            return Ok(());
        };
        match scanout {
            OutputScanout::Shared(pool) => {
                let Some(bo) = pool.bos.get_mut(bo_idx) else {
                    return Ok(());
                };
                bo.rearm_export_semaphore_after_quiescence()
                    .map_err(|result| {
                        io::Error::other(format!(
                            "rearm shared scanout export semaphore after failed handoff: {result:?}",
                        ))
                    })?;
                bo.state.reset_after_dependency_safe();
            }
            OutputScanout::Copied(pool) => {
                let source = pool
                    .sources
                    .get_mut(bo_idx)
                    .ok_or_else(|| io::Error::other("copied source index out of range"))?;
                source.recover_failed_cycle_after_renderer_quiescence()?;
                let destination = pool
                    .destinations
                    .bos
                    .get_mut(bo_idx)
                    .ok_or_else(|| io::Error::other("copied destination index out of range"))?;
                destination.state.reset_after_dependency_safe();
            }
        }
        Ok(())
    }

    /// Abandon a target that never reached `vkQueueSubmit2`. No semaphore
    /// payload or ownership release was executed, so only the display-pool
    /// reservation needs to be undone. Any prepared temporary import remains
    /// valid and may be reused by the next recording attempt.
    pub(crate) fn cancel_scanout_bo_recording(&mut self, output_idx: usize, bo_idx: usize) {
        self.debug_assert_scanout_pool_route(output_idx);
        let Some(bo) = self
            .scanout_pools
            .get_mut(output_idx)
            .and_then(Option::as_mut)
            .and_then(|pool| pool.display_pool_mut().bos.get_mut(bo_idx))
        else {
            return;
        };
        bo.state = BoState::default();
    }

    /// VT-switch suspend: force every scanout BO on every output back to
    /// `BoPhase::Free` and reset its content tracking.
    ///
    /// A pageflip submitted just before a VT switch never gets its
    /// page-flip-complete event once DRM master is lost, so its BO would
    /// stay stuck in `Pending`/`OnScreen` forever. Combined with the
    /// scene draining its `pending_acks`, the platform pool would then
    /// leak a BO per VT round until `acquire_scanout_bo` starves and the
    /// output wedges (observed: `tick skip reason=NoBO` after a few VT
    /// switches; also the `on_page_flip_complete: >1 pending BO` warning
    /// from stale Pending BOs). `drain_all_pending` device-wait-idles and
    /// transitions each BO to `Free`, closing any held dma-buf fences.
    ///
    /// Content is marked invalidated so the post-resume full-damage
    /// repaint does a full redraw rather than trusting a stale buffer-age
    /// generation. Safe to call while still master (no DRM ioctl here —
    /// only Vulkan idle + fence-fd close).
    pub(crate) fn reset_scanout_bos_for_suspend(&mut self) -> io::Result<()> {
        self.clear_scanout_render_completions();
        // Lifecycle callers reach this point only after every old output is
        // disabled and its DRM events are drained. Prepared rows can therefore
        // be discarded before the pool reset closes their BO-owned fences.
        self.unflip_batch_context = None;
        self.prepared_unflip_planes.clear();
        let mut first_error = None;
        for output_idx in 0..self.scanout_pools.len() {
            if let Err(error) = self.drain_scanout_pool_at(output_idx) {
                log::error!(
                    "suspend could not quiesce scanout output {output_idx}: {error}; \
                     copied resources remain quarantined"
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        for gens in &mut self.bo_generations {
            for g in gens {
                g.last_present_generation = None;
                g.content_invalidated = true;
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Quiesce one output's renderer and (for copied scanout) sink devices.
    /// Any failure makes the live renderer path unusable; the copied pool
    /// keeps uncertain resources quarantined and its Drop path leaks them
    /// instead of freeing memory that a GPU may still reference.
    pub(super) fn drain_scanout_pool_at(&mut self, output_idx: usize) -> io::Result<()> {
        let Some(vk) = self.gpu.vk.clone() else {
            return Ok(());
        };
        let result = self
            .scanout_pools
            .get_mut(output_idx)
            .and_then(Option::as_mut)
            .map_or(Ok(()), |pool| pool.drain_all_pending(&vk));
        if result.is_err() {
            self.gpu.renderer_failed = true;
        }
        result
    }

    /// Called by the SceneCompositor's tick after `present_scanout`
    /// returns Ok. Records that `bo_idx` is now pending the next
    /// page-flip-complete event for `output_idx`, and assigns the
    /// generation number for the in-flight frame.
    ///
    /// Returns the freshly-allocated generation.
    pub(super) fn record_present(&mut self, _output_idx: usize, _bo_idx: usize) -> u64 {
        self.next_present_generation = self
            .next_present_generation
            .checked_add(1)
            .expect("next_present_generation overflow");
        self.next_present_generation
    }

    /// SceneCompositor calls this on page-flip-complete after
    /// `on_page_flip_complete` to write the new
    /// `last_present_generation` and clear `content_invalidated`.
    pub(crate) fn commit_bo_present(&mut self, output_idx: usize, bo_idx: usize, generation: u64) {
        if let Some(gens) = self.bo_generations.get_mut(output_idx)
            && let Some(g) = gens.get_mut(bo_idx)
        {
            g.last_present_generation = Some(generation);
            g.content_invalidated = false;
        }
    }
}

/// Decide whether KMS bring-up may proceed, given how many outputs were
/// enumerated versus how many got a live scanout pool.
///
/// Refuse only when there is at least one connected output but *none* of
/// them can be driven (`output_count > 0 && live_pool_count == 0`) — that
/// is the "display attached but we can't put anything on it" case, which
/// otherwise manifests as a silent black screen (e.g. RPi 4/400 split-GPU
/// scanout with no shared modifier). Zero outputs is not a failure
/// (headless start; runtime hotplug may attach one later), and partial
/// success (some outputs live) proceeds on the ones that work.
pub(super) fn check_scanout_liveness(
    output_count: usize,
    live_pool_count: usize,
    errors: &[String],
) -> Result<(), String> {
    if output_count > 0 && live_pool_count == 0 {
        return Err(format!(
            "no displayable output: all {output_count} connected output(s) failed scanout \
             buffer allocation, so nothing can be shown. Per-output errors: [{}]",
            errors.join("; ")
        ));
    }
    Ok(())
}

/// pure boundary tests for grouped unflip readiness and dependency provenance.
#[cfg(test)]
mod pure_boundary_tests {
    use super::*;
    use crate::kms::backend::OutputKey;

    fn output_key(name: &str) -> OutputKey {
        OutputKey::new(
            crate::platform::drm::DrmDeviceKey {
                major: 226,
                minor: 0,
            },
            name,
        )
    }

    fn expected(
        output_idx: usize,
        output_key: OutputKey,
        bo_idx: usize,
        job: u64,
    ) -> ExpectedUnflipPlane {
        ExpectedUnflipPlane {
            output_idx,
            output_key,
            bo_idx,
            render_job_id: job,
            recording_complete: true,
            topology_generation: 31,
        }
    }

    fn prepared(
        expected: &ExpectedUnflipPlane,
        final_in_fence: Option<RawFd>,
    ) -> PreparedUnflipPlane {
        prepared_unflip_plane(
            expected.output_idx,
            expected.output_key.clone(),
            expected.bo_idx,
            expected.render_job_id,
            expected.recording_complete,
            expected.topology_generation,
            final_in_fence,
        )
    }

    // pure boundary. Mutation: accept the first ready output, ignore frame identity/generation,
    // or let an incomplete recording count as complete.
    #[test]
    fn grouped_unflip_readiness_requires_every_complete_current_output() {
        let keys = [output_key("DP-1"), output_key("HDMI-1")];
        let expected_rows = [
            expected(0, keys[0].clone(), 2, 51),
            expected(1, keys[1].clone(), 1, 52),
        ];
        let prepared_rows = [
            prepared(&expected_rows[0], Some(71)),
            prepared(&expected_rows[1], None),
        ];

        assert!(grouped_unflip_ready(&keys, &expected_rows, &prepared_rows));
        assert!(grouped_unflip_ready(
            &keys,
            &expected_rows,
            &[
                prepared(&expected_rows[0], None),
                prepared(&expected_rows[1], Some(83))
            ],
        ));
        assert!(!grouped_unflip_ready(
            &keys,
            &expected_rows[..1],
            &prepared_rows
        ));
        assert!(!grouped_unflip_ready(
            &keys,
            &expected_rows,
            &prepared_rows[..1]
        ));
        assert!(!grouped_unflip_ready(
            &keys,
            &[expected_rows[0].clone(), expected_rows[0].clone()],
            &prepared_rows,
        ));
        assert!(!grouped_unflip_ready(
            &keys,
            &expected_rows,
            &[prepared_rows[0].clone(), prepared_rows[0].clone()],
        ));

        let mut mismatches = Vec::new();
        let mut wrong_index = expected_rows[1].clone();
        wrong_index.output_idx = 0;
        mismatches.push((wrong_index, prepared_rows[1].clone()));
        let mut wrong_key = expected_rows[1].clone();
        wrong_key.output_key = keys[0].clone();
        mismatches.push((wrong_key, prepared_rows[1].clone()));
        let mut wrong_bo = expected_rows[1].clone();
        wrong_bo.bo_idx += 1;
        mismatches.push((expected_rows[1].clone(), prepared(&wrong_bo, None)));
        let mut wrong_job = expected_rows[1].clone();
        wrong_job.render_job_id += 1;
        mismatches.push((expected_rows[1].clone(), prepared(&wrong_job, None)));
        let mut incomplete_expected = expected_rows[1].clone();
        incomplete_expected.recording_complete = false;
        mismatches.push((incomplete_expected, prepared_rows[1].clone()));
        let mut incomplete_prepared = prepared_rows[1].clone();
        incomplete_prepared.recording_complete = false;
        mismatches.push((expected_rows[1].clone(), incomplete_prepared));
        let mut wrong_generation = expected_rows[1].clone();
        wrong_generation.topology_generation += 1;
        mismatches.push((expected_rows[1].clone(), prepared(&wrong_generation, None)));

        for (mutated_expected, mutated_prepared) in mismatches {
            let mut expected_rows = expected_rows.clone();
            let mut prepared_rows = prepared_rows.clone();
            expected_rows[1] = mutated_expected;
            prepared_rows[1] = mutated_prepared;
            assert!(!grouped_unflip_ready(&keys, &expected_rows, &prepared_rows));
        }

        let mut incomplete = expected_rows.clone();
        incomplete[1].recording_complete = false;
        assert!(!grouped_unflip_ready(&keys, &incomplete, &prepared_rows));
    }

    // pure boundary. Mutation: retain the source-render dependency instead of the final
    // sink-copy dependency on a prepared copied plane.
    #[test]
    fn prepared_copied_plane_retains_sink_copy_dependency() {
        let key = output_key("DP-1");
        let final_fd = crate::kms::vk::scanout::copied_final_dependency(Some(41), Some(73));
        let prepared = prepared_unflip_plane(0, key, 2, 9, true, 31, final_fd);
        assert_eq!(prepared.final_in_fence, Some(73));
    }
}
