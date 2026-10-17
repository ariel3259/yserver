//! Per-frame cursor metadata retained across shared and copied scanout submits.

use crate::kms::render::{
    platform::{PlatformBackend, cursor_state::CursorPlaneUpdate},
    scene::{
        CursorAcceptedMode, CursorCommitReceipt, CursorFrameId, CursorFramePlan,
        CursorPrimaryContent,
    },
};
use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorCommitOwner {
    Scene(CursorFrameId),
}

pub(crate) struct StagedCursorFrame {
    pub(crate) owner: CursorCommitOwner,
    pub(crate) plan: CursorFramePlan,
    pub(crate) primary_cursor: Option<CursorPrimaryContent>,
    pub(crate) cursor_updates: Vec<CursorPlaneUpdate>,
    pub(crate) cursor_updates_reserved: bool,
}

pub(crate) fn stage_cursor_frame(
    platform: &mut PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
    plan: CursorFramePlan,
) -> io::Result<()> {
    let key = (output_idx, bo_idx);
    if platform.staged_cursor_frames.contains_key(&key) {
        return Err(io::Error::other(format!(
            "cursor frame already staged for output {output_idx} BO {bo_idx}"
        )));
    }
    platform.staged_cursor_frames.insert(
        key,
        StagedCursorFrame {
            owner: CursorCommitOwner::Scene(plan.frame),
            plan,
            primary_cursor: None,
            cursor_updates: Vec::new(),
            cursor_updates_reserved: false,
        },
    );
    Ok(())
}

pub(crate) fn cancel_cursor_frame(
    platform: &mut PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
    frame: CursorFrameId,
) {
    let key = (output_idx, bo_idx);
    if platform
        .staged_cursor_frames
        .get(&key)
        .is_some_and(|staged| staged.plan.frame == frame)
        && let Some(staged) = platform.staged_cursor_frames.remove(&key)
        && staged.cursor_updates_reserved
    {
        platform.reject_cursor_plane_update_reservations(&staged.cursor_updates);
    }
}

pub(crate) fn store_cursor_frame_updates(
    platform: &mut PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
    frame: CursorFrameId,
    updates: Vec<CursorPlaneUpdate>,
    reserved: bool,
) -> io::Result<()> {
    let staged = platform
        .staged_cursor_frames
        .get_mut(&(output_idx, bo_idx))
        .ok_or_else(|| io::Error::other("cursor frame plan was not staged"))?;
    if staged.plan.frame != frame {
        return Err(io::Error::other("cursor frame plan identity mismatch"));
    }
    staged.cursor_updates = updates;
    staged.cursor_updates_reserved = reserved;
    Ok(())
}

/// The copied job keeps its frame's cursor lease, but position remains live
/// until the final atomic request is built. Rebuild only the position delta
/// from the current pointer while retaining the frame-owned sprite lease.
pub(crate) fn cursor_frame_updates_for_submit(
    platform: &mut PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
    frame: CursorFrameId,
) -> io::Result<Vec<CursorPlaneUpdate>> {
    let (mut updates, assignment, sprite) = {
        let staged = platform
            .staged_cursor_frames
            .get(&(output_idx, bo_idx))
            .filter(|staged| staged.plan.frame == frame)
            .ok_or_else(|| io::Error::other("copied cursor frame identity mismatch"))?;
        (
            staged.cursor_updates.clone(),
            staged.plan.assignment,
            staged.plan.sprite.clone(),
        )
    };
    if updates.is_empty()
        && assignment == crate::kms::render::scene::CursorFrameAssignment::Hardware
    {
        // The frame plan retains its complete desired binding even when it
        // initially matched the applied state. Rebuild once at final submit
        // so motion queued while the copied render waited can join this
        // primary commit instead of being cleared as an empty delta settles.
        if let Some(update) =
            platform.prepare_scene_cursor_frame_update(output_idx, assignment, sprite.as_ref())?
        {
            platform.reserve_cursor_plane_updates(std::slice::from_ref(&update))?;
            updates.push(update);
            let staged = platform
                .staged_cursor_frames
                .get_mut(&(output_idx, bo_idx))
                .expect("copied cursor frame was validated before rebuilding its delta");
            staged.cursor_updates = updates.clone();
            staged.cursor_updates_reserved = true;
        }
    }
    let layout = platform
        .outputs
        .get(output_idx)
        .ok_or_else(|| io::Error::other("copied cursor output disappeared"))?;
    let pointer = platform.cursor_pointer_root;
    for update in &mut updates {
        let Some(mut next) = update.prepared.next else {
            continue;
        };
        if update.device_key != layout.key.device_key || update.crtc != layout.output.crtc {
            continue;
        }
        let (x, y) = super::cursor::cursor_root_to_crtc_local(
            pointer.0, pointer.1, layout.x, layout.y, next.hot_x, next.hot_y,
        );
        next.x = x;
        next.y = y;
        let device = platform
            .device_for_key(update.device_key)
            .ok_or_else(|| io::Error::other("copied cursor device disappeared"))?;
        let state = device.cursor.crtc_states.get(&update.crtc);
        let valid = state.is_some_and(|state| state.valid);
        let (width, height) = device
            .cursor
            .plane
            .as_ref()
            .map_or((0, 0), |plane| (plane.width(), plane.height()));
        update.prepared = super::cursor_commit::prepare_cursor_commit(
            next.properties,
            update.crtc,
            update.prepared.prior,
            valid,
            Some(next),
            width,
            height,
        );
        update.motion_timestamp = state.and_then(|state| state.intent_motion_timestamp);
    }
    Ok(updates)
}

