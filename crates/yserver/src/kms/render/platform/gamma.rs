use super::PlatformBackend;
use crate::{
    drm::{
        atomic::{AtomicCommitEvent, AtomicCommitKind, AtomicCommitRequest},
        modeset::{append_pending_gamma_lut, finish_pending_gamma_lut},
    },
    kms::render::platform::CrtcKey,
};

impl PlatformBackend {
    /// Submit an idle output's pending ramp as an event-producing CRTC commit.
    /// An EBUSY rejection retains the ramp and waits for that CRTC's next event.
    pub(crate) fn submit_pending_gamma_for_output(
        &self,
        output_idx: usize,
        now: std::time::Instant,
        force_other_crtc_busy: bool,
    ) -> std::io::Result<()> {
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
            now,
            force_other_crtc_busy,
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
            true,
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

    /// Retry retained gamma after a page-flip event frees a CRTC on its device.
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
        let deadline_elapsed = output
            .pending_gamma_deadline
            .get()
            .is_some_and(|deadline| now >= deadline);
        if let Some(deadline) = output.pending_gamma_deadline.get()
            && now < deadline
            && self.gamma_output_is_active(output_idx)
        {
            return;
        }
        if let Err(error) = self.submit_pending_gamma_for_output(output_idx, now, deadline_elapsed)
        {
            log::warn!(
                "kms gamma: retry for CRTC {:?} failed: {error}",
                output.crtc
            );
        }
    }

    /// Keep Task 2's one-refresh wait consistent between request-time
    /// submission and retries triggered by another CRTC's page-flip event.
    pub(crate) fn gamma_output_is_active(&self, output_idx: usize) -> bool {
        let Some(layout) = self.outputs.get(output_idx) else {
            return false;
        };
        let refresh_hz = match layout.output.picked.vrefresh {
            0 => 60,
            refresh_hz => refresh_hz,
        };
        let refresh_period = std::time::Duration::from_nanos(1_000_000_000 / u64::from(refresh_hz));
        let activity_now = crate::drm::atomic::monotonic_timestamp();
        self.present_get_last_page_flip_received_at(CrtcKey::for_output(layout))
            .zip(activity_now)
            .and_then(|(received_at, now)| now.checked_sub(received_at))
            .is_some_and(|age| age <= refresh_period.saturating_mul(2))
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
