//! Owner-route plumbing for direct scanout.
//!
//! The legacy direct producer remains in `backend.rs`. This module owns the
//! one owner-route fork: offering a prepared direct Present, building the
//! owner description/resources for its admission, and waking the conductor
//! after a retirement.

use std::{io, rc::Rc};

use crate::{
    kms::{
        owner::{
            admission::{AdmissionDecision, Admitted, DirectSuccessor},
            build::CommitDescription,
        },
        render::{
            backend::KmsBackend,
            composed_commit::{
                ComposedPlane, composed_description, discover_composed_property_ids,
            },
            resources::CommitResources,
            store::DrawableId,
        },
    },
    platform::drm::DrmDeviceKey,
};

fn direct_successor(admitted: &Admitted) -> Option<&DirectSuccessor> {
    match admitted {
        Admitted::Direct { successor } => Some(successor),
        Admitted::Bundle { members } => members.iter().find_map(direct_successor),
        _ => None,
    }
}

fn decision_direct_successor(decision: &AdmissionDecision) -> Option<&DirectSuccessor> {
    direct_successor(match &decision.admitted {
        Admitted::Maintenance { .. } => decision.combined_primary.as_ref()?,
        admitted => admitted,
    })
}

/// Offer a prepared direct Present to the device's latest-wins successor slot.
/// The caller has already evaluated the production eligibility predicate and
/// selected this device's route; this function performs the owner-side
/// preparation and wakes admission exactly once for a successful offer.
pub(crate) fn offer(
    backend: &mut KmsBackend,
    device: DrmDeviceKey,
    source_id: DrawableId,
    candidate: yserver_core::backend::PresentScanoutCandidate,
    event: yserver_core::backend::CompletedPresentEvent,
) -> io::Result<bool> {
    let offered = backend
        .admission_offer_direct(device, source_id, candidate, event)
        .map_err(|error| io::Error::other(format!("owner direct offer failed: {error}")))?;
    if offered {
        let _ = backend.admission_wake(device, false);
    }
    Ok(offered)
}

/// Build the minimum direct atomic description used by Task 4.
///
/// Later direct-producer tasks extend this builder with the producer-owned
/// leases, Present event context, and CRTC consumer ids. The framebuffer and
/// KMS members already come from the queued producer frame here; no injected
/// admission source is consulted.
pub(crate) fn description(
    backend: &mut KmsBackend,
    device: DrmDeviceKey,
    decision: &AdmissionDecision,
) -> Result<CommitDescription, String> {
    let successor = decision_direct_successor(decision)
        .ok_or_else(|| "direct owner description needs a direct primary".to_string())?;
    let source_id = backend
        .direct_successor_source_for_owner(successor.source_generation)
        .ok_or_else(|| "direct owner successor frame is unavailable".to_string())?;
    let framebuffer = backend
        .direct_source_framebuffer_for_owner(source_id)
        .ok_or_else(|| "direct owner framebuffer is unavailable".to_string())?;

    let output_indices = backend
        .platform
        .outputs
        .iter()
        .enumerate()
        .filter(|(_, output)| {
            output.key.device_key == device
                && successor.crtcs.contains(&u32::from(output.output.crtc))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if output_indices.is_empty() {
        return Err("direct owner successor has no live output members".to_string());
    }

    let members = output_indices
        .iter()
        .map(|&index| ComposedPlane {
            output: &backend.platform.outputs[index].output,
            framebuffer,
        })
        .collect::<Vec<_>>();
    let device_index = backend
        .platform
        .devices
        .iter()
        .position(|entry| entry.key == device)
        .ok_or_else(|| "direct owner KMS device is unavailable".to_string())?;
    let drm_device = Rc::clone(&backend.platform.devices[device_index].device);
    let property_ids = discover_composed_property_ids(
        &drm_device,
        &members,
        &mut backend.platform.devices[device_index].active_property_cache,
    )
    .map_err(|error| format!("discover direct owner properties: {error}"))?;

    let mut description = composed_description(&members, property_ids);
    // Task 6 adds the Present event/context through the context-carrying
    // owner entry.
    description.page_flip_event = false;
    Ok(description)
}

/// Construct the producer-owned resource envelope for a direct commit.
///
/// Task 5 fills this envelope with the frame's members and present-pin leases;
/// keeping construction here makes the production dispatch independent of the
/// injected `AdmissionSource` from the start of the owner route.
pub(crate) fn resources() -> CommitResources {
    CommitResources::new(Vec::new(), None, None, None, Vec::new(), Vec::new())
}

/// Wake the owner conductor after a direct retirement has been enqueued.
pub(crate) fn retirement_wake(backend: &mut KmsBackend, device: DrmDeviceKey) {
    let _ = backend.admission_wake(device, true);
}
