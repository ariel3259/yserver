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
    {
        platform.staged_cursor_frames.remove(&key);
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
    platform.staged_cursor_frames.remove(&(output_idx, bo_idx));
}

pub(crate) fn clear_cursor_frames(platform: &mut PlatformBackend) {
    platform.staged_cursor_frames.clear();
}

pub(crate) fn cancel_cursor_frames_for_output(platform: &mut PlatformBackend, output_idx: usize) {
    platform
        .staged_cursor_frames
        .retain(|(staged_output_idx, _), _| *staged_output_idx != output_idx);
}
