//! `PlatformBackend` as the scene's output and cursor-plane sink: thin
//! wrappers over the scanout BO, page-flip, render-completion and cursor-plane
//! methods, plus the frame sequence of `finish_frame` around the scene's
//! record step and the scanout targets the scene composes into.

use crate::kms::{
    render::{
        gpu::GpuCore,
        platform::PlatformBackend,
        scene::{
            ComposeRenderTarget, CursorAcceptedMode, CursorCommitReceipt, CursorFramePlan,
            CursorPlaneSink, CursorPrimaryContent, FinishOutcome, FlipRetirement, FrameHandoff,
            FrameTargetError, OutputRoute, OutputSink, RecordFrame, RenderCompletion, SlotToken,
        },
    },
    vk::{
        compositor::PresentError,
        scanout::{BoPhase, BoState, CopiedRenderSource, OutputScanout, ScanoutBo},
    },
};
use ash::vk;
use std::{io, os::fd::OwnedFd, rc::Rc};

struct SharedScanoutAccepted {
    compose: crate::kms::render::scene::ComposeSubmit,
    event_id: u64,
    async_outcome: crate::drm::async_flip::AsyncOutcome,
    cursor_updates: Vec<super::cursor_state::CursorPlaneUpdate>,
    detach_retry_accepted: bool,
    cursorless_retry_accepted: bool,
    hardware_only_retry_accepted: bool,
    cursor_key_rejected: bool,
}

struct SharedScanoutRejected {
    error: PresentError,
    cursor_rejected: bool,
    cursor_key_rejected: bool,
    discard_software_frame: bool,
}

impl PlatformBackend {
    /// A cursor-attributed HW-only update whose cursorless primary retry was
    /// accepted leaves the applied binding unchanged. Carry the refusal into
    /// the ordinary cursor-only detach lane, including an unbound attach, and
    /// retain the key captured by the rejected request.
    pub(crate) fn queue_rejected_hardware_cursor_detach(
        &mut self,
        updates: &[super::cursor_state::CursorPlaneUpdate],
    ) {
        self.reject_cursor_plane_update_reservations(updates);
        for update in updates {
            if update.prepared.prior.is_none() && update.prepared.next.is_none() {
                continue;
            }
            let Some(device) = self
                .devices
                .iter_mut()
                .find(|device| device.key == update.device_key)
            else {
                continue;
            };
            device.cursor.note_rejected_cursor_key(
                update.crtc,
                update.topology_epoch,
                update.sprite_key,
            );
            let Some(state) = device.cursor.crtc_states.get_mut(&update.crtc) else {
                continue;
            };
            if state.applied != update.prepared.prior {
                continue;
            }
            state.fallback = super::cursor_state::CursorFallbackState::DetachPending {
                rejected_key: update.sprite_key,
            };
            state.intent = Some(super::cursor_state::CursorIntent::Hide);
            state.intent_scene_owned = false;
            state.intent_motion_timestamp = None;
            device
                .cursor
                .cursor_retry_waits_for_flip
                .remove(&update.crtc);
        }
    }

    fn reject_scene_cursor_show(&mut self, updates: &[super::cursor_state::CursorPlaneUpdate]) {
        self.reject_cursor_plane_update_reservations(updates);
        for update in updates {
            if update.prepared.prior.is_some() || update.prepared.next.is_none() {
                continue;
            }
            let Some(device) = self
                .devices
                .iter_mut()
                .find(|device| device.key == update.device_key)
            else {
                continue;
            };
            device.cursor.note_rejected_cursor_key(
                update.crtc,
                update.topology_epoch,
                update.sprite_key,
            );
            let fallback = super::cursor_recovery::software_for_rejected_sprite(update.sprite_key);
            device
                .cursor
                .cursor_retry_waits_for_flip
                .remove(&update.crtc);
            if let Some(state) = device.cursor.crtc_states.get_mut(&update.crtc) {
                let rejected_intent = super::cursor_state::CursorIntent::Show(
                    update
                        .prepared
                        .next
                        .expect("show update has a next binding"),
                );
                if state.intent == Some(rejected_intent) {
                    state.intent = None;
                    state.intent_scene_owned = false;
                    state.intent_motion_timestamp = None;
                }
                state.fallback = fallback;
            }
        }
    }

