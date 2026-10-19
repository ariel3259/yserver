//! Single classification of failed atomic commits that carried cursor fields.
//!
//! The atomic sink attributes a rejected request to the cursor only when
//! `TEST_ONLY` rejects it as sent and accepts it without the cursor fields
//! (`drm/atomic.rs`). Every submit path that can fail with cursor fields asks
//! [`classify_cursor_failure`] what recovery the failure calls for, so no path
//! keeps its own parallel decision.

use std::io;

use super::{
    CrtcKey,
    cursor_state::{CursorFallbackState, CursorPlaneUpdate, CursorQualification, SpriteKey},
};
use crate::kms::render::scene::CursorFrameAssignment;

/// What the failure says about the request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorFailureClass {
    /// `EBUSY`: another commit is pending on the CRTC. Never a cursor failure.
    Busy,
    /// The cursor fields caused the rejection.
    CursorProperties,
    /// Anything else: primary, fence or allocation failures of the frame.
    Frame,
}

/// Which submit path produced the failed request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorFailureContext {
    /// A cursor-only commit. Every property it carries is a cursor property.
    CursorOnly,
    /// A scene frame that hides the plane because its output starts painting
    /// the software cursor.
    SceneToSoftware,
    /// A scene frame that attaches the plane because its output stops painting
    /// the software cursor.
    SceneToHardware,
    /// A scene frame that only changes a plane that stays attached.
    SceneHardwareOnly,
    /// A held direct frame whose cursor fields are hardware-only.
    DirectHardwareOnly,
}

/// Recovery a failed submit path must take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CursorRecoveryAction {
    /// `EBUSY`: wait for the outstanding flip, then retry.
    WaitForFlip,
    /// Cursor-only `EINVAL`: send an explicit detach, then fall back to software.
    DetachThenSoftware,
    /// Software-cursor frame rejected while hiding: retry it with an explicit detach.
    RetryFrameWithDetach,
    /// Frame rejected because of its cursor fields: retry it without them.
    RetryFrameWithoutCursor,
    /// Not a cursor failure: today's flip error path.
    FlipError,
}

impl CursorRecoveryAction {
    /// Whether the failed request's cursor fields own the failure, so the
    /// submit path settles its cursor leases as rejected.
    pub(crate) fn is_cursor_failure(self) -> bool {
        !matches!(self, Self::WaitForFlip | Self::FlipError)
    }
}

pub(crate) fn failure_class(error: &io::Error, cursor_attributed: bool) -> CursorFailureClass {
    if error.raw_os_error() == Some(libc::EBUSY) {
        CursorFailureClass::Busy
    } else if cursor_attributed {
        CursorFailureClass::CursorProperties
    } else {
        CursorFailureClass::Frame
    }
}

/// Classify one failed atomic commit. `cursor_attributed` is the sink's
/// attribution for the request (`AtomicCommitSink::take_last_cursor_rejection`).
pub(crate) fn classify_cursor_failure(
    error: &io::Error,
    cursor_attributed: bool,
    context: CursorFailureContext,
) -> CursorRecoveryAction {
    match failure_class(error, cursor_attributed) {
        CursorFailureClass::Busy => CursorRecoveryAction::WaitForFlip,
        CursorFailureClass::Frame => CursorRecoveryAction::FlipError,
        CursorFailureClass::CursorProperties => match context {
            CursorFailureContext::CursorOnly => CursorRecoveryAction::DetachThenSoftware,
            CursorFailureContext::SceneToSoftware => CursorRecoveryAction::RetryFrameWithDetach,
            CursorFailureContext::SceneToHardware | CursorFailureContext::SceneHardwareOnly => {
                CursorRecoveryAction::RetryFrameWithoutCursor
            }
            CursorFailureContext::DirectHardwareOnly => {
                CursorRecoveryAction::RetryFrameWithoutCursor
            }
        },
    }
}

/// A rejected hardware-only update can ride a primary retry without cursor
/// fields. Once that primary is accepted, the normal cursor lane owes an
/// explicit detach before software ownership can begin.
pub(crate) fn retry_riding_frame_without_cursor(
    action: CursorRecoveryAction,
    context: CursorFailureContext,
    updates: &[CursorPlaneUpdate],
) -> bool {
    action == CursorRecoveryAction::RetryFrameWithoutCursor
        && matches!(
            context,
            CursorFailureContext::SceneToSoftware
                | CursorFailureContext::SceneToHardware
                | CursorFailureContext::SceneHardwareOnly
                | CursorFailureContext::DirectHardwareOnly
        )
        && !updates.is_empty()
        && updates
            .iter()
            .all(|update| update.owner == super::cursor_frames::CursorCommitOwner::HardwareOnly)
}

/// Retain the refused key separately from the newest wanted sprite. Eligibility
/// compares them so a newer key can qualify without forgetting the refusal.
pub(crate) fn software_for_rejected_sprite(rejected_key: Option<SpriteKey>) -> CursorFallbackState {
    CursorFallbackState::Software { rejected_key }
}

/// Record the submitted sprite's qualification key for the epoch that owned
/// the rejected atomic request.
pub(crate) fn record_rejected_qualification(
    qualification: &mut CursorQualification,
    topology_epoch: u64,
    submitted_sprite_key: Option<SpriteKey>,
) {
    qualification.note_rejected(topology_epoch, submitted_sprite_key);
}

