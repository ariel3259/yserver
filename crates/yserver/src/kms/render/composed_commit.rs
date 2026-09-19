use std::{collections::BTreeMap, io};

use drm::control::framebuffer;

use crate::{
    drm::{Device, modeset::PropMap},
    kms::owner::{
        build::CommitDescription,
        closure::{CrtcPower, ObjectKind, PropertyIds, SerializedObject},
    },
    platform::drm::Output,
};

/// One output member of a composed atomic commit.
///
/// The framebuffer is the image rendered for this output. The plane and CRTC
/// properties come from the output's KMS discovery, so the builder cannot
/// accidentally substitute a property from another device.
#[allow(
    dead_code,
    reason = "Task 5 wires the owner-route producer to this builder"
)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct ComposedPlane<'a> {
    pub(crate) output: &'a Output,
    pub(crate) framebuffer: framebuffer::Handle,
}

/// Per-KMS-device cache for the CRTC `ACTIVE` property.
///
/// `Output` already caches the plane properties and optional
/// `OUT_FENCE_PTR`, but it did not carry `ACTIVE`. Keep this cache with the
/// platform's device entry instead of doing a property enumeration for every
/// composed frame. Property ids are stable for a CRTC while that KMS device
/// and topology incarnation remain in use.
#[allow(
    dead_code,
    reason = "Task 5 wires the owner-route producer to this cache"
)]
#[derive(Debug, Default)]
pub(crate) struct ActivePropertyCache {
    by_crtc: BTreeMap<u32, u32>,
}

impl ActivePropertyCache {
    #[allow(
        dead_code,
        reason = "Task 5 wires the owner-route producer to discovery"
    )]
    pub(crate) fn get_or_discover(
        &mut self,
        device: &Device,
        crtc: drm::control::crtc::Handle,
    ) -> io::Result<u32> {
        let crtc_id = u32::from(crtc);
        if let Some(&property_id) = self.by_crtc.get(&crtc_id) {
            return Ok(property_id);
        }
        let property_id = u32::from(PropMap::for_object(device, crtc)?.id("ACTIVE")?);
        self.by_crtc.insert(crtc_id, property_id);
        Ok(property_id)
    }
}

#[derive(Debug, thiserror::Error)]
#[allow(
    dead_code,
    reason = "Task 5 wires the owner-route producer to discovery"
)]
pub(crate) enum ComposedPropertyError {
    #[error("a composed description needs at least one output member")]
    EmptyMembers,
    #[error("could not discover ACTIVE for CRTC {crtc}: {source}")]
    Active {
        crtc: u32,
        #[source]
        source: io::Error,
    },
    #[error("could not discover OUT_FENCE_PTR for CRTC {crtc}: {source}")]
    OutFence {
        crtc: u32,
        #[source]
        source: io::Error,
    },
    #[error("composed members use different {property} property ids ({first} and {other})")]
    InconsistentProperty {
        property: &'static str,
        first: u32,
        other: u32,
    },
}