    fn accepted_cursor_mode(
        &self,
        output_idx: usize,
        primary_cursor: CursorPrimaryContent,
    ) -> CursorAcceptedMode {
        if self.cursor_plane().visible(output_idx) {
            CursorAcceptedMode::Hardware
        } else if matches!(primary_cursor, CursorPrimaryContent::Software { .. }) {
            CursorAcceptedMode::Software
        } else {
            CursorAcceptedMode::Hidden
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests_pending_scanout_render_completion_count(&self) -> usize {
        self.pending_scanout_render_completions.len()
    }

    #[cfg(test)]
    pub(crate) fn for_tests_pending_scanout_render_completion_ids(&self) -> Vec<u64> {
        self.pending_scanout_render_completions
            .iter()
            .map(|pending| pending.job_id)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn for_tests_pending_scanout_render_completion_associations(
        &self,
    ) -> Vec<(
        u64,
        super::CrtcKey,
        usize,
        crate::kms::render::scene::CursorFrameId,
    )> {
        self.pending_scanout_render_completions
            .iter()
            .map(|pending| {
                (
                    pending.job_id,
                    pending.crtc_key,
                    pending.bo_idx,
                    pending.cursor_frame,
                )
            })
            .collect()
    }
}

impl OutputSink for PlatformBackend {
    fn gpu(&self) -> &GpuCore {
        &self.gpu
    }

    fn gpu_mut(&mut self) -> &mut GpuCore {
        &mut self.gpu
    }

    fn cursor_plane(&self) -> &dyn CursorPlaneSink {
        self
    }

    fn cursor_plane_mut(&mut self) -> &mut dyn CursorPlaneSink {
        self
    }

    fn route(&self, output_idx: usize) -> OutputRoute {
        match self.scanout_pools.get(output_idx).and_then(Option::as_ref) {
            Some(OutputScanout::Shared(_)) => OutputRoute::Shared,
            Some(OutputScanout::Copied(_)) => OutputRoute::Copied,
            None => OutputRoute::None,
        }
    }

    fn output_commit_in_flight(&self, output_idx: usize) -> bool {
        self.cursor_commit_in_flight_for_output(output_idx)
    }

    fn slot_count(&self, output_idx: usize) -> Option<usize> {
        self.scanout_pools
            .get(output_idx)
            .and_then(|p| p.as_ref().map(|pool| pool.display_pool().bos.len()))
    }

    fn acquire_slot(&mut self, output_idx: usize) -> Option<SlotToken> {
        self.acquire_scanout_bo(output_idx)
    }

    fn slot_image(&self, output_idx: usize, bo_idx: usize) -> Option<vk::Image> {
        match self.scanout_pools.get(output_idx).and_then(|p| p.as_ref()) {
            Some(OutputScanout::Shared(pool)) => pool.bos.get(bo_idx).map(|bo| bo.vk_image),
            Some(OutputScanout::Copied(pool)) => {
                pool.sources.get(bo_idx).map(|source| source.image())
            }
            None => None,
        }
    }

    fn check_frame_target(&self, output_idx: usize) -> Result<(), FrameTargetError> {
        let output_key = &self.outputs[output_idx].key;
        if self.device_for_output(output_key).is_none() {
            return Err(FrameTargetError::NoDevice(io::Error::other(format!(
                "no DRM device for output {output_key:?}"
            ))));
        }
        if self
            .scanout_pools
            .get(output_idx)
            .and_then(Option::as_ref)
            .is_none()
        {
            return Err(FrameTargetError::NoSlot);
        }
        Ok(())
    }

    fn finish_frame(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
        cursor: CursorFramePlan,
        record: &mut RecordFrame<'_>,
    ) -> FinishOutcome {
        let output_key = &self.outputs[output_idx].key;
        let Some(drm_device) = self
            .device_for_output(output_key)
            .map(|device| Rc::clone(&device.device))
        else {
            return FinishOutcome::Unavailable(FrameTargetError::NoDevice(io::Error::other(
                format!("no DRM device for output {output_key:?}"),
            )));
        };
        let Some(atomic_commit_sink) = self
            .device_for_output(output_key)
            .map(|device| Rc::clone(&device.atomic_commit_sink))
        else {
            return FinishOutcome::Unavailable(FrameTargetError::NoDevice(io::Error::other(
                format!("no atomic commit sink for output {output_key:?}"),
            )));
        };
        let requested_timing = self
            .scene_flip_timings
            .get(output_idx)
            .copied()
            .flatten()
            .unwrap_or(self.requested_flip_timing);
        let timing = if self.cursor_plane_visible_for_output(output_idx)
            && matches!(
                cursor.assignment,
                crate::kms::render::scene::CursorFrameAssignment::Software { .. }
                    | crate::kms::render::scene::CursorFrameAssignment::Hidden
                    | crate::kms::render::scene::CursorFrameAssignment::CursorlessReplacement
            ) {
            crate::drm::async_flip::FlipTiming::Vsync
        } else {
            requested_timing
        };
        let target_slot_exists = self
            .scanout_pools
            .get(output_idx)
            .and_then(Option::as_ref)
            .is_some_and(|pool| match pool {
                OutputScanout::Shared(pool) => pool.bos.get(bo_idx).is_some(),
                OutputScanout::Copied(pool) => {
                    pool.sources.get(bo_idx).is_some()
                        && pool.destinations.bos.get(bo_idx).is_some()
                }
            });
        if !target_slot_exists {
            return FinishOutcome::Unavailable(FrameTargetError::NoSlot);
        }
        let frame = cursor.frame;
        let cursor_assignment = cursor.assignment;
        if let Err(error) =
            super::cursor_frames::stage_cursor_frame(self, output_idx, bo_idx, cursor)
        {
            return FinishOutcome::Unavailable(FrameTargetError::NoDevice(error));
        }
        // Async requests are cursorless by construction. Do not prepare or
        // reserve a cursor delta that an Async EINVAL fallback could later
        // accidentally settle as accepted.
        let scene_cursor_owner = timing == crate::drm::async_flip::FlipTiming::Vsync;
        let cursor_updates = if scene_cursor_owner {
            match super::cursor_frames::prepare_frame_cursor_commit(self, output_idx, bo_idx, frame)
            {
                Ok(updates) => updates,
                Err(error) => {
                    super::cursor_frames::cancel_cursor_frame(self, output_idx, bo_idx, frame);
                    return FinishOutcome::Unavailable(FrameTargetError::NoDevice(error));
                }
            }
        } else {
            Vec::new()
        };
        if let Err(error) = self.reserve_cursor_plane_updates(&cursor_updates) {
            super::cursor_frames::cancel_cursor_frame(self, output_idx, bo_idx, frame);
            return FinishOutcome::Unavailable(FrameTargetError::NoDevice(error));
        }
        let failure_context =
            super::cursor_recovery::scene_frame_failure_context(cursor_assignment, &cursor_updates);
        let detach_retry_updates =
            if failure_context == super::cursor_recovery::CursorFailureContext::SceneToSoftware {
                self.scene_detach_retry_updates(&cursor_updates)
            } else {
                Vec::new()
            };
        if let Err(error) = super::cursor_frames::store_cursor_frame_updates(
            self,
            output_idx,
            bo_idx,
            frame,
            cursor_updates.clone(),
            !cursor_updates.is_empty(),
        ) {
            self.reject_cursor_plane_update_reservations(&cursor_updates);
            super::cursor_frames::cancel_cursor_frame(self, output_idx, bo_idx, frame);
            return FinishOutcome::Unavailable(FrameTargetError::NoDevice(error));
        }
        let async_ledger = &mut self.async_ledger;
        let tearing_context = crate::drm::atomic::TearingCommitContext::composed(
            self.tearing_copy_received_at,
            self.tearing_copy_received_monotonic,
        );
        let pool = self
            .scanout_pools
            .get_mut(output_idx)
            .and_then(Option::as_mut)
            .expect("scanout pool was checked before cursor reservation");
        let output = &self.outputs[output_idx].output;
        let output_crtc = output.crtc;
        let mut shared_cursor_commit_submitted = false;
        let mut cursor_rejected = false;
        let mut cursor_key_rejected = false;
        let mut discarded_software_frame = false;
        let mut accepted_cursor_updates = cursor_updates.clone();
        let mut accepted_detach_retry = false;
        let mut cursorless_retry_accepted = false;
        let mut hardware_only_retry_accepted = false;
        let mut async_outcome = None;
        let mut outcome = match pool {
            OutputScanout::Shared(pool) => {
                let allocation_plan = pool.allocation_plan;
                let bo = pool
                    .bos
                    .get_mut(bo_idx)
                    .expect("shared scanout slot was checked before cursor reservation");
                let submitted = submit_shared_scanout_frame(
                    &drm_device,
                    atomic_commit_sink.as_ref(),
                    output,
                    allocation_plan,
                    bo,
                    record,
                    &cursor_updates,
                    &detach_retry_updates,
                    failure_context,
                    async_ledger,
                    timing,
                    tearing_context,
                );
                let result = match submitted {
                    Ok(accepted) => {
                        async_outcome = Some(accepted.async_outcome);
                        shared_cursor_commit_submitted = accepted.async_outcome
                            != crate::drm::async_flip::AsyncOutcome::Accepted;
                        accepted_cursor_updates = accepted.cursor_updates;
                        accepted_detach_retry = accepted.detach_retry_accepted;
                        cursorless_retry_accepted = accepted.cursorless_retry_accepted;
                        hardware_only_retry_accepted = accepted.hardware_only_retry_accepted;
                        cursor_key_rejected = accepted.cursor_key_rejected;
                        Ok(FrameHandoff::FlipPending {
                            cursor: CursorCommitReceipt {
                                frame: Some(frame),
                                mode: CursorAcceptedMode::Hidden,
                                primary_cursor: accepted.compose.primary_cursor,
                                needs_recompose: false,
                            },
                            submit: accepted.compose,
                            event_id: Some(accepted.event_id),
                        })
                    }
                    Err(rejected) => {
                        cursor_rejected = rejected.cursor_rejected;
                        cursor_key_rejected = rejected.cursor_key_rejected;
                        discarded_software_frame = rejected.discard_software_frame;
                        Err(rejected.error)
                    }
                };
                FinishOutcome::Finished {
                    result,
                    previous_gpu_ns: bo.last_gpu_render_ns.take(),
                    fail_stop: false,
                }
            }
            OutputScanout::Copied(pool) => {
                debug_assert!(timing == crate::drm::async_flip::FlipTiming::Vsync);
                let source = pool
                    .sources
                    .get_mut(bo_idx)
                    .expect("copied source slot was checked before cursor reservation");
                let destination = pool
                    .destinations
                    .bos
                    .get_mut(bo_idx)
                    .expect("copied destination was checked before cursor reservation");
                let result = submit_copied_scanout_render(source, &mut destination.state, record);
                let fail_stop = result
                    .as_ref()
                    .is_err_and(CopiedRenderSubmitError::requires_fail_stop);
                FinishOutcome::Finished {
                    result: result
                        .map(
                            |(completion, primary_cursor)| FrameHandoff::RenderCompletion {
                                completion,
                                primary_cursor,
                            },
                        )
                        .map_err(CopiedRenderSubmitError::into_present),
                    previous_gpu_ns: source.last_gpu_render_ns.take(),
                    fail_stop,
                }
            }
        };
        if cursor_key_rejected {
            self.note_rejected_cursor_qualifications(&cursor_updates);
        }
        if let Some(async_outcome) = async_outcome {
            self.async_outcomes.insert(
                crate::drm::async_flip::AsyncStream::Composed(output_crtc),
                async_outcome,
            );
            // The receipt belongs to this damage once any commit for it is
            // accepted. An EINVAL retry can successfully submit Vsync, and
            // later damage must not inherit the old frame's timestamp.
            if self.tearing_copy_received_at.is_some() {
                self.tearing_copy_received_at = None;
                self.tearing_copy_received_monotonic = None;
            }
        }
        let copied_handoff_waiting = matches!(
            &outcome,
            FinishOutcome::Finished {
                result: Ok(FrameHandoff::RenderCompletion { .. }),
                ..
            }
        );
        if cursorless_retry_accepted {
            if super::cursor_recovery::retry_requires_software_recompose(
                super::cursor_recovery::CursorRecoveryAction::RetryFrameWithoutCursor,
                failure_context,
            ) {
                self.reject_scene_cursor_show(&cursor_updates);
            } else {
                self.reject_cursor_plane_update_reservations(&cursor_updates);
                self.restore_failed_hardware_cursor_intents(&cursor_updates);
            }
        }
        if discarded_software_frame {
            self.reject_cursor_plane_update_reservations(&cursor_updates);
            let detach_error = match &outcome {
                FinishOutcome::Finished {
                    result: Err(PresentError::Io(error)),
                    ..
                } => Some(error.raw_os_error().map_or_else(
                    || io::Error::new(error.kind(), error.to_string()),
                    io::Error::from_raw_os_error,
                )),
                _ => None,
            };
            let device_idx = self
                .devices
                .iter()
                .position(|device| device.key == self.outputs[output_idx].key.device_key)
                .expect("scene output retains its DRM device");
            if let Some(error) = detach_error {
                self.retain_hardware_after_detach(
                    device_idx,
                    self.outputs[output_idx].output.crtc,
                    &error,
                );
            }
            self.mark_discarded_software_scene_frame(output_idx);
            super::cursor_frames::mark_cursor_frame_updates_settled(
                self, output_idx, bo_idx, frame,
            );
        } else if cursor_rejected {
            // A scene-owned cursor rejection abandons this frame with its old
            // primary and actual binding intact. Recovery may recompose; no
            // ownership transition is published from the rejected frame plan.
            self.reject_cursor_plane_update_reservations(&cursor_updates);
            self.restore_failed_hardware_cursor_intents(&cursor_updates);
            super::cursor_frames::mark_cursor_frame_updates_settled(
                self, output_idx, bo_idx, frame,
            );
        } else if shared_cursor_commit_submitted && scene_cursor_owner {
            if accepted_detach_retry {
                self.prepare_scene_detach_retry(&accepted_cursor_updates);
            }
            let settlement = if accepted_cursor_updates
                .iter()
                .any(|update| update.owner == super::cursor_frames::CursorCommitOwner::HardwareOnly)
            {
                self.cursor_plane_scene_commit_submitted(&accepted_cursor_updates)
            } else {
                self.cursor_plane_scene_frame_submitted(output_idx, frame, &accepted_cursor_updates)
            };
            if accepted_detach_retry {
                self.accept_scene_detach_retry(&accepted_cursor_updates);
            }
            if let Err(error) = settlement {
                // The atomic request already changed the display. Stop future
                // rendering and quarantine the sprite pool if even accepted-
                // state recovery cannot account for its framebuffer leases.
                log::error!("accepted shared scene frame could not settle cursor leases: {error}");
                self.quarantine_cursor_output(output_idx);
                self.gpu.renderer_failed = true;
            }
            super::cursor_frames::mark_cursor_frame_updates_settled(
                self, output_idx, bo_idx, frame,
            );
        } else if shared_cursor_commit_submitted {
            self.cursor_plane_scene_commit_submitted(&cursor_updates)
                .expect("accepted shared scene commit keeps its reserved cursor leases");
            super::cursor_frames::mark_cursor_frame_updates_settled(
                self, output_idx, bo_idx, frame,
            );
        } else if !copied_handoff_waiting {
            self.reject_cursor_plane_update_reservations(&cursor_updates);
            self.restore_failed_hardware_cursor_intents(&cursor_updates);
            super::cursor_frames::mark_cursor_frame_updates_settled(
                self, output_idx, bo_idx, frame,
            );
        }
        match &mut outcome {
            FinishOutcome::Finished {
                result:
                    Ok(FrameHandoff::FlipPending {
                        submit,
                        cursor: receipt,
                        ..
                    }),
                ..
            } => {
                let mode = self.accepted_cursor_mode(output_idx, submit.primary_cursor);
                let mut accepted = super::cursor_frames::accept_cursor_frame(
                    self,
                    output_idx,
                    bo_idx,
                    frame,
                    mode,
                    submit.primary_cursor,
                );
                if cursorless_retry_accepted
                    && super::cursor_recovery::retry_requires_software_recompose(
                        super::cursor_recovery::CursorRecoveryAction::RetryFrameWithoutCursor,
                        failure_context,
                    )
                {
                    accepted.mode = CursorAcceptedMode::Software;
                    accepted = accepted.with_recompose_after_acceptance();
                }
                *receipt = accepted;
            }
            FinishOutcome::Finished {
                result: Ok(FrameHandoff::RenderCompletion { primary_cursor, .. }),
                ..
            } => {
                if let Err(error) = super::cursor_frames::record_primary_cursor(
                    self,
                    output_idx,
                    bo_idx,
                    frame,
                    *primary_cursor,
                ) {
                    log::error!("render copied scanout: could not retain cursor frame: {error}");
                    super::cursor_frames::cancel_cursor_frame(self, output_idx, bo_idx, frame);
                }
            }
            FinishOutcome::Finished { result: Err(_), .. } => {
                self.release_scene_cursor_retry_after_failed_submit(&cursor_updates);
                super::cursor_frames::cancel_cursor_frame(self, output_idx, bo_idx, frame);
            }
            FinishOutcome::Unavailable(_) => {
                super::cursor_frames::cancel_cursor_frame(self, output_idx, bo_idx, frame);
            }
        }
        if hardware_only_retry_accepted {
            self.queue_rejected_hardware_cursor_detach(&cursor_updates);
        }
        outcome
    }

    fn invalidate_slot(&mut self, output_idx: usize, bo_idx: usize) {
        self.invalidate_bo(output_idx, bo_idx);
    }

    fn cancel_slot(&mut self, output_idx: usize, bo_idx: usize) {
        self.cancel_scanout_bo_recording(output_idx, bo_idx);
        super::cursor_frames::cancel_cursor_frame_for_slot(self, output_idx, bo_idx);
    }

    fn recycle_failed_slot(&mut self, output_idx: usize, bo_idx: usize) -> io::Result<()> {
        self.recycle_failed_submit_bo(output_idx, bo_idx)
    }

    fn on_page_flip_complete(&mut self, output_idx: usize) -> Option<FlipRetirement> {
        PlatformBackend::on_page_flip_complete(self, output_idx)
    }

    fn commit_slot_present(&mut self, output_idx: usize, bo_idx: usize, generation: u64) {
        self.commit_bo_present(output_idx, bo_idx, generation);
    }

    fn register_render_completion(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
        fd: Option<OwnedFd>,
    ) -> io::Result<u64> {
        let output_key = self.outputs[output_idx].key.clone();
        let Some(frame) = super::cursor_frames::staged_cursor_frame_id(self, output_idx, bo_idx)
        else {
            super::cursor_frames::cancel_cursor_frame_for_slot(self, output_idx, bo_idx);
            return Err(io::Error::other(
                "scanout completion has no staged cursor frame",
            ));
        };
        match self.register_scanout_render_completion(output_key, bo_idx, frame, fd) {
            Ok(job_id) => Ok(job_id),
            Err(error) => {
                super::cursor_frames::cancel_cursor_frame_for_slot(self, output_idx, bo_idx);
                Err(error)
            }
        }
    }

    fn drain_render_completions(&mut self) -> Vec<RenderCompletion> {
        self.drain_scanout_render_completions()
            .into_iter()
            .filter_map(|completion| {
                let output_idx = self
                    .outputs
                    .iter()
                    .position(|output| output.key == completion.output_key);
                let still_owned = output_idx.is_some_and(|output_idx| {
                    super::CrtcKey::for_output(&self.outputs[output_idx]) == completion.crtc_key
                        && super::cursor_frames::staged_cursor_frame_id(
                            self,
                            output_idx,
                            completion.bo_idx,
                        ) == Some(completion.cursor_frame)
                });
                if !still_owned {
                    if let Some(output_idx) = output_idx {
                        super::cursor_frames::cancel_cursor_frame(
                            self,
                            output_idx,
                            completion.bo_idx,
                            completion.cursor_frame,
                        );
                    }
                    return None;
                }
                Some(RenderCompletion {
                    job_id: completion.job_id,
                    output: Ok(output_idx.expect("owned completion has an output")),
                    bo_idx: completion.bo_idx,
                    fd: completion.fd,
                })
            })
            .collect()
    }

    fn clear_render_completions(&mut self) {
        self.clear_scanout_render_completions();
        super::cursor_frames::clear_cursor_frames(self);
    }

    fn submit_copied(
        &mut self,
        output_idx: usize,
        bo_idx: usize,
        fd: Option<OwnedFd>,
    ) -> io::Result<(CursorCommitReceipt, Option<u64>)> {
        let frame = super::cursor_frames::staged_cursor_frame_id(self, output_idx, bo_idx)
            .ok_or_else(|| io::Error::other("copied cursor frame was not staged"))?;
        let primary_cursor = self
            .staged_cursor_frames
            .get(&(output_idx, bo_idx))
            .and_then(|staged| staged.primary_cursor)
            .ok_or_else(|| io::Error::other("copied cursor frame has no recorded pixels"))?;
        let cursor_assignment = self
            .staged_cursor_frames
            .get(&(output_idx, bo_idx))
            .map(|staged| staged.plan.assignment)
            .ok_or_else(|| io::Error::other("copied cursor frame plan was not staged"))?;
        let mut cursor_updates = match super::cursor_frames::cursor_frame_updates_for_submit(
            self, output_idx, bo_idx, frame,
        ) {
            Ok(updates) => updates,
            Err(error) => {
                super::cursor_frames::cancel_cursor_frame(self, output_idx, bo_idx, frame);
                return Err(error);
            }
        };
        let failure_context =
            super::cursor_recovery::scene_frame_failure_context(cursor_assignment, &cursor_updates);
        match self.submit_copied_scanout(
            output_idx,
            bo_idx,
            fd,
            &mut cursor_updates,
            failure_context,
        ) {
            Ok((recovery, event_id)) => {
                let cursorless_retry_accepted =
                    recovery == super::scanout_bos::CopiedScanoutRecovery::CursorlessRetry;
                let hardware_only_retry_accepted =
                    recovery == super::scanout_bos::CopiedScanoutRecovery::HardwareOnlyRetry;
                let rejected_hardware_updates =
                    hardware_only_retry_accepted.then(|| cursor_updates.clone());
                let failed_show_retry = cursorless_retry_accepted
                    && super::cursor_recovery::retry_requires_software_recompose(
                        super::cursor_recovery::CursorRecoveryAction::RetryFrameWithoutCursor,
                        failure_context,
                    );
                if cursorless_retry_accepted {
                    if failed_show_retry {
                        self.reject_scene_cursor_show(&cursor_updates);
                    } else {
                        self.reject_cursor_plane_update_reservations(&cursor_updates);
                        self.restore_failed_hardware_cursor_intents(&cursor_updates);
                    }
                    cursor_updates.clear();
                } else if hardware_only_retry_accepted {
                    cursor_updates.clear();
                }
                let settlement = if cursor_updates.iter().any(|update| {
                    update.owner == super::cursor_frames::CursorCommitOwner::HardwareOnly
                }) {
                    self.cursor_plane_scene_commit_submitted(&cursor_updates)
                } else {
                    self.cursor_plane_scene_frame_submitted(output_idx, frame, &cursor_updates)
                };
                if let Err(error) = settlement {
                    log::error!(
                        "accepted copied scene frame could not settle cursor leases: {error}"
                    );
                    self.quarantine_cursor_output(output_idx);
                    self.gpu.renderer_failed = true;
                    super::cursor_frames::mark_cursor_frame_updates_settled(
                        self, output_idx, bo_idx, frame,
                    );
                    super::cursor_frames::cancel_cursor_frame(self, output_idx, bo_idx, frame);
                    return Err(error);
                }
                if hardware_only_retry_accepted {
                    self.queue_rejected_hardware_cursor_detach(
                        rejected_hardware_updates
                            .as_deref()
                            .expect("hardware retry retains the rejected cursor updates"),
                    );
                }
                if recovery == super::scanout_bos::CopiedScanoutRecovery::DetachRetry {
                    self.accept_scene_detach_retry(&cursor_updates);
                }
                super::cursor_frames::mark_cursor_frame_updates_settled(
                    self, output_idx, bo_idx, frame,
                );
                let mode = if failed_show_retry {
                    CursorAcceptedMode::Software
                } else {
                    self.accepted_cursor_mode(output_idx, primary_cursor)
                };
                let mut receipt = super::cursor_frames::accept_cursor_frame(
                    self,
                    output_idx,
                    bo_idx,
                    frame,
                    mode,
                    primary_cursor,
                );
                if failed_show_retry {
                    receipt = receipt.with_recompose_after_acceptance();
                }
                Ok((receipt, Some(event_id)))
            }
            Err(submit_error) => {
                self.release_scene_cursor_retry_after_failed_submit(&cursor_updates);
                if submit_error.discard_software_frame {
                    super::cursor_frames::mark_cursor_frame_updates_settled(
                        self, output_idx, bo_idx, frame,
                    );
                } else if submit_error.cursor_rejected {
                    let outcome =
                        self.reject_atomic_cursor_commit(&cursor_updates, &submit_error.error);
                    self.defer_cursor_move_outcome(outcome);
                    super::cursor_frames::mark_cursor_frame_updates_settled(
                        self, output_idx, bo_idx, frame,
                    );
                }
                self.restore_failed_hardware_cursor_intents(&cursor_updates);
                super::cursor_frames::cancel_cursor_frame(self, output_idx, bo_idx, frame);
                Err(submit_error.error)
            }
        }
    }
}

impl PlatformBackend {
    /// Select the timing used by the shared scene-flip route for this output.
    /// `maybe_composite` resets every output to Vsync before making any
    /// admission decision, so an earlier pass cannot leave Async behind.
    pub(crate) fn set_scene_flip_timing(
        &mut self,
        output_idx: usize,
        timing: crate::drm::async_flip::FlipTiming,
    ) {
        if self.scene_flip_timings.len() <= output_idx {
            self.scene_flip_timings.resize(output_idx + 1, None);
        }
        self.scene_flip_timings[output_idx] = Some(timing);
    }

    /// Return the exact shape used by the shared composed-flip submission
    /// path. Copied scanout never requests Async.
    pub(crate) fn scene_async_shape(
        &self,
        output_idx: usize,
    ) -> Option<crate::drm::async_flip::AsyncShape> {
        let output = &self.outputs.get(output_idx)?.output;
        let Some(Some(OutputScanout::Shared(pool))) = self.scanout_pools.get(output_idx) else {
            return None;
        };
        let bo = pool.bos.first()?;
        Some(composed_async_shape(
            output,
            pool.allocation_plan,
            bo.pitch,
            bo.width,
            bo.height,
        ))
    }
}

fn composed_async_shape(
    output: &crate::drm::modeset::Output,
    allocation_plan: crate::kms::vk::scanout::ScanoutAllocationPlan,
    pitch: u32,
    width: u32,
    height: u32,
) -> crate::drm::async_flip::AsyncShape {
    const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
    let modifier = match allocation_plan {
        crate::kms::vk::scanout::ScanoutAllocationPlan::GbmModifier(modifier)
        | crate::kms::vk::scanout::ScanoutAllocationPlan::DrmModifier(modifier) => modifier,
        crate::kms::vk::scanout::ScanoutAllocationPlan::PaddedExplicitLinear { .. }
        | crate::kms::vk::scanout::ScanoutAllocationPlan::ExplicitLinear
        | crate::kms::vk::scanout::ScanoutAllocationPlan::LegacyLinear => 0,
    };
    crate::drm::async_flip::AsyncShape {
        fourcc: DRM_FORMAT_XRGB8888,
        modifier,
        pitch,
        width,
        height,
        crtcs: vec![output.crtc],
        direct: false,
    }
}

impl CursorPlaneSink for PlatformBackend {
    fn fits(&self, output_idx: usize, width: u32, height: u32) -> bool {
        self.cursor_plane_fits_for_output(output_idx, width, height)
    }

    fn visible(&self, output_idx: usize) -> bool {
        self.cursor_plane_visible_for_output(output_idx)
    }

    fn software_cursor_suppressed(&self, output_idx: usize) -> bool {
        self.software_cursor_suppressed_for_output(output_idx)
    }

    fn take_discarded_software_frame(&mut self, output_idx: usize) -> bool {
        self.take_discarded_software_scene_frame(output_idx)
    }

    fn uploaded_image_matches(
        &self,
        output_idx: usize,
        version: u64,
        hot_x: u16,
        hot_y: u16,
    ) -> bool {
        self.cursor_plane_uploaded_image_matches_for_output(output_idx, version, hot_x, hot_y)
    }

    fn note_composed_retirement(&mut self, _output_idx: usize) -> bool {
        false
    }
}

/// A copied frame's failure, before or after the record.
enum CopiedRenderSubmitError {
    RendererAcquire(io::Error),
    Present(PresentError),
}

impl CopiedRenderSubmitError {
    fn requires_fail_stop(&self) -> bool {
        matches!(self, Self::RendererAcquire(_))
    }

    fn into_present(self) -> PresentError {
        match self {
            Self::RendererAcquire(error) => PresentError::Io(error),
            Self::Present(error) => error,
        }
    }
}

impl ComposeRenderTarget for ScanoutBo {
    fn image(&self) -> vk::Image {
        self.vk_image
    }

    fn image_view(&self) -> vk::ImageView {
        self.vk_image_view
    }

    fn command_buffer(&self) -> vk::CommandBuffer {
        self.vk_transfer.command_buffer
    }

    fn completion_semaphore(&self) -> vk::Semaphore {
        self.vk_semaphore
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn timestamp_pool(&self) -> vk::QueryPool {
        self.vk_transfer.timestamp_pool
    }

    fn timestamps_written(&self) -> bool {
        self.vk_transfer.timestamps_written
    }

    fn mark_timestamps_written(&mut self) {
        self.vk_transfer.timestamps_written = true;
    }

    fn set_last_gpu_render_ns(&mut self, value: Option<u64>) {
        self.last_gpu_render_ns = value;
    }

    fn prepare_post_compose(&mut self) -> Result<(), PresentError> {
        self.compose_prepare_error
            .as_ref()
            .and_then(|script| script.borrow_mut().take_error_if_due())
            .map_or(Ok(()), |error| Err(PresentError::Vk(error)))
    }

    fn record_post_compose(
        &mut self,
        vk: &crate::kms::vk::device::VkContext,
        command_buffer: vk::CommandBuffer,
    ) {
        let to_scanout = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .dst_access_mask(vk::AccessFlags2::empty())
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .image(self.vk_image)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            )];
        crate::vk_count!(cmd_pipeline_barrier2);
        unsafe {
            vk.device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().image_memory_barriers(&to_scanout),
            );
        }
    }
}

impl ComposeRenderTarget for CopiedRenderSource {
    fn image(&self) -> vk::Image {
        self.image()
    }

