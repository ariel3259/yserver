use super::CursorEntry;

/// Monotonic identity of a cursor-bearing frame within one output lifetime.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CursorFrameId(pub u64);

/// The cursor ownership requested by a rendered scene frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorFrameAssignment {
    Hardware,
    Software {
        pos: (i32, i32),
    },
    Hidden,
    /// This primary must stay cursorless while an already accepted hardware
    /// binding is retained after a refused detach.
    CursorlessReplacement,
}

/// Cursor inputs captured when the scene builds one output frame.
#[derive(Clone, Debug)]
pub struct CursorFramePlan {
    pub frame: CursorFrameId,
    pub assignment: CursorFrameAssignment,
    /// The accepted output already owns the hardware cursor and this frame
    /// does not change that ownership. Its cursor delta comes from the live
    /// hardware intent, not from a frame-captured sprite snapshot.
    pub hardware_only: bool,
    pub sprite: Option<CursorEntry>,
}

/// Cursor ownership the output accepted with a commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorAcceptedMode {
    Hardware,
    Software,
    Hidden,
}

/// Cursor pixels actually included in the accepted primary framebuffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorPrimaryContent {
    Cursorless,
    Software {
        pos: (i32, i32),
        record_version: u64,
    },
}

/// Receipt for the cursor state accepted with a primary or cursor-only commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorCommitReceipt {
    /// `None` means an external cursor update without a rendered frame.
    pub frame: Option<CursorFrameId>,
    pub mode: CursorAcceptedMode,
    pub primary_cursor: CursorPrimaryContent,
    pub needs_recompose: bool,
}

impl CursorCommitReceipt {
    /// Mark an accepted receipt whose primary must be replaced at the next
    /// scene opportunity before its cursor ownership is fully presented.
    pub const fn with_recompose_after_acceptance(mut self) -> Self {
        self.needs_recompose = true;
        self
    }
}