/// Discover the device-level property ids needed by [`composed_description`].
///
/// Discovery is fallible by design. A producer must treat an error as
/// readiness failure; this helper never turns a missing DRM property into a
/// panic or a partially-built commit description.
#[allow(
    dead_code,
    reason = "Task 5 wires the owner-route producer to discovery"
)]
pub(crate) fn discover_composed_property_ids(
    device: &Device,
    members: &[ComposedPlane<'_>],
    active_properties: &mut ActivePropertyCache,
) -> Result<PropertyIds, ComposedPropertyError> {
    let first = members.first().ok_or(ComposedPropertyError::EmptyMembers)?;
    let first_crtc = u32::from(first.output.crtc);
    let crtc_id = u32::from(first.output.plane_crtc_id_prop);
    let active = active_properties
        .get_or_discover(device, first.output.crtc)
        .map_err(|source| ComposedPropertyError::Active {
            crtc: first_crtc,
            source,
        })?;
    let out_fence_ptr = output_out_fence_ptr_property(device, first.output).map_err(|source| {
        ComposedPropertyError::OutFence {
            crtc: first_crtc,
            source,
        }
    })?;

    for member in &members[1..] {
        let member_crtc = u32::from(member.output.crtc);
        let member_crtc_id = u32::from(member.output.plane_crtc_id_prop);
        if member_crtc_id != crtc_id {
            return Err(ComposedPropertyError::InconsistentProperty {
                property: "plane CRTC_ID",
                first: crtc_id,
                other: member_crtc_id,
            });
        }
        let member_active = active_properties
            .get_or_discover(device, member.output.crtc)
            .map_err(|source| ComposedPropertyError::Active {
                crtc: member_crtc,
                source,
            })?;
        if member_active != active {
            return Err(ComposedPropertyError::InconsistentProperty {
                property: "CRTC ACTIVE",
                first: active,
                other: member_active,
            });
        }
        let member_out_fence =
            output_out_fence_ptr_property(device, member.output).map_err(|source| {
                ComposedPropertyError::OutFence {
                    crtc: member_crtc,
                    source,
                }
            })?;
        if member_out_fence != out_fence_ptr {
            return Err(ComposedPropertyError::InconsistentProperty {
                property: "CRTC OUT_FENCE_PTR",
                first: out_fence_ptr,
                other: member_out_fence,
            });
        }
    }

    Ok(PropertyIds {
        crtc_id,
        active,
        out_fence_ptr,
    })
}

#[allow(
    dead_code,
    reason = "Task 5 wires the owner-route producer to discovery"
)]
fn output_out_fence_ptr_property(device: &Device, output: &Output) -> io::Result<u32> {
    output.crtc_out_fence_ptr_prop.map(u32::from).map_or_else(
        || {
            Ok(u32::from(
                PropMap::for_object(device, output.crtc)?.id("OUT_FENCE_PTR")?,
            ))
        },
        Ok,
    )
}

