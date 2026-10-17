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
    let prior = state.applied.take();
    state.resume_visible |= prior.is_some();
    state.lifecycle_pending = prior;
    state.valid = true;
    state.intent = None;
    state.intent_motion_timestamp = None;
}
