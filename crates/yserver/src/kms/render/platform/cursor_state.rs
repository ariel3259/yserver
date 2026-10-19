//! Accepted cursor-plane state and the ownership carried by its commits.

use crate::{drm::atomic::AtomicPropertyUpdate, kms::cursor_plane::sprites::SpriteBufferLease};
use std::{collections::HashSet, time::Instant};

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
    /// Whether this delta belongs to a scene-owned cursor mode transition.
    /// Failed transitions stay with the primary retry; stable HW updates can
    /// still use the cursor-only lane after a primary submit failure.
    pub(crate) scene_owned_intent: bool,
    pub(crate) device_key: crate::platform::drm::DrmDeviceKey,
    pub(crate) crtc: ::drm::control::crtc::Handle,
    /// Sprite identity associated with this request, captured before submit.
    /// Copied frames can settle after the device has selected a newer sprite.
    pub(crate) sprite_key: Option<SpriteKey>,
    /// Topology epoch at which this request was prepared. A later rejection
    /// cannot poison qualification after the device has changed topology.
    pub(crate) topology_epoch: u64,
    pub(crate) prepared: PreparedCursorCommit,
    pub(crate) motion_timestamp: Option<Instant>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedCursorCommit {
    pub(crate) prior: Option<CursorBinding>,
    pub(crate) next: Option<CursorBinding>,
    pub(crate) updates: Vec<CursorPropertyUpdate>,
}

/// Submitted sprite parameters used when settling a rejection. Pixel and
/// animation changes never change this identity.
pub(crate) type SpriteKey = (u16, u16, u16, u16);

/// Hardware qualification depends on geometry and hotspot, never image bytes
/// or a cursor record's animation/version identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CursorQualificationKey {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) hot_x: u16,
    pub(crate) hot_y: u16,
}

impl From<SpriteKey> for CursorQualificationKey {
    fn from((width, height, hot_x, hot_y): SpriteKey) -> Self {
        Self {
            width,
            height,
            hot_x,
            hot_y,
        }
    }
}

/// Rejected hardware configurations for one CRTC in one device topology
/// epoch. A changed epoch is the sole operation that forgets prior keys.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CursorQualification {
    topology_epoch: u64,
    rejected_keys: HashSet<CursorQualificationKey>,
}

impl CursorQualification {
    pub(crate) fn rejected_in_epoch(&self, epoch: u64, sprite_key: Option<SpriteKey>) -> bool {
        self.topology_epoch == epoch
            && sprite_key.is_some_and(|key| self.rejected_keys.contains(&key.into()))
    }

    pub(crate) fn note_rejected(&mut self, epoch: u64, sprite_key: Option<SpriteKey>) {
        self.change_epoch(epoch);
        if let Some(sprite_key) = sprite_key {
            self.rejected_keys.insert(sprite_key.into());
        }
    }

    pub(crate) fn change_epoch(&mut self, epoch: u64) {
        if self.topology_epoch != epoch {
            self.topology_epoch = epoch;
            self.rejected_keys.clear();
        }
    }
}

/// Recovery state of one CRTC after a cursor-only commit was rejected with a
/// cursor-caused error (spec §3). Hardware ownership is `applied`; this state
/// only says what recovery is owed or suppressed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CursorFallbackState {
    /// No cursor-only rejection is outstanding.
    #[default]
    Hardware,
    /// The rejected cursor-only commit owes an explicit detach
    /// (`FB_ID=0`, `CRTC_ID=0`). The detach is the intent's commit, so an
    /// `EBUSY` wait retries it after the flip. `rejected_key` is the sprite the
    /// refused commit carried, captured at refusal.
    DetachPending { rejected_key: Option<SpriteKey> },
    /// A failed-show retry or explicit detach was accepted: the output paints
    /// the software cursor. `rejected_key` is the refused sprite; only a
    /// different key requalifies hardware, and a topology change clears it.
    Software { rejected_key: Option<SpriteKey> },
    /// The explicit detach was rejected: the accepted binding stays bound and
    /// its cursor-only work is suppressed until a topology change or a
    /// successful lifecycle disable.
    RetainHardware,
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
    /// A scene-owned cursor delta must stay with its primary frame even if
    /// that frame enters submit backoff. A fresh independent cursor intent
    /// clears this marker when it is queued.
    pub(crate) intent_scene_owned: bool,
    /// A rejected scene cursor delta cannot be replayed as an idle cursor-only
    /// commit. A fresh cursor producer clears this until the primary retries.
    pub(crate) retry_backoff_cursor_suppressed: bool,
    /// Pointer timestamp belongs to the current full-state intent and follows
    /// it through any event-gated retry.
    pub(crate) intent_motion_timestamp: Option<Instant>,
    /// A replacement keeps both leases owned until the corresponding event.
    pub(crate) in_flight: Option<PreparedCursorCommit>,
    /// Cursor-only rejection recovery owed or retained for this CRTC.
    pub(crate) fallback: CursorFallbackState,
    /// Per-CRTC set of refused parameter keys in its current device epoch.
    pub(crate) qualification: CursorQualification,
    /// The frame carrying a software cursor was refused after both its hide
    /// and explicit detach failed. The renderer consumes this one-shot signal
    /// to recompose without software cursor pixels.
    pub(crate) discarded_software_scene_frame: bool,
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
