//! Accepted cursor-plane state and the ownership carried by its commits.

use crate::{drm::atomic::AtomicPropertyUpdate, kms::cursor_plane::sprites::SpriteBufferLease};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CursorBinding {
    pub(crate) properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    pub(crate) lease: SpriteBufferLease,
    pub(crate) framebuffer: u32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) hot_x: u16,
    pub(crate) hot_y: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorIntent {
    Show(CursorBinding),
    Hide,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CursorPropertyUpdate {
    pub(crate) object: ::drm::control::RawResourceHandle,
    pub(crate) property: ::drm::control::property::Handle,
    pub(crate) value: u64,
}

/// One CRTC's prepared full-state delta for a producer-owned atomic commit.
/// The accepted `PreparedCursorCommit` is shared by cursor-only, scene, direct,
/// and unflip commit paths so every owner settles the same framebuffer leases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CursorPlaneUpdate {
    pub(crate) owner: super::cursor_frames::CursorCommitOwner,
    pub(crate) device_key: crate::platform::drm::DrmDeviceKey,
    pub(crate) crtc: ::drm::control::crtc::Handle,
    pub(crate) prepared: PreparedCursorCommit,
    pub(crate) motion_timestamp: Option<Instant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedCursorCommit {
    pub(crate) prior: Option<CursorBinding>,
    pub(crate) next: Option<CursorBinding>,
    pub(crate) updates: Vec<CursorPropertyUpdate>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CursorCrtcState {
    /// Last state accepted by the kernel, regardless of whether its event has
    /// retired. This is the source for all visibility decisions.
    pub(crate) applied: Option<CursorBinding>,
    pub(crate) valid: bool,
    pub(crate) resume_visible: bool,
    /// On-screen owner removed by a successful blocking disable. Kept until
    /// the old per-CRTC events have been drained.
    pub(crate) lifecycle_pending: Option<CursorBinding>,
    pub(crate) intent: Option<CursorIntent>,
    /// Pointer timestamp belongs to the current full-state intent and follows
    /// it through any event-gated retry.
    pub(crate) intent_motion_timestamp: Option<Instant>,
    /// A replacement keeps both leases owned until the corresponding event.
    pub(crate) in_flight: Option<PreparedCursorCommit>,
}

impl CursorPropertyUpdate {
    pub(crate) fn atomic_update(self) -> AtomicPropertyUpdate {
        AtomicPropertyUpdate {
            object: self.object,
            property: self.property,
            value: self.value,
        }
    }
}