/// Build the persistent part of a composed commit.
///
/// The owner request builder appends `OUT_FENCE_PTR` to each expected CRTC;
/// this function deliberately does not. Composed commits are non-Present and
/// carry neither a page-flip event nor Present consumers.
#[allow(
    dead_code,
    reason = "Task 5 wires the owner-route producer to this builder"
)]
pub(crate) fn composed_description(
    members: &[ComposedPlane<'_>],
    property_ids: PropertyIds,
) -> CommitDescription {
    let mut objects = Vec::with_capacity(members.len() * 2);
    let mut crtc_state = Vec::with_capacity(members.len());

    for member in members {
        let output = member.output;
        let crtc_id = u32::from(output.crtc);
        objects.push(SerializedObject {
            object: crtc_id,
            kind: ObjectKind::Crtc,
            old_crtc_id: None,
            props: vec![(property_ids.active, 1)],
        });
        objects.push(SerializedObject {
            object: u32::from(output.plane),
            kind: ObjectKind::Plane,
            old_crtc_id: Some(crtc_id),
            props: vec![
                (
                    u32::from(output.plane_fb_id_prop),
                    u64::from(u32::from(member.framebuffer)),
                ),
                (u32::from(output.plane_crtc_id_prop), u64::from(crtc_id)),
            ],
        });
        crtc_state.push(CrtcPower {
            crtc_id,
            old_active: true,
            new_active: true,
        });
    }

    CommitDescription {
        objects,
        crtc_state,
        present_consumers: Vec::new(),
        page_flip_event: false,
        property_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::{ComposedPlane, composed_description};
    use crate::kms::{
        executor::{HostCallClass, protocol::DRM_MODE_PAGE_FLIP_EVENT},
        owner::{
            build::build_atomic_request,
            closure::{ObjectKind, PropertyIds},
            test_fixtures::atomic_correlation_for_tests,
        },
    };

    #[test]
    fn c0_conv_ci_description_single_output() {
        let output = test_output(1, 10);
        let framebuffer = ::drm::control::from_u32(100).unwrap();
        let member = ComposedPlane {
            output: &output,
            framebuffer,
        };

        let description = composed_description(&[member], test_property_ids());
        assert!(!description.page_flip_event);
        assert!(description.present_consumers.is_empty());
        assert_eq!(description.crtc_state.len(), 1);
        assert_eq!(description.objects.len(), 2);
        assert_eq!(description.objects[0].object, 1);
        assert_eq!(description.objects[0].kind, ObjectKind::Crtc);
        assert_eq!(description.objects[0].props, vec![(21, 1)]);
        assert_eq!(description.objects[1].object, 10);
        assert_eq!(description.objects[1].kind, ObjectKind::Plane);
        assert_eq!(description.objects[1].props, vec![(19, 100), (20, 1)]);

        let (request, closure) = build_atomic_request(
            &description,
            atomic_correlation_for_tests(1),
            HostCallClass::SeatActiveNonblock,
        )
        .expect("composed description must pass the owner closure");
        assert_eq!(closure.expected_completion(), &[1]);
        assert_eq!(request.flags & DRM_MODE_PAGE_FLIP_EVENT, 0);
    }

    #[test]
    fn c0_conv_ci_description_bundle() {
        let outputs = [test_output(1, 10), test_output(2, 20), test_output(3, 30)];
        let framebuffers = [100, 200, 300].map(|id| ::drm::control::from_u32(id).unwrap());
        let members = outputs
            .iter()
            .zip(framebuffers)
            .map(|(output, framebuffer)| ComposedPlane {
                output,
                framebuffer,
            })
            .collect::<Vec<_>>();

        let description = composed_description(&members, test_property_ids());
        assert!(!description.page_flip_event);
        assert!(description.present_consumers.is_empty());
        assert_eq!(description.objects.len(), 6);
        assert_eq!(
            description
                .objects
                .iter()
                .map(|object| object.object)
                .collect::<Vec<_>>(),
            vec![1, 10, 2, 20, 3, 30]
        );

        let (request, closure) = build_atomic_request(
            &description,
            atomic_correlation_for_tests(2),
            HostCallClass::SeatActiveNonblock,
        )
        .expect("composed bundle must pass the owner closure");
        assert_eq!(closure.expected_completion(), &[1, 2, 3]);
        assert_eq!(request.flags & DRM_MODE_PAGE_FLIP_EVENT, 0);
    }

    fn test_property_ids() -> PropertyIds {
        PropertyIds {
            crtc_id: 20,
            active: 21,
            out_fence_ptr: 22,
        }
    }

    fn test_output(crtc_id: u32, plane_id: u32) -> crate::platform::drm::Output {
        crate::platform::drm::Output {
            connector: ::drm::control::from_u32(crtc_id).unwrap(),
            connector_name: format!("test-{crtc_id}"),
            encoder: ::drm::control::from_u32(crtc_id).unwrap(),
            crtc: ::drm::control::from_u32(crtc_id).unwrap(),
            plane: ::drm::control::from_u32(plane_id).unwrap(),
            // SAFETY: this fixture never passes the mode to DRM.
            mode: unsafe { std::mem::zeroed() },
            picked: crate::platform::drm::Mode {
                name: format!("test-{crtc_id}"),
                width: 800,
                height: 600,
                vrefresh: 60,
                preferred: true,
                ..Default::default()
            },
            plane_fb_id_prop: ::drm::control::from_u32(19).unwrap(),
            plane_crtc_id_prop: ::drm::control::from_u32(20).unwrap(),
            plane_src_x_prop: ::drm::control::from_u32(1).unwrap(),
            plane_src_y_prop: ::drm::control::from_u32(2).unwrap(),
            plane_src_w_prop: ::drm::control::from_u32(3).unwrap(),
            plane_src_h_prop: ::drm::control::from_u32(4).unwrap(),
            plane_crtc_x_prop: ::drm::control::from_u32(5).unwrap(),
            plane_crtc_y_prop: ::drm::control::from_u32(6).unwrap(),
            plane_crtc_w_prop: ::drm::control::from_u32(7).unwrap(),
            plane_crtc_h_prop: ::drm::control::from_u32(8).unwrap(),
            plane_in_fence_fd_prop: None,
            crtc_out_fence_ptr_prop: Some(::drm::control::from_u32(22).unwrap()),
            scanout_modifiers: Vec::new(),
            mm_width: 0,
            mm_height: 0,
            edid: Vec::new(),
            connector_type: "unknown".to_string(),
            modes: Vec::new(),
        }
    }
}
