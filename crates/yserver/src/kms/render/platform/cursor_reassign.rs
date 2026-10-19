//! Desired cursor-plane assignments and the accepted owners they must wait for.

use crate::{
    kms::{
        cursor_plane::topology::CursorPlaneProperties,
        render::platform::{CrtcKey, KmsDevice},
    },
    platform::drm::DrmDeviceKey,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CursorPlaneKey {
    pub(crate) device_key: DrmDeviceKey,
    pub(crate) plane: ::drm::control::plane::Handle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CursorReassignment {
    pub(crate) plane: CursorPlaneKey,
    pub(crate) old_owner: CrtcKey,
    pub(crate) new_owner: CrtcKey,
}

/// Compare a newly discovered matching with the owners retained by accepted
/// bindings and commits whose replacement events have not drained yet.
pub(crate) fn reconcile_cursor_assignments(
    desired: &[(CursorPlaneKey, CrtcKey)],
    actual: &[(CursorPlaneKey, CrtcKey)],
) -> Vec<CursorReassignment> {
    let mut reassignments = Vec::new();
    for &(plane, new_owner) in desired {
        for &(actual_plane, old_owner) in actual {
            if actual_plane == plane && old_owner != new_owner {
                reassignments.push(CursorReassignment {
                    plane,
                    old_owner,
                    new_owner,
                });
            }
        }
    }
    reassignments
}

/// True while the plane is still accepted on another CRTC or an accepted
/// detach/replacement on that CRTC has not reached its event boundary.
pub(crate) fn attach_barrier(
    reassignments: &[CursorReassignment],
    plane: CursorPlaneKey,
    destination: CrtcKey,
) -> bool {
    reassignments
        .iter()
        .any(|move_| move_.plane == plane && move_.new_owner == destination)
}

pub(crate) fn assignments_for_device(
    device: &KmsDevice,
    desired_routes: impl IntoIterator<Item = (CrtcKey, CursorPlaneProperties)>,
) -> Vec<CursorReassignment> {
    let desired: Vec<_> = desired_routes
        .into_iter()
        .map(|(owner, properties)| {
            (
                CursorPlaneKey {
                    device_key: device.key,
                    plane: properties.plane,
                },
                owner,
            )
        })
        .collect();
    let mut actual = Vec::new();
    for (&crtc, state) in &device.cursor.crtc_states {
        let owner = CrtcKey {
            device_key: device.key,
            crtc,
        };
        let bindings = state
            .applied
            .into_iter()
            .chain(state.lifecycle_pending)
            .chain(
                state
                    .in_flight
                    .iter()
                    .flat_map(|commit| [commit.prior, commit.next].into_iter().flatten()),
            );
        for binding in bindings {
            actual.push((
                CursorPlaneKey {
                    device_key: device.key,
                    plane: binding.properties.plane,
                },
                owner,
            ));
        }
    }
    reconcile_cursor_assignments(&desired, &actual)
}
