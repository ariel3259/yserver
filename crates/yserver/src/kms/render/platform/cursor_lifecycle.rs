//! Blocking lifecycle cursor detach helpers.

use crate::kms::cursor_plane::topology::CursorPlaneProperties;

impl From<CursorPlaneProperties> for crate::drm::atomic::AtomicCursorPlaneDetach {
    fn from(properties: CursorPlaneProperties) -> Self {
        Self {
            plane: properties.plane,
            fb_id: properties.fb_id,
            crtc_id: properties.crtc_id,
        }
    }
}

pub(crate) fn accept_blocking_detach(state: &mut super::cursor_state::CursorCrtcState) {
    // The accepted disable unbinds the plane, so a retained or pending detach
    // is settled. A software CRTC keeps its rejected-key record until topology.
    if matches!(
        state.fallback,
        super::cursor_state::CursorFallbackState::DetachPending { .. }
            | super::cursor_state::CursorFallbackState::RetainHardware
    ) {
        state.fallback = super::cursor_state::CursorFallbackState::Hardware;
    }
    let prior = state.applied.take();
    state.resume_visible |= prior.is_some();
    state.lifecycle_pending = prior.or(state.lifecycle_pending);
    state.valid = true;
    state.intent = None;
    state.intent_scene_owned = false;
    state.intent_motion_timestamp = None;
}

/// Forget whether the cached comparison describes current kernel state while
/// retaining every accepted framebuffer owner. A modeset or master transition
/// can invalidate the comparison without proving that the old binding stopped
/// using its framebuffer.
pub(crate) fn invalidate_applied_comparison(state: &mut super::cursor_state::CursorCrtcState) {
    state.valid = false;
}