    fn image_view(&self) -> vk::ImageView {
        self.image_view()
    }

    fn command_buffer(&self) -> vk::CommandBuffer {
        self.transfer.command_buffer
    }

    fn completion_semaphore(&self) -> vk::Semaphore {
        self.completion_semaphore
    }

    fn width(&self) -> u32 {
        self.width()
    }

    fn height(&self) -> u32 {
        self.height()
    }

    fn timestamp_pool(&self) -> vk::QueryPool {
        self.transfer.timestamp_pool
    }

    fn timestamps_written(&self) -> bool {
        self.transfer.timestamps_written
    }

    fn mark_timestamps_written(&mut self) {
        self.transfer.timestamps_written = true;
    }

    fn set_last_gpu_render_ns(&mut self, value: Option<u64>) {
        self.last_gpu_render_ns = value;
    }

    fn prepare_post_compose(&mut self) -> Result<(), PresentError> {
        let preparation = self.transport_preparation().map_err(PresentError::Io)?;
        self.post_compose_preparation = Some(preparation);
        Ok(())
    }

    fn renderer_wait_semaphore(&self) -> Option<vk::Semaphore> {
        self.renderer_wait_semaphore()
    }

    fn note_submit_succeeded(&mut self) {
        self.note_renderer_submit_succeeded();
    }

