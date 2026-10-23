use super::PlatformBackend;
use crate::drm::{
    atomic::{AtomicCommitEvent, AtomicCommitKind, AtomicCommitRequest},
    modeset::{append_pending_gamma_lut, finish_pending_gamma_lut},
};

impl PlatformBackend {
    /// Submit an idle output's pending ramp as an event-producing CRTC commit.
    /// An EBUSY rejection retains the ramp and waits for that CRTC's next event.
    pub(crate) fn submit_pending_gamma_for_output(&self, output_idx: usize) -> std::io::Result<()> {
        let Some(layout) = self.outputs.get(output_idx) else {
            return Ok(());
        };
        let Some(device) = self.device_for_key(layout.key.device_key) else {
            return Ok(());
        };
        let output = &layout.output;
        if output.pending_gamma_lut.borrow().is_none()
            || device
                .atomic_commit_sink
                .page_flip_commit_in_flight(&device.device, output.crtc)
        {
            return Ok(());
        }

        let mut request = AtomicCommitRequest::new();
        let Some(blob) = append_pending_gamma_lut(
            &device.device,
            &device.atomic_commit_sink,
            output,
            &mut request,
        ) else {
            return Ok(());
        };
        let result = device.atomic_commit_sink.submit_with_commit_id(
            &device.device,
            ::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT
                | ::drm::control::AtomicCommitFlags::NONBLOCK,
            AtomicCommitKind::GammaOnly,
            request,
            &[AtomicCommitEvent {
                crtc: output.crtc,
                motion_timestamp: None,
            }],
        );
        finish_pending_gamma_lut(
            &device.device,
            &device.atomic_commit_sink,
            output,
            blob,
            &result,
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) if error.raw_os_error() == Some(libc::EBUSY) => Ok(()),
            Err(error)
                if error.raw_os_error() == Some(libc::EINVAL)
                    && output.pending_gamma_lut.borrow().is_none() =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Submit retained gamma after a page-flip event has freed this CRTC.
    /// Pending gamma from an EBUSY rejection and gamma held behind an async
    /// flip both use the same event-driven path.
    pub(crate) fn retry_pending_gamma_after_flip(
        &self,
        output_idx: usize,
        now: std::time::Instant,
    ) {
        let Some(output) = self.outputs.get(output_idx).map(|layout| &layout.output) else {
            return;
        };
        if output.pending_gamma_lut.borrow().is_none() {
            return;
        }
        if let Some(deadline) = output.pending_gamma_deadline.get() {
            if now < deadline {
                return;
            }
            output.pending_gamma_deadline.set(None);
        }
        if let Err(error) = self.submit_pending_gamma_for_output(output_idx) {
            log::warn!(
                "kms gamma: retry for CRTC {:?} failed: {error}",
                output.crtc
            );
        }
    }

    /// Scene, direct-scanout, and composed scheduling share this gate so an
    /// accepted gamma-only event commit owns the CRTC until its event arrives.
    pub(crate) fn scanout_commit_in_flight_for_output(&self, output_idx: usize) -> bool {
        if self.cursor_commit_in_flight_for_output(output_idx) {
            return true;
        }
        let Some(layout) = self.outputs.get(output_idx) else {
            return false;
        };
        self.device_for_key(layout.key.device_key)
            .is_some_and(|device| {
                device
                    .atomic_commit_sink
                    .page_flip_commit_in_flight(&device.device, layout.output.crtc)
            })
    }

    pub(crate) fn scanout_commit_in_flight_for_outputs(&self, output_indices: &[usize]) -> bool {
        output_indices
            .iter()
            .any(|output_idx| self.scanout_commit_in_flight_for_output(*output_idx))
    }
}
