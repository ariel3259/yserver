//! Owner-route request preparation for the direct-to-composed unflip.
//!
//! The legacy request flags remain on `KmsBackend`. This module is the single
//! owner fork: it records the device-scoped admission intent and performs the
//! request-side preparation without reserving a dispatch capacity role.

use std::{collections::BTreeSet, io, rc::Rc};

use crate::{
    kms::{
        owner::{admission::Admitted, build::CommitDescription},
        render::{
            backend::KmsBackend,
            composed_commit::{
                ComposedPlane, composed_description, discover_composed_property_ids,
            },
            platform::CrtcKey,
            resources::{CommitResources, GroupMember, ResourceError},
        },
    },
    platform::drm::DrmDeviceKey,
};

fn direct_group_crtcs(backend: &KmsBackend, device: DrmDeviceKey) -> BTreeSet<u32> {
    backend
        .platform
        .outputs
        .iter()
        .filter(|output| output.key.device_key == device)
        .map(|output| u32::from(output.output.crtc))
        .collect()
}

fn decision_crtcs(decision: &Admitted) -> Option<&BTreeSet<u32>> {
    match decision {
        Admitted::Unflip { crtcs } => Some(crtcs),
        _ => None,
    }
}

/// The unflip always describes the complete direct group. The admission
/// barrier names that same group, but it is not a license to omit a device
/// CRTC from the atomic replacement.
pub(crate) fn members(
    backend: &KmsBackend,
    device: DrmDeviceKey,
    crtcs: &BTreeSet<u32>,
) -> Result<Vec<GroupMember>, ResourceError> {
    if !backend.direct_scanout_topology_eligible() {
        return Err(ResourceError::InvalidProof);
    }
    let expected = direct_group_crtcs(backend, device);
    if expected != *crtcs {
        return Err(ResourceError::InvalidProof);
    }
    let topology_generation = backend
        .platform
        .owner_ref(device)
        .map_or(0, |owner| owner.topology_generation());
    Ok(backend
        .platform
        .outputs
        .iter()
        .filter(|output| output.key.device_key == device)
        .map(|output| GroupMember::new(CrtcKey::for_output(output), topology_generation, 1))
        .collect())
}

/// Build one primary-only owner transaction for the complete device plane set.
/// Cursor and gamma are intentionally absent, just as in the composed owner
/// producer; their producers belong to later stages.
pub(crate) fn description(
    backend: &mut KmsBackend,
    device: DrmDeviceKey,
    decision: &Admitted,
) -> io::Result<CommitDescription> {
    let Some(crtcs) = decision_crtcs(decision) else {
        return Err(io::Error::other("owner unflip needs an Unflip decision"));
    };
    members(backend, device, crtcs)
        .map_err(|error| io::Error::other(format!("owner unflip members: {error}")))?;
    let device_index = backend
        .platform
        .devices
        .iter()
        .position(|entry| entry.key == device)
        .ok_or_else(|| io::Error::other("owner unflip KMS device is unavailable"))?;
    let output_indices = backend
        .platform
        .outputs
        .iter()
        .enumerate()
        .filter(|(_, output)| output.key.device_key == device)
        .map(|(output_idx, _)| output_idx)
        .collect::<Vec<_>>();
    let framebuffers = output_indices
        .iter()
        .map(|&output_idx| {
            let bo_fb_handle = backend
                .platform
                .scanout_pools
                .get(output_idx)
                .and_then(Option::as_ref)
                .and_then(|scanout| scanout.display_pool().bos.first())
                .and_then(|bo| bo.fb_handle);
            let framebuffer = if let Some(service) = backend.resource_service.as_mut() {
                backend
                    .scene
                    .owner_current_framebuffer(output_idx, bo_fb_handle, service)
                    .ok()
                    .flatten()
                    .or_else(|| backend.platform.retained_composed_framebuffer(output_idx))
            } else {
                backend.platform.retained_composed_framebuffer(output_idx)
            };
            framebuffer.ok_or_else(|| {
                io::Error::other(format!(
                    "owner unflip: output {output_idx} has no retained composed framebuffer"
                ))
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    let planes = output_indices
        .iter()
        .zip(framebuffers)
        .map(|(&output_idx, framebuffer)| ComposedPlane {
            output: &backend.platform.outputs[output_idx].output,
            framebuffer,
        })
        .collect::<Vec<_>>();
    let drm_device = Rc::clone(&backend.platform.devices[device_index].device);
    let property_ids = discover_composed_property_ids(
        &drm_device,
        &planes,
        &mut backend.platform.devices[device_index].active_property_cache,
    )
    .map_err(|error| io::Error::other(format!("discover owner unflip properties: {error}")))?;
    Ok(composed_description(&planes, property_ids))
}

/// The composed return has no direct-capacity role of its own. Keep one
/// resource entry per member so a later scene commit can replace one output's
/// returned ownership while another output is still awaiting its proof; the
/// owner transaction itself remains one atomic complete-device replacement.
pub(crate) fn resources(
    backend: &KmsBackend,
    device: DrmDeviceKey,
    decision: &Admitted,
) -> Result<Vec<CommitResources>, ResourceError> {
    let Some(crtcs) = decision_crtcs(decision) else {
        return Err(ResourceError::InvalidProof);
    };
    let members = members(backend, device, crtcs)?;
    if members.is_empty() {
        return Err(ResourceError::InvalidProof);
    }
    Ok(members
        .into_iter()
        .map(|member| CommitResources::new(Vec::new(), None, None, None, vec![member], Vec::new()))
        .collect())
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