    fn record_post_compose(
        &mut self,
        _vk: &crate::kms::vk::device::VkContext,
        command_buffer: vk::CommandBuffer,
    ) {
        let Some(preparation) = self.post_compose_preparation.take() else {
            unreachable!("copied target recorded post-compose without a preparation")
        };
        self.record_transport_copy(command_buffer, preparation);
    }
}

/// Render directly into the KMS framebuffer and immediately queue its flip.
fn submit_shared_scanout_frame(
    drm: &crate::drm::Device,
    atomic_commit_sink: &crate::drm::atomic::AtomicCommitSink,
    output: &crate::platform::drm::Output,
    allocation_plan: crate::kms::vk::scanout::ScanoutAllocationPlan,
    bo: &mut ScanoutBo,
    record: &mut RecordFrame<'_>,
    cursor_updates: &[crate::kms::render::platform::cursor_state::CursorPlaneUpdate],
    detach_retry_updates: &[crate::kms::render::platform::cursor_state::CursorPlaneUpdate],
    failure_context: super::cursor_recovery::CursorFailureContext,
    async_ledger: &mut crate::drm::async_flip::AsyncFlipLedger,
    timing: crate::drm::async_flip::FlipTiming,
    tearing_context: crate::drm::atomic::TearingCommitContext,
) -> Result<SharedScanoutAccepted, SharedScanoutRejected> {
    use std::os::fd::{FromRawFd, IntoRawFd};

    if bo.state.phase != BoPhase::Free {
        return Err(SharedScanoutRejected {
            error: PresentError::WrongPhase(bo.state.phase),
            cursor_rejected: false,
            cursor_key_rejected: false,
            discard_software_frame: false,
        });
    }
    let Some(fb_handle) = bo.fb_handle else {
        return Err(SharedScanoutRejected {
            error: PresentError::NoFb,
            cursor_rejected: false,
            cursor_key_rejected: false,
            discard_software_frame: false,
        });
    };
    bo.state.transition_to_recording();
    let submitted = record(bo).map_err(|error| SharedScanoutRejected {
        error,
        cursor_rejected: false,
        cursor_key_rejected: false,
        discard_software_frame: false,
    })?;

    let fd = bo
        .export_signaled_fd()
        .map_err(|error| SharedScanoutRejected {
            error: PresentError::Vk(error),
            cursor_rejected: false,
            cursor_key_rejected: false,
            discard_software_frame: false,
        })?
        .map_or(-1, IntoRawFd::into_raw_fd);
    bo.state.transition_to_submitted(fd);

    let shape = composed_async_shape(output, allocation_plan, bo.pitch, bo.width, bo.height);
    let stream = crate::drm::async_flip::AsyncStream::Composed(output.crtc);
    if timing == crate::drm::async_flip::FlipTiming::Async {
        async_ledger.observe_shape(stream, &shape);
    }
    let attempt_timing = if timing == crate::drm::async_flip::FlipTiming::Async
        && async_ledger.allows(stream, &shape)
    {
        crate::drm::async_flip::FlipTiming::Async
    } else {
        crate::drm::async_flip::FlipTiming::Vsync
    };
    let mut out_fence: i32 = -1;
    let mut submit_result = crate::drm::page_flip::submit_scene_flip_with_fences_tearing(
        drm,
        atomic_commit_sink,
        output,
        fb_handle,
        fd,
        &mut out_fence,
        cursor_updates,
        attempt_timing,
        tearing_context,
    );
    let async_outcome = if attempt_timing == crate::drm::async_flip::FlipTiming::Async {
        match submit_result {
            Ok(event_id) => {
                async_ledger.note_accepted(stream, &shape);
                submit_result = Ok(event_id);
                crate::drm::async_flip::AsyncOutcome::Accepted
            }
            Err(error) if error.raw_os_error() == Some(libc::EINVAL) => {
                let suspended = async_ledger.note_einval(stream, &shape);
                let retry_fd = bo
                    .state
                    .transition_to_recording_after_atomic_reject()
                    .unwrap_or(-1);
                bo.state.transition_to_submitted(retry_fd);
                out_fence = -1;
                submit_result = crate::drm::page_flip::submit_scene_flip_with_fences_tearing(
                    drm,
                    atomic_commit_sink,
                    output,
                    fb_handle,
                    retry_fd,
                    &mut out_fence,
                    cursor_updates,
                    crate::drm::async_flip::FlipTiming::Vsync,
                    crate::drm::atomic::TearingCommitContext {
                        async_fallback: true,
                        ..tearing_context
                    },
                );
                crate::drm::async_flip::AsyncOutcome::FellBack { suspended }
            }
            Err(error) => {
                submit_result = Err(error);
                crate::drm::async_flip::AsyncOutcome::NotRequested
            }
        }
    } else {
        crate::drm::async_flip::AsyncOutcome::NotRequested
    };

    let initial_action = submit_result.as_ref().err().map(|error| {
        super::cursor_recovery::classify_cursor_failure(
            error,
            atomic_commit_sink.take_last_cursor_rejection(),
            failure_context,
        )
    });
    let cursor_key_rejected = initial_action.is_some_and(|action| action.is_cursor_failure());
    let mut accepted_cursor_updates = cursor_updates.to_vec();
    let mut detach_retry_accepted = false;
    let mut cursorless_retry_accepted = false;
    let mut hardware_only_retry_accepted = false;
    let mut cursor_rejected = initial_action.is_some_and(|action| action.is_cursor_failure());
    let mut discard_software_frame = false;
    if initial_action == Some(super::cursor_recovery::CursorRecoveryAction::RetryFrameWithDetach)
        && !detach_retry_updates.is_empty()
    {
        let retry_fd = bo
            .state
            .transition_to_recording_after_atomic_reject()
            .unwrap_or(-1);
        bo.state.transition_to_submitted(retry_fd);
        out_fence = -1;
        submit_result = crate::drm::page_flip::submit_scene_flip_with_fences_tearing(
            drm,
            atomic_commit_sink,
            output,
            fb_handle,
            retry_fd,
            &mut out_fence,
            detach_retry_updates,
            crate::drm::async_flip::FlipTiming::Vsync,
            tearing_context,
        );
        match &submit_result {
            Ok(_) => {
                accepted_cursor_updates = detach_retry_updates.to_vec();
                detach_retry_accepted = true;
                cursor_rejected = false;
            }
            Err(error) => {
                let retry_action = super::cursor_recovery::classify_cursor_failure(
                    error,
                    atomic_commit_sink.take_last_cursor_rejection(),
                    super::cursor_recovery::scene_failure_context(detach_retry_updates),
                );
                cursor_rejected = false;
                discard_software_frame = retry_action.is_cursor_failure();
            }
        }
    }
    let hardware_only_retry = initial_action.is_some_and(|action| {
        super::cursor_recovery::retry_riding_frame_without_cursor(
            action,
            failure_context,
            cursor_updates,
        )
    });
    let software_recompose_retry = !hardware_only_retry
        && initial_action.is_some_and(|action| {
            super::cursor_recovery::retry_requires_software_recompose(action, failure_context)
        });
    if hardware_only_retry || software_recompose_retry {
        let retry_fd = bo
            .state
            .transition_to_recording_after_atomic_reject()
            .unwrap_or(-1);
        bo.state.transition_to_submitted(retry_fd);
        out_fence = -1;
        submit_result = crate::drm::page_flip::submit_scene_flip_with_fences_tearing(
            drm,
            atomic_commit_sink,
            output,
            fb_handle,
            retry_fd,
            &mut out_fence,
            &[],
            crate::drm::async_flip::FlipTiming::Vsync,
            tearing_context,
        );
        match &submit_result {
            Ok(_) => {
                accepted_cursor_updates.clear();
                hardware_only_retry_accepted = hardware_only_retry;
                cursorless_retry_accepted = software_recompose_retry;
                cursor_rejected = false;
            }
            Err(_) => {
                // The retry contains only the original primary, so its
                // failure belongs to the ordinary flip error path.
                cursor_rejected = false;
            }
        }
    }
    match submit_result {
        Ok(event_id) => {
            if let Some(reclaimed) = bo.state.transition_to_pending(out_fence) {
                // SAFETY: this is the in-fence fd retained across any async
                // EINVAL retry and returned after atomic acceptance.
                drop(unsafe { std::os::fd::OwnedFd::from_raw_fd(reclaimed) });
            }
            Ok(SharedScanoutAccepted {
                compose: submitted,
                event_id,
                async_outcome,
                cursor_updates: accepted_cursor_updates,
                detach_retry_accepted,
                cursorless_retry_accepted,
                hardware_only_retry_accepted,
                cursor_key_rejected,
            })
        }
        Err(error) => {
            if let Some(reclaimed) = bo.state.transition_to_recording_after_atomic_reject() {
                // SAFETY: same fd inserted by `transition_to_submitted`.
                drop(unsafe { std::os::fd::OwnedFd::from_raw_fd(reclaimed) });
            }
            if out_fence >= 0 {
                // Defensive: OUT_FENCE_PTR should only be written on success.
                drop(unsafe { std::os::fd::OwnedFd::from_raw_fd(out_fence) });
            }
            Err(SharedScanoutRejected {
                error: PresentError::Io(error),
                cursor_rejected,
                cursor_key_rejected,
                discard_software_frame,
            })
        }
    }
}

/// Render into A's exportable source. The paired destination phase reserves
/// the same BO index until readiness advances the frame to B's copy + KMS
/// submission on the main-loop boundary.
fn submit_copied_scanout_render(
    source: &mut CopiedRenderSource,
    destination_state: &mut BoState,
    record: &mut RecordFrame<'_>,
) -> Result<(Option<OwnedFd>, CursorPrimaryContent), CopiedRenderSubmitError> {
    if destination_state.phase != BoPhase::Free {
        return Err(CopiedRenderSubmitError::Present(PresentError::WrongPhase(
            destination_state.phase,
        )));
    }
    source
        .prepare_renderer_acquire()
        .map_err(CopiedRenderSubmitError::RendererAcquire)?;
    destination_state.transition_to_recording();
    let primary_cursor = record(source)
        .map_err(CopiedRenderSubmitError::Present)?
        .primary_cursor;
    let completion = source
        .export_render_completion()
        .map_err(|error| CopiedRenderSubmitError::Present(PresentError::Vk(error)))?;
    Ok((completion, primary_cursor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_renderer_acquire_failure_is_fail_stop_before_submit() {
        let acquire = CopiedRenderSubmitError::RendererAcquire(io::Error::other(
            "retained B-to-A completion import failed",
        ));
        assert!(acquire.requires_fail_stop());
        assert!(matches!(acquire.into_present(), PresentError::Io(_)));

        let ordinary = CopiedRenderSubmitError::Present(PresentError::Vk(
            vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
        ));
        assert!(!ordinary.requires_fail_stop());
    }
}
