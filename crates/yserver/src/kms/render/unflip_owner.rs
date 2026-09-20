//! Owner-route request preparation for the direct-to-composed unflip.
//!
//! The legacy request flags remain on `KmsBackend`. This module is the single
//! owner fork: it records the device-scoped admission intent and performs the
//! request-side preparation without reserving a dispatch capacity role.

use std::collections::BTreeSet;

use crate::{kms::render::backend::KmsBackend, platform::drm::DrmDeviceKey};

fn direct_group_crtcs(backend: &KmsBackend, device: DrmDeviceKey) -> BTreeSet<u32> {
    backend
        .platform
        .outputs
        .iter()
        .filter(|output| output.key.device_key == device)
        .map(|output| u32::from(output.output.crtc))
        .collect()
}

/// Enter the owner admission request once for the direct group, then perform
/// the request half of the managed unflip seam. A materialization failure is
/// deliberately non-fatal: the request remains queued and the transaction
/// fork retries the shadow on a later tick.
pub(crate) fn request(backend: &mut KmsBackend, device: DrmDeviceKey) {
    let crtcs = direct_group_crtcs(backend, device);
    if crtcs.is_empty() {
        log::warn!("owner unflip request has no live CRTC members on {device:?}");
        return;
    }
    if let Err(error) = backend.admission_request_unflip(device, crtcs) {
        log::warn!("owner unflip request refused for {device:?}: {error}");
        return;
    }
    if let Err(error) = backend.managed_prepare_direct_unflip_request() {
        log::debug!("owner unflip shadow is not ready yet for {device:?}: {error}");
    }
}

/// Retry only the shadow materialization at the `maybe_composite` transaction
/// fork. Once ready, wake admission; the dispatch task consumes the ready
/// unflip and replaces the legacy transaction at that same fork.
pub(crate) fn retry_materialization(backend: &mut KmsBackend, device: DrmDeviceKey) -> bool {
    if !backend.direct_unflip_shadow_ready()
        && let Err(error) = backend.managed_retry_direct_unflip_shadow()
    {
        log::debug!("owner unflip shadow retry is still pending for {device:?}: {error}");
        return false;
    }
    let _ = backend.admission_wake(device, false);
    true
}
