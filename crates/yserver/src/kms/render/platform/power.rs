use crate::{drm, kms::render::platform::PlatformBackend};
use std::{io, rc::Rc};

pub(in crate::kms::render) trait OutputPowerExecutor {
    fn fallback_modeset_framebuffer(&self) -> Option<::drm::control::framebuffer::Handle> {
        None
    }

    fn disable(
        &self,
        _device: &drm::Device,
        output: &crate::platform::drm::Output,
        cursor_detach: Option<crate::drm::atomic::AtomicCursorPlaneDetach>,
        services: &super::services::KmsServices,
    ) -> io::Result<()>;

    fn enable(
        &self,
        device: &drm::Device,
        output: &crate::platform::drm::Output,
        framebuffer: ::drm::control::framebuffer::Handle,
        services: &super::services::KmsServices,
    ) -> io::Result<()>;
}

pub(super) struct DrmOutputPowerExecutor;

impl OutputPowerExecutor for DrmOutputPowerExecutor {
    fn disable(
        &self,
        device: &drm::Device,
        output: &crate::platform::drm::Output,
        cursor_detach: Option<crate::drm::atomic::AtomicCursorPlaneDetach>,
        services: &super::services::KmsServices,
    ) -> io::Result<()> {
        drm::modeset::disable_output_with_cursor_executor(
            device,
            output,
            cursor_detach,
            services.atomic.as_ref(),
        )
    }

