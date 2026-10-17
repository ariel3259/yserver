//! Full-state cursor atomic request construction and submission.

use crate::{
    drm::atomic::{AtomicCommitRequest, AtomicPropertyUpdate},
    kms::{
        cursor_plane::topology::CursorPlaneProperties,
        render::platform::cursor_state::{
            CursorBinding, CursorPlaneUpdate, CursorPropertyUpdate, PreparedCursorCommit,
        },
    },
};

pub(crate) fn prepare_cursor_commit(
    properties: CursorPlaneProperties,
    crtc: ::drm::control::crtc::Handle,
    applied: Option<CursorBinding>,
    valid: bool,
    next: Option<CursorBinding>,
    width: u32,
    height: u32,
) -> PreparedCursorCommit {
    let mut updates = Vec::new();
    let mut add = |property, value| {
        updates.push(CursorPropertyUpdate {
            object: properties.plane.into(),
            property,
            value,
        });
    };
    match next {
        Some(binding) => {
            let old = applied;
            let attach = !valid || old.is_none();
            let new_framebuffer = !valid
                || old.is_none_or(|previous| {
                    previous.lease != binding.lease || previous.framebuffer != binding.framebuffer
                });
            // Sprite buffers are allocated at the driver's fixed cursor
            // capacity, so replacing an image does not change its geometry.
            // Only attachment or invalidation needs a full geometry restore.
            let geometry = attach;
            if attach || new_framebuffer {
                add(properties.fb_id, u64::from(binding.framebuffer));
            }
            if attach {
                add(properties.crtc_id, u64::from(u32::from(crtc)));
            }
            if !valid || old.is_none_or(|previous| previous.x != binding.x) {
                add(properties.crtc_x, i64::from(binding.x) as u64);
            }
            if !valid || old.is_none_or(|previous| previous.y != binding.y) {
                add(properties.crtc_y, i64::from(binding.y) as u64);
            }
            if geometry {
                add(properties.src_x, 0);
                add(properties.src_y, 0);
                add(properties.src_w, u64::from(width) << 16);
                add(properties.src_h, u64::from(height) << 16);
                add(properties.crtc_w, u64::from(width));
                add(properties.crtc_h, u64::from(height));
            }
            if (!valid || old.is_none_or(|previous| previous.hot_x != binding.hot_x))
                && let Some(property) = properties.hotspot_x
            {
                add(property, u64::from(binding.hot_x));
            }
            if (!valid || old.is_none_or(|previous| previous.hot_y != binding.hot_y))
                && let Some(property) = properties.hotspot_y
            {
                add(property, u64::from(binding.hot_y));
            }
        }
        None => {
            if !valid || applied.is_some() {
                add(properties.fb_id, 0);
                add(properties.crtc_id, 0);
            }
        }
    }
    PreparedCursorCommit {
        prior: applied,
        next,
        updates,
    }
}

pub(crate) fn request_for_cursor_commit(prepared: &PreparedCursorCommit) -> AtomicCommitRequest {
    let mut request = AtomicCommitRequest::new();
    for update in &prepared.updates {
        let AtomicPropertyUpdate {
            object,
            property,
            value,
        } = update.atomic_update();
        request.add_cursor_property(object, property, value);
    }
    request
}

pub(crate) fn add_cursor_plane_updates(
    request: &mut AtomicCommitRequest,
    updates: &[CursorPlaneUpdate],
) {
    for update in updates {
        for property in &update.prepared.updates {
            let atomic = (*property).atomic_update();
            request.add_cursor_property(atomic.object, atomic.property, atomic.value);
        }
    }
}

/// Append cursor deltas only to a Vsync scene request. Async scene requests
/// are cursorless by contract, even when the frame was prepared with a shared
/// route cursor plan.
pub(crate) fn add_vsync_scene_cursor_updates(
    request: &mut AtomicCommitRequest,
    updates: &[CursorPlaneUpdate],
    timing: crate::drm::async_flip::FlipTiming,
) -> bool {
    if timing != crate::drm::async_flip::FlipTiming::Vsync {
        return false;
    }
    add_cursor_plane_updates(request, updates);
    !updates.is_empty()
}

pub(crate) fn request_for_cursor_updates(updates: &[CursorPlaneUpdate]) -> AtomicCommitRequest {
    let mut request = AtomicCommitRequest::new();
    add_cursor_plane_updates(&mut request, updates);
    request
}