pub(crate) fn mark_cursor_frame_updates_settled(
    platform: &mut PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
    frame: CursorFrameId,
) {
    if let Some(staged) = platform.staged_cursor_frames.get_mut(&(output_idx, bo_idx))
        && staged.plan.frame == frame
    {
        staged.cursor_updates_reserved = false;
    }
}

pub(crate) fn prepare_frame_cursor_commit(
    platform: &mut PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
    frame: CursorFrameId,
) -> io::Result<Vec<CursorPlaneUpdate>> {
    let (owner, assignment, sprite) = platform
        .staged_cursor_frames
        .get(&(output_idx, bo_idx))
        .map(|staged| {
            (
                staged.owner,
                staged.plan.assignment,
                staged.plan.sprite.clone(),
            )
        })
        .ok_or_else(|| io::Error::other("cursor frame plan was not staged"))?;
    if owner != CursorCommitOwner::Scene(frame) {
        return Err(io::Error::other("cursor frame plan identity mismatch"));
    }
    let updates = platform
        .prepare_scene_cursor_frame_update(output_idx, assignment, sprite.as_ref())?
        .into_iter()
        .collect::<Vec<_>>();
    // Scene plans stay in this slot-keyed table. The cursor-only scheduler
    // reads `CursorCrtcState.intent` and never consults staged frame owners.
    Ok(updates)
}

pub(crate) fn record_primary_cursor(
    platform: &mut PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
    frame: CursorFrameId,
    primary_cursor: CursorPrimaryContent,
) -> io::Result<()> {
    let staged = platform
        .staged_cursor_frames
        .get_mut(&(output_idx, bo_idx))
        .ok_or_else(|| io::Error::other("cursor frame plan was not staged"))?;
    if staged.plan.frame != frame {
        return Err(io::Error::other("cursor frame plan identity mismatch"));
    }
    staged.primary_cursor = Some(primary_cursor);
    Ok(())
}

pub(crate) fn accept_cursor_frame(
    platform: &mut PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
    frame: CursorFrameId,
    mode: CursorAcceptedMode,
    primary_cursor: CursorPrimaryContent,
) -> CursorCommitReceipt {
    let staged = platform.staged_cursor_frames.remove(&(output_idx, bo_idx));
    debug_assert!(staged.as_ref().is_some_and(|staged| {
        staged.owner == CursorCommitOwner::Scene(frame)
            && !staged.cursor_updates_reserved
            && staged
                .primary_cursor
                .is_none_or(|recorded| recorded == primary_cursor)
    }));
    CursorCommitReceipt {
        frame: Some(frame),
        mode,
        primary_cursor,
        needs_recompose: false,
    }
}

pub(crate) fn staged_cursor_frame_id(
    platform: &PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
) -> Option<CursorFrameId> {
    platform
        .staged_cursor_frames
        .get(&(output_idx, bo_idx))
        .map(|staged| staged.plan.frame)
}

pub(crate) fn cancel_cursor_frame_for_slot(
    platform: &mut PlatformBackend,
    output_idx: usize,
    bo_idx: usize,
) {
    if let Some(staged) = platform.staged_cursor_frames.remove(&(output_idx, bo_idx))
        && staged.cursor_updates_reserved
    {
        platform.reject_cursor_plane_update_reservations(&staged.cursor_updates);
    }
}

pub(crate) fn clear_cursor_frames(platform: &mut PlatformBackend) {
    let staged = std::mem::take(&mut platform.staged_cursor_frames);
    for frame in staged.into_values() {
        if frame.cursor_updates_reserved {
            platform.reject_cursor_plane_update_reservations(&frame.cursor_updates);
        }
    }
}

pub(crate) fn cancel_cursor_frames_for_output(platform: &mut PlatformBackend, output_idx: usize) {
    let staged = platform
        .staged_cursor_frames
        .keys()
        .filter(|(staged_output_idx, _)| *staged_output_idx == output_idx)
        .copied()
        .collect::<Vec<_>>();
    for key in staged {
        cancel_cursor_frame_for_slot(platform, key.0, key.1);
    }
}