    fn enable(
        &self,
        device: &drm::Device,
        output: &crate::platform::drm::Output,
        framebuffer: ::drm::control::framebuffer::Handle,
        services: &super::services::KmsServices,
    ) -> io::Result<()> {
        drm::modeset::commit_modeset_with_io(
            device,
            output,
            framebuffer,
            services.mode_blobs.as_ref(),
            services.atomic.as_ref(),
        )
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OutputPowerRecord {
    pub(crate) enabled: bool,
    pub(crate) connector: ::drm::control::connector::Handle,
    pub(crate) crtc: ::drm::control::crtc::Handle,
    pub(crate) framebuffer: Option<::drm::control::framebuffer::Handle>,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingOutputPowerExecutor {
    records: std::cell::RefCell<Vec<OutputPowerRecord>>,
}

#[cfg(test)]
impl RecordingOutputPowerExecutor {
    pub(crate) fn records(&self) -> Vec<OutputPowerRecord> {
        self.records.borrow().clone()
    }
}

#[cfg(test)]
impl OutputPowerExecutor for RecordingOutputPowerExecutor {
    fn fallback_modeset_framebuffer(&self) -> Option<::drm::control::framebuffer::Handle> {
        Some(::drm::control::from_u32(0xF001).expect("synthetic test framebuffer"))
    }

    fn disable(
        &self,
        _device: &drm::Device,
        output: &crate::platform::drm::Output,
        cursor_detach: Option<crate::drm::atomic::AtomicCursorPlaneDetach>,
        services: &super::services::KmsServices,
    ) -> io::Result<()> {
        self.records.borrow_mut().push(OutputPowerRecord {
            enabled: false,
            connector: output.connector,
            crtc: output.crtc,
            framebuffer: None,
        });
        drm::modeset::disable_output_with_properties(
            _device,
            output,
            cursor_detach,
            drm::modeset::DisableOutputPropertyHandles::for_tests(),
            services.atomic.as_ref(),
        )
    }

    fn enable(
        &self,
        _device: &drm::Device,
        output: &crate::platform::drm::Output,
        framebuffer: ::drm::control::framebuffer::Handle,
        services: &super::services::KmsServices,
    ) -> io::Result<()> {
        self.records.borrow_mut().push(OutputPowerRecord {
            enabled: true,
            connector: output.connector,
            crtc: output.crtc,
            framebuffer: Some(framebuffer),
        });
        drm::modeset::commit_modeset_with_io(
            _device,
            output,
            framebuffer,
            services.mode_blobs.as_ref(),
            services.atomic.as_ref(),
        )
    }
}

impl PlatformBackend {
    // ── Disable output ──────────────────────────────────────────

    /// Best-effort wait for all in-flight GPU work to complete, bounded
    /// to 5 seconds (matching the `FenceTicket::wait` / `device_wait_idle`
    /// convention used at shutdown). Called by `KmsBackend::run_suspend`
    /// before DRM master is dropped, so in-flight submits don't race a
    /// kernel-side scanout teardown.
    ///
    /// Errors from `device_wait_idle` are logged and swallowed: the VT
    /// release path must always continue even if the wait times out or the
    /// device is already lost.
    pub(crate) fn wait_idle_bounded(&self) {
        // `device_wait_idle` is inherently blocking; 5 s is the same bound
        // used by FenceTicket::wait in the pool destructor.  We do not set a
        // real timeout here because ash's `device_wait_idle` wraps
        // `vkDeviceWaitIdle` which has no timeout parameter — on a lost
        // device it returns VK_ERROR_DEVICE_LOST promptly.  The 5-second
        // comment in the plan refers to the *practical* upper bound the
        // driver enforces on a wedged device; real quiescence is typically
        // sub-millisecond.
        if let Some(vk) = self.gpu.vk.as_ref() {
            let result = unsafe { vk.device.device_wait_idle() };
            if let Err(e) = result {
                log::warn!("kms: wait_idle_bounded: device_wait_idle failed: {e:?}");
            }
        }
        for (renderer_id, vk) in &self.copy_vk_contexts {
            let result = unsafe { vk.device.device_wait_idle() };
            if let Err(e) = result {
                log::warn!(
                    "kms: wait_idle_bounded: copied sink {renderer_id:?} device_wait_idle failed: {e:?}"
                );
            }
        }
    }

    /// Post-loop teardown — disable each output, leaving the
    /// scanout BOs in a state where their Drop can clean up
    /// (or, on atomic disable failure, disarm them so we leak
    /// rather than confuse KMS — same shape as v1).
    ///
    /// # Errors
    ///
    /// Propagates the first per-output `disable_output` failure;
    /// subsequent outputs still attempted.
    pub(crate) fn disable_output(&mut self) -> io::Result<()> {
        self.shutting_down = true;
        self.clear_scanout_render_completions();

        // Best-effort: drain both the selected renderer and every copied
        // sink transfer device before pulling the modeset.
        self.wait_idle_bounded();

        // Stage 3f.10: drain the pixmap pool so the recycled
        // image/memory/view triples don't leak through the
        // VkContext destruction path. Safe to drain here: every
        // in-flight CB has been waited on by device_wait_idle.
        if let Some(pool) = self.gpu.pixmap_pool.as_ref() {
            pool.drain();
        }

        let mut first_err: Option<io::Error> = None;
        for i in 0..self.outputs.len() {
            let output_key = self.outputs[i].key.clone();
            let connector_name = self.outputs[i].output.connector_name.clone();
            let Some(device) = self
                .device_for_output(&output_key)
                .map(|device| Rc::clone(&device.device))
            else {
                log::warn!(
                    "render disable_output: no DRM device {} for {}",
                    output_key.device_key,
                    connector_name,
                );
                continue;
            };
            let cursor_detach = self.cursor_plane_detach_for_output(i);
            if let Err(e) = self.output_power_executor.disable(
                &device,
                &self.outputs[i].output,
                cursor_detach,
                &self.services,
            ) {
                log::warn!(
                    "render disable_output: failed for {} (output {i}): {e}",
                    connector_name,
                );
                // Disarm the matching scanout pool so its Drop
                // doesn't try to destroy framebuffers KMS may
                // still hold (matches v1's behaviour).
                if let Some(pool) = self.scanout_pools.get_mut(i).and_then(|p| p.as_mut()) {
                    pool.disarm();
                }
                self.quarantine_cursor_output(i);
                if first_err.is_none() {
                    first_err = Some(e);
                }
            } else {
                self.accept_cursor_lifecycle_detach(i);
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Drive every output to a binary KMS power state. Used by
    /// DPMS — collapses Standby/Suspend/Off to "outputs inactive"
    /// and On to "outputs active". Unlike the post-loop
    /// `disable_output` it does NOT set `shutting_down`, NOT call
    /// `device_wait_idle`, and NOT disarm scanout pools — DPMS is
    /// reversible.
    ///
    /// # Errors
    ///
    /// Collects the first per-output failure, continues with the
    /// rest, then returns it. The caller (KmsBackend::set_dpms_power)
    /// logs and advances the in-memory DPMS state regardless.
    pub(crate) fn dpms_set_outputs_active(&mut self, active: bool) -> io::Result<()> {
        let mut first_err: Option<io::Error> = None;
        if active {
            let mut modeset_crtcs = Vec::new();
            // Re-commit modeset. Pick the OnScreen BO (last frame
            // before blank) or any registered fb — same selection
            // logic as `requery_outputs_and_modeset` at :2030.
            for (i, layout) in self.outputs.iter().enumerate() {
                let cursor_key = crate::kms::render::platform::CrtcKey::for_output(layout);
                let Some(device) = self
                    .device_for_output(&layout.key)
                    .map(|device| Rc::clone(&device.device))
                else {
                    self.active_cursor_crtcs.remove(&cursor_key);
                    let error = io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "dpms_set_outputs_active(true): no DRM device {} for {}",
                            layout.key.device_key, layout.output.connector_name,
                        ),
                    );
                    log::error!("{error}");
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                    continue;
                };
                let front =
                    self.scanout_pools
                        .get(i)
                        .and_then(|p| p.as_ref())
                        .and_then(|pool| {
                            use crate::kms::vk::scanout::BoPhase;
                            let pool = pool.display_pool();
                            pool.bos
                                .iter()
                                .enumerate()
                                .find(|(_, bo)| bo.state.phase == BoPhase::OnScreen)
                                .and_then(|(bo_idx, bo)| bo.fb_handle.map(|fb| (bo_idx, fb)))
                                .or_else(|| {
                                    pool.bos.iter().enumerate().find_map(|(bo_idx, bo)| {
                                        bo.fb_handle.map(|fb| (bo_idx, fb))
                                    })
                                })
                        });
                let Some((bo_idx, fb_id)) = front else {
                    self.active_cursor_crtcs.remove(&cursor_key);
                    let error = io::Error::other(format!(
                        "dpms_set_outputs_active(true): no framebuffer for output {}; \
                         an ordinary page flip cannot restore MODE_ID/ACTIVE",
                        layout.output.connector_name
                    ));
                    log::error!("{error}");
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                    continue;
                };
                if let Err(e) = self.output_power_executor.enable(
                    &device,
                    &layout.output,
                    fb_id,
                    &self.services,
                ) {
                    self.active_cursor_crtcs.remove(&cursor_key);
                    log::error!(
                        "dpms_set_outputs_active(true): commit_modeset for {} failed: {e}",
                        layout.output.connector_name,
                    );
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                } else {
                    modeset_crtcs.push(cursor_key);
                    self.active_cursor_crtcs.insert(cursor_key);
                    if let Some(scanout) = self.scanout_pools.get_mut(i).and_then(Option::as_mut) {
                        scanout.display_pool_mut().bos[bo_idx]
                            .state
                            .mark_on_screen_after_modeset();
                        if let Err(error) = scanout.note_kms_modeset_installed(bo_idx) {
                            log::error!(
                                "dpms_set_outputs_active(true): copied ownership ledger failed for \
                                 output {i} bo {bo_idx}: {error}"
                            );
                            self.gpu.renderer_failed = true;
                            if first_err.is_none() {
                                first_err = Some(error);
                            }
                        }
                    }
                }
            }
            for key in modeset_crtcs {
                self.invalidate_cursor_applied(key);
            }
        } else {
            for output_idx in 0..self.outputs.len() {
                let output_key = self.outputs[output_idx].key.clone();
                let connector_name = self.outputs[output_idx].output.connector_name.clone();
                let cursor_key =
                    crate::kms::render::platform::CrtcKey::for_output(&self.outputs[output_idx]);
                let Some(device) = self
                    .device_for_output(&output_key)
                    .map(|device| Rc::clone(&device.device))
                else {
                    let error = io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "dpms_set_outputs_active(false): no DRM device {} for {}",
                            output_key.device_key, connector_name,
                        ),
                    );
                    log::error!("{error}");
                    if first_err.is_none() {
                        first_err = Some(error);
                    }
                    continue;
                };
                let cursor_detach = self.cursor_plane_detach_for_output(output_idx);
                if let Err(e) = self.output_power_executor.disable(
                    &device,
                    &self.outputs[output_idx].output,
                    cursor_detach,
                    &self.services,
                ) {
                    log::error!(
                        "dpms_set_outputs_active(false): disable_output for {} failed: {e}",
                        connector_name,
                    );
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                    self.quarantine_cursor_output(output_idx);
                } else {
                    self.accept_cursor_lifecycle_detach(output_idx);
                    self.active_cursor_crtcs.remove(&cursor_key);
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests_accept_output_power_commits(&mut self) {
        let executor = self
            .services
            .test_recorders
            .as_ref()
            .map(|recorders| Rc::clone(&recorders.output_power))
            .unwrap_or_else(|| Rc::new(RecordingOutputPowerExecutor::default()));
        self.output_power_executor = executor;
    }

    #[cfg(test)]
    pub(crate) fn for_tests_record_complete_output_power_commits(&mut self) {
        let executor = self
            .services
            .test_recorders
            .as_ref()
            .map(|recorders| Rc::clone(&recorders.output_power))
            .unwrap_or_else(|| Rc::new(RecordingOutputPowerExecutor::default()));
        self.output_power_executor = executor;
    }
}