/// Context of a scene frame from the cursor transitions it carries.
pub(crate) fn scene_failure_context(updates: &[CursorPlaneUpdate]) -> CursorFailureContext {
    if !updates.is_empty()
        && updates
            .iter()
            .all(|update| update.owner == super::cursor_frames::CursorCommitOwner::HardwareOnly)
    {
        CursorFailureContext::SceneHardwareOnly
    } else if updates.iter().any(|update| update.prepared.next.is_none()) {
        CursorFailureContext::SceneToSoftware
    } else if updates.iter().any(|update| update.prepared.prior.is_none()) {
        CursorFailureContext::SceneToHardware
    } else {
        CursorFailureContext::SceneHardwareOnly
    }
}

/// Classify a scene rejection using the content assignment captured for that
/// frame. A hidden or cursorless primary has no software cursor to protect, so
/// it must not take the HW-to-software explicit-detach retry.
pub(crate) fn scene_frame_failure_context(
    assignment: CursorFrameAssignment,
    updates: &[CursorPlaneUpdate],
) -> CursorFailureContext {
    if !updates.is_empty()
        && updates
            .iter()
            .all(|update| update.owner == super::cursor_frames::CursorCommitOwner::HardwareOnly)
    {
        CursorFailureContext::SceneHardwareOnly
    } else {
        match assignment {
            CursorFrameAssignment::Software { .. } => CursorFailureContext::SceneToSoftware,
            CursorFrameAssignment::Hidden | CursorFrameAssignment::CursorlessReplacement => {
                CursorFailureContext::SceneHardwareOnly
            }
            CursorFrameAssignment::Hardware => scene_failure_context(updates),
        }
    }
}

/// A cursorless retry of a show frame accepted only the primary. The output
/// remains software-owned and needs one composed frame that includes the
/// software cursor; a hardware-only retry does not change primary ownership.
pub(crate) fn retry_requires_software_recompose(
    action: CursorRecoveryAction,
    context: CursorFailureContext,
) -> bool {
    action == CursorRecoveryAction::RetryFrameWithoutCursor
        && context == CursorFailureContext::SceneToHardware
}

/// Observer for recovery decisions that keep hardware ownership in place.
pub(crate) trait CursorRecoveryReporter {
    /// A detach of `key`'s cursor plane was rejected while `epoch` was the
    /// device's topology epoch. The accepted hardware binding stays.
    fn retained_after_detach_rejection(&self, key: CrtcKey, epoch: u64, error: &io::Error);
}

/// Production reporter: one warning per observation.
pub(crate) struct LogCursorRecoveryReporter;

impl CursorRecoveryReporter for LogCursorRecoveryReporter {
    fn retained_after_detach_rejection(&self, key: CrtcKey, epoch: u64, error: &io::Error) {
        log::warn!(
            "render cursor atomic detach: device {} CRTC {:?} topology epoch {epoch} rejected ({error}); retaining the hardware cursor binding",
            key.device_key,
            key.crtc
        );
    }
}

/// Counting reporter for tests. Injected at the same services seam the
/// production logger uses.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingCursorRecoveryReporter {
    calls: std::cell::Cell<u64>,
}

#[cfg(test)]
impl RecordingCursorRecoveryReporter {
    pub(crate) fn calls(&self) -> u64 {
        self.calls.get()
    }
}

#[cfg(test)]
impl CursorRecoveryReporter for RecordingCursorRecoveryReporter {
    fn retained_after_detach_rejection(&self, _key: CrtcKey, _epoch: u64, _error: &io::Error) {
        self.calls.set(self.calls.get().saturating_add(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn einval() -> io::Error {
        io::Error::from_raw_os_error(libc::EINVAL)
    }

    #[test]
    // Mutation (run and seen to fail): M10 records the newest key as refused, so a newer key never qualifies.
    fn software_for_rejected_sprite_retains_the_refused_key() {
        let refused = Some((1, 1, 0, 0));
        assert_eq!(
            software_for_rejected_sprite(refused),
            CursorFallbackState::Software {
                rejected_key: refused
            },
        );
    }

    #[test]
    // Mutation (run and seen to fail): M5b makes EBUSY skip the Busy class, so the Busy row no longer waits.
    // Mutation (run and seen to fail): M8 ignores the attribution flag, so the Frame row becomes a cursor retry.
    fn classifier_table_separates_busy_cursor_and_frame_failures() {
        assert_eq!(
            classify_cursor_failure(
                &io::Error::from_raw_os_error(libc::EBUSY),
                true,
                CursorFailureContext::CursorOnly,
            ),
            CursorRecoveryAction::WaitForFlip,
        );
        assert_eq!(
            classify_cursor_failure(&einval(), true, CursorFailureContext::CursorOnly),
            CursorRecoveryAction::DetachThenSoftware,
        );
        assert_eq!(
            classify_cursor_failure(&einval(), false, CursorFailureContext::SceneToSoftware),
            CursorRecoveryAction::FlipError,
        );
        assert_eq!(
            classify_cursor_failure(&einval(), true, CursorFailureContext::SceneToSoftware),
            CursorRecoveryAction::RetryFrameWithDetach,
        );
        assert!(!CursorRecoveryAction::WaitForFlip.is_cursor_failure());
        assert!(!CursorRecoveryAction::FlipError.is_cursor_failure());
        assert!(CursorRecoveryAction::DetachThenSoftware.is_cursor_failure());
    }
}
