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
            resources::{
                AllocationLease, AllocationPayload, CommitResources, DirectRole, ResourceError,
                RoleReservation,
            },
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
    prepared: &CommitResources,
) -> Result<
    (
        CommitDescription,
        crate::kms::owner::completion::CompletionContext,
    ),
    String,
> {
    let successor = decision_direct_successor(decision)
        .ok_or_else(|| "direct owner description needs a direct primary".to_string())?;
    let framebuffer = prepared
        .allocations
        .iter()
        .find_map(|lease| {
            backend
                .resource_service
                .as_ref()?
                .direct_framebuffer_handle(lease)
                .ok()
        })
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

    // This is intentionally a primary-only description. Cursor and gamma
    // maintenance are stage 4 payloads; Cii must not copy an unchanged
    // generation into the direct primary request. The scene has no owner
    // damage transaction for this producer either, so owner milestones
    // cannot stage or retire composed damage.
    let mut description = composed_description(&members, property_ids);
    description.page_flip_event = true;
    description.present_consumers = successor.crtcs.iter().copied().collect();
    let context =
        backend.direct_owner_completion_context(device, &description.present_consumers)?;
    Ok((description, context))
}

/// Adopt the accepted M1 framebuffer at the Owner preparation fork. The
/// cache is converted from a raw-resource owner to a managed index only
/// after service adoptability has been proven; once `into_managed` runs, the
/// returned payload is the sole owner and every failure is discharged through
/// the paired registry.
pub(crate) fn adopt_framebuffer(
    backend: &mut KmsBackend,
    source_id: DrawableId,
    source_pin: u64,
    preparing: &RoleReservation,
) -> Result<Option<AllocationLease>, ResourceError> {
    let (device, incarnation, service_binding) = {
        let service = backend
            .resource_service
            .as_ref()
            .ok_or(ResourceError::InvalidState)?;
        (
            service.device(),
            service.incarnation(),
            service.direct_lease_binding(),
        )
    };
    let permit = backend.commit_consumer.capacity.mint_direct_lease_permit(
        preparing,
        device,
        incarnation,
        service_binding,
    )?;
    {
        let service = backend
            .resource_service
            .as_ref()
            .ok_or(ResourceError::InvalidState)?;
        if service
            .check_direct_framebuffer_adoptability(
                &backend.commit_consumer.capacity,
                DirectRole::Preparing,
                &permit,
            )
            .is_err()
        {
            return Ok(None);
        }
    }

    let framebuffer = backend
        .take_owner_probe_framebuffer(source_id)
        .ok_or(ResourceError::InvalidState)?;
    let source_key = backend
        .present_source_pin_lease(source_pin)
        .map(|storage| storage.allocation.key());
    let source_lease = match source_key {
        Some(key) => Some(
            backend
                .resource_service
                .as_mut()
                .ok_or(ResourceError::InvalidState)?
                .reserve(key, crate::kms::render::resources::UseKind::Read)?,
        ),
        None => None,
    };
    let allocation = framebuffer
        .into_managed(
            backend
                .drm_cleanup_registry
                .as_mut()
                .ok_or(ResourceError::InvalidState)?,
            source_lease,
        )
        .ok_or(ResourceError::InvalidState)?;
    let payload = AllocationPayload::DirectFramebuffer(allocation);

    let adopted = {
        let service = backend
            .resource_service
            .as_mut()
            .ok_or(ResourceError::InvalidState)?;
        let registry = backend
            .drm_cleanup_registry
            .as_mut()
            .ok_or(ResourceError::InvalidState)?;
        service.adopt_direct_framebuffer(
            payload,
            registry,
            &backend.commit_consumer.capacity,
            DirectRole::Preparing,
            permit,
        )
    };
    match adopted {
        Ok(lease) => Ok(Some(lease)),
        Err((error, mut payload)) => {
            backend.remove_owner_probe_entry(source_id);
            let cleanup = backend
                .drm_cleanup_registry
                .as_mut()
                .ok_or(ResourceError::InvalidState)?;
            if let Err(cleanup_error) = payload.discharge_file_owned(cleanup) {
                // Task 2 supplies the retry owner. Until then, preserve the
                // sole payload owner and fail closed; never silently drop a
                // right whose cleanup did not succeed.
                log::error!(
                    "owner direct framebuffer adoption cleanup failed after service refusal: {cleanup_error}"
                );
                backend.commit_consumer.capacity.close_admission();
                // Task 2 removes the forget.
                std::mem::forget(payload);
                return Err(ResourceError::InvalidState);
            }
            backend.commit_consumer.capacity.close_admission();
            Err(error)
        }
    }
}

/// Move the producer-owned members and Present pin leases into a direct commit.
///
/// The queued frame is the source of truth. In particular, this does not
/// inspect the consumer's current resource state to reconstruct membership.
pub(crate) fn resources(backend: &mut KmsBackend) -> Result<CommitResources, ResourceError> {
    backend.take_direct_owner_resources()
}

/// Wake the owner conductor after a direct retirement has been enqueued.
pub(crate) fn retirement_wake(backend: &mut KmsBackend, device: DrmDeviceKey) {
    let _ = backend.admission_wake(device, true);
}
