//! Minimal atomic description for one output's client modeset.
//!
//! Preparation owns the new scanout pool, scene state and mode blob. This
//! module projects their already-prepared handles and the staged DPMS
//! decision into the device commit description.

use std::rc::Rc;

use crate::kms::{
    backend::{OutputInstanceId, OutputKey},
    owner::{
        build::CommitDescription,
        closure::{CrtcPower, ObjectKind, PropertyIds, SerializedObject},
        lifecycle::{DpmsTarget, dpms_target_for_level},
    },
    render::scene::StagedOutputSceneState,
    vk::scanout::OutputScanout,
};

use crate::platform::drm::Output;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StagedDpmsProjection {
    pub(crate) output: OutputKey,
    pub(crate) level: u8,
    pub(crate) target: DpmsTarget,
    pub(crate) epoch: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClientModesetObjects {
    pub(crate) connector: u32,
    pub(crate) crtc: u32,
    pub(crate) primary_plane: u32,
    /// The binding before this request, or zero if the target was detached.
    pub(crate) old_crtc_id: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClientModesetPropertyIds {
    pub(crate) connector_crtc_id: u32,
    pub(crate) crtc_mode_id: u32,
    pub(crate) plane_fb_id: u32,
    pub(crate) plane_crtc_id: u32,
    pub(crate) plane_src_x: u32,
    pub(crate) plane_src_y: u32,
    pub(crate) plane_src_w: u32,
    pub(crate) plane_src_h: u32,
    pub(crate) plane_crtc_x: u32,
    pub(crate) plane_crtc_y: u32,
    pub(crate) plane_crtc_w: u32,
    pub(crate) plane_crtc_h: u32,
    pub(crate) common: PropertyIds,
}

#[derive(Debug, Clone)]
pub(crate) enum ClientModesetOperation {
    Configure {
        width: u16,
        height: u16,
        framebuffer: u32,
        mode_blob: u32,
        projection: StagedDpmsProjection,
    },
    Disable,
}

#[derive(Debug, Clone)]
pub(crate) struct ClientModesetDescriptionInput {
    pub(crate) objects: ClientModesetObjects,
    pub(crate) properties: ClientModesetPropertyIds,
    pub(crate) old_active: bool,
    pub(crate) operation: ClientModesetOperation,
}

pub(crate) struct PreparedClientModesetDescription {
    pub(crate) description: CommitDescription,
    /// Present for enable and mode-change transactions. The caller retains it
    /// with the prepared transaction so freshness can be checked before both
    /// validation and executor dispatch.
    pub(crate) staged_projection: Option<StagedDpmsProjection>,
    pub(crate) prepared_set: PreparedClientModesetSet,
}

pub(crate) struct BuiltClientModesetDescription {
    pub(crate) description: CommitDescription,
    pub(crate) staged_projection: Option<StagedDpmsProjection>,
}

/// Resources kept alive from preparation until promotion or a non-installing
/// result. The owner consumes this set on every terminal path; Task 6 will
/// move its members into the installed platform and scene.
pub(crate) struct PreparedClientModesetSet {
    pub(crate) output: Option<Output>,
    pub(crate) output_instance_id: Option<OutputInstanceId>,
    pub(crate) scanout: Option<OutputScanout>,
    pub(crate) scene: Option<StagedOutputSceneState>,
    pub(crate) mode_blob: Option<OwnedModeBlob>,
    pub(crate) allocation_keys: Vec<crate::kms::render::resources::AllocationKey>,
    #[cfg(test)]
    pub(crate) framebuffer_handles_for_tests: Vec<u32>,
}

impl PreparedClientModesetSet {
    /// Drop the staged scene before releasing its never-submitted pool. The
    /// pool's managed retain leases and counted DRM aliases must be detached
    /// through the same 2c-i service path that adopted them.
    pub(crate) fn release(
        mut self,
        service: &mut crate::kms::render::resources::ResourceService,
        registry: &mut crate::kms::render::resources::DrmCleanupRegistry,
    ) {
        self.scene.take();
        if let Some(mut scanout) = self.scanout.take() {
            scanout.detach_managed_entries(Some(registry));
            drop(scanout);
        }
        self.mode_blob.take();
        let _ = service.service_ready_with_registry(registry);
    }
}

/// RAII owner for a DRM mode property blob used by TEST_ONLY and the live
/// request. Destroying it before installation is safe; a successful kernel
/// commit retains its own reference before this userspace reference drops.
pub(crate) struct OwnedModeBlob {
    device: Rc<crate::drm::Device>,
    raw: Option<u64>,
}

impl OwnedModeBlob {
    pub(crate) fn new(device: Rc<crate::drm::Device>, raw: u64) -> Self {
        Self {
            device,
            raw: Some(raw),
        }
    }

    #[cfg(test)]
    pub(crate) fn raw_for_tests(&self) -> Option<u64> {
        self.raw
    }
}

impl Drop for OwnedModeBlob {
    fn drop(&mut self) {
        use ::drm::control::Device as _;

        if let Some(raw) = self.raw.take()
            && let Err(error) = self.device.destroy_property_blob(raw)
        {
            log::error!("could not destroy prepared client modeset blob {raw}: {error}");
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ClientModesetBuildError {
    #[error("client modeset object ids must be nonzero and distinct")]
    InvalidObjectIds,
    #[error("client modeset property ids must be nonzero")]
    InvalidPropertyIds,
    #[error("the plane CRTC_ID property must match the closure CRTC_ID")]
    InconsistentCrtcIdProperty,
    #[error("client modeset dimensions must be nonzero")]
    InvalidDimensions,
    #[error("prepared framebuffer and mode blob ids must be nonzero")]
    MissingPreparedHandle,
    #[error("invalid staged DPMS level {0}")]
    InvalidDpmsLevel(u8),
    #[error("staged DPMS target does not match level {0}")]
    InconsistentDpmsTarget(u8),
}

pub(crate) fn stage_dpms_projection(
    output: OutputKey,
    level: u8,
    epoch: u64,
) -> Result<StagedDpmsProjection, ClientModesetBuildError> {
    let target =
        dpms_target_for_level(level).ok_or(ClientModesetBuildError::InvalidDpmsLevel(level))?;
    Ok(StagedDpmsProjection {
        output,
        level,
        target,
        epoch,
    })
}

pub(crate) fn build_client_modeset_description(
    input: ClientModesetDescriptionInput,
) -> Result<BuiltClientModesetDescription, ClientModesetBuildError> {
    let ClientModesetDescriptionInput {
        objects,
        properties,
        old_active,
        operation,
    } = input;
    if objects.connector == 0
        || objects.crtc == 0
        || objects.primary_plane == 0
        || objects.connector == objects.crtc
        || objects.connector == objects.primary_plane
        || objects.crtc == objects.primary_plane
    {
        return Err(ClientModesetBuildError::InvalidObjectIds);
    }

    let property_ids = [
        properties.connector_crtc_id,
        properties.crtc_mode_id,
        properties.plane_fb_id,
        properties.plane_crtc_id,
        properties.plane_src_x,
        properties.plane_src_y,
        properties.plane_src_w,
        properties.plane_src_h,
        properties.plane_crtc_x,
        properties.plane_crtc_y,
        properties.plane_crtc_w,
        properties.plane_crtc_h,
        properties.common.crtc_id,
        properties.common.active,
        properties.common.out_fence_ptr,
    ];
    if property_ids.contains(&0) {
        return Err(ClientModesetBuildError::InvalidPropertyIds);
    }
    if properties.plane_crtc_id != properties.common.crtc_id {
        return Err(ClientModesetBuildError::InconsistentCrtcIdProperty);
    }

    let target_crtc = u64::from(objects.crtc);
    let mut connector_props = vec![(properties.connector_crtc_id, target_crtc)];
    let (mode_id, active, plane_props, staged_projection) = match operation {
        ClientModesetOperation::Configure {
            width,
            height,
            framebuffer,
            mode_blob,
            projection,
        } => {
            if width == 0 || height == 0 {
                return Err(ClientModesetBuildError::InvalidDimensions);
            }
            if framebuffer == 0 || mode_blob == 0 {
                return Err(ClientModesetBuildError::MissingPreparedHandle);
            }
            if dpms_target_for_level(projection.level) != Some(projection.target) {
                return Err(ClientModesetBuildError::InconsistentDpmsTarget(
                    projection.level,
                ));
            }
            let active = u64::from(projection.target == DpmsTarget::On);
            let plane_props = vec![
                (properties.plane_fb_id, u64::from(framebuffer)),
                (properties.plane_crtc_id, target_crtc),
                (properties.plane_src_x, 0),
                (properties.plane_src_y, 0),
                (properties.plane_src_w, u64::from(width) << 16),
                (properties.plane_src_h, u64::from(height) << 16),
                (properties.plane_crtc_x, 0),
                (properties.plane_crtc_y, 0),
                (properties.plane_crtc_w, u64::from(width)),
                (properties.plane_crtc_h, u64::from(height)),
            ];
            (u64::from(mode_blob), active, plane_props, Some(projection))
        }
        ClientModesetOperation::Disable => {
            connector_props[0].1 = 0;
            (
                0,
                0,
                vec![(properties.plane_fb_id, 0), (properties.plane_crtc_id, 0)],
                None,
            )
        }
    };

    let description = CommitDescription {
        objects: vec![
            SerializedObject {
                object: objects.connector,
                kind: ObjectKind::Connector,
                old_crtc_id: Some(objects.old_crtc_id),
                props: connector_props,
            },
            SerializedObject {
                object: objects.crtc,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![
                    (properties.crtc_mode_id, mode_id),
                    (properties.common.active, active),
                ],
            },
            SerializedObject {
                object: objects.primary_plane,
                kind: ObjectKind::Plane,
                old_crtc_id: Some(objects.old_crtc_id),
                props: plane_props,
            },
        ],
        crtc_state: vec![CrtcPower {
            crtc_id: objects.crtc,
            old_active,
            new_active: active != 0,
        }],
        present_consumers: Vec::new(),
        page_flip_event: false,
        property_ids: properties.common,
    };

    Ok(BuiltClientModesetDescription {
        description,
        staged_projection,
    })
}
