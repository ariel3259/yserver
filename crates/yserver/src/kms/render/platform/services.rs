//! Construction-selected cursor, commit, and event operations for KMS.

use std::{rc::Rc, time::Instant};

use crate::{
    drm::{
        atomic::{AtomicCommitExecutor, DrmAtomicCommitExecutor},
        io::{DrmModeBlobIo, ModeBlobIo, RecordingModeBlobIo},
    },
    kms::{
        cursor_plane::ops::{
            CursorBufferIo, DrmCursorBufferIo, DrmLegacyCursorIo, LegacyCursorIo,
            RecordingCursorBufferIo, RecordingLegacyCursorIo,
        },
        hotplug::{DisplayHotplugSource, DrmDisplayHotplugSource, RecordingDisplayHotplugSource},
        render::platform::flip_events::{DrmPageFlipEventSource, PageFlipEventSource},
    },
};

/// Raw operations retained by a KMS backend. Production algorithms keep
/// constructing requests, scheduling cursor work, and retiring events.
#[derive(Clone)]
pub struct KmsServices {
    pub(crate) mode_blobs: Rc<dyn ModeBlobIo>,
    pub(crate) cursor_buffers: Rc<dyn CursorBufferIo>,
    pub(crate) legacy_cursor: Rc<dyn LegacyCursorIo>,
    pub(crate) atomic: Rc<dyn AtomicCommitExecutor>,
    pub(crate) page_flip_events: Rc<dyn PageFlipEventSource>,
    pub(crate) hotplug: Rc<dyn DisplayHotplugSource>,
    pub(crate) monotonic_clock: Rc<dyn Fn() -> Instant>,
    pub(crate) test_recorders: Option<Rc<KmsTestRecorders>>,
}

/// The subset needed by isolated scanout qualification workers. Worker code
/// constructs this pair locally because `Rc`-backed backend services cannot
/// cross the worker boundary.
pub(crate) struct KmsModesetServices {
    pub(crate) mode_blobs: Rc<dyn ModeBlobIo>,
    pub(crate) atomic: Rc<dyn AtomicCommitExecutor>,
}

impl KmsModesetServices {
    pub(crate) fn production() -> Self {
        Self {
            mode_blobs: Rc::new(DrmModeBlobIo),
            atomic: Rc::new(DrmAtomicCommitExecutor),
        }
    }
}

pub(crate) struct KmsTestRecorders {
    pub(crate) atomic: Rc<crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor>,
    pub(crate) mode_blobs: Rc<RecordingModeBlobIo>,
    pub(crate) cursor_buffer_ledger: Rc<crate::kms::cursor_plane::ops::CursorBufferLedger>,
    pub(crate) legacy_cursor: Rc<RecordingLegacyCursorIo>,
    pub(crate) page_flip_events:
        Rc<crate::kms::render::platform::flip_events::RecordingPageFlipEventSource>,
    pub(crate) hotplug: Rc<RecordingDisplayHotplugSource>,
}

impl KmsServices {
    pub(crate) fn modeset(&self) -> KmsModesetServices {
        KmsModesetServices {
            mode_blobs: Rc::clone(&self.mode_blobs),
            atomic: Rc::clone(&self.atomic),
        }
    }

    pub fn production() -> Self {
        Self {
            mode_blobs: Rc::new(DrmModeBlobIo),
            cursor_buffers: Rc::new(DrmCursorBufferIo),
            legacy_cursor: Rc::new(DrmLegacyCursorIo),
            atomic: Rc::new(DrmAtomicCommitExecutor),
            page_flip_events: Rc::new(DrmPageFlipEventSource),
            hotplug: Rc::new(DrmDisplayHotplugSource::new()),
            monotonic_clock: Rc::new(Instant::now),
            test_recorders: None,
        }
    }

    pub(crate) fn for_tests() -> Self {
        let atomic =
            Rc::new(crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default());
        let mode_blobs = Rc::new(RecordingModeBlobIo::default());
        let (cursor_buffers, cursor_buffer_ledger) = RecordingCursorBufferIo::new(256);
        let cursor_buffers = Rc::new(cursor_buffers);
        let legacy_cursor = Rc::new(RecordingLegacyCursorIo::default());
        let page_flip_events = Rc::new(
            crate::kms::render::platform::flip_events::RecordingPageFlipEventSource::default(),
        );
        let hotplug =
            Rc::new(RecordingDisplayHotplugSource::new().expect("recording hotplug socket pair"));
        let test_recorders = Rc::new(KmsTestRecorders {
            atomic: Rc::clone(&atomic),
            mode_blobs: Rc::clone(&mode_blobs),
            cursor_buffer_ledger,
            legacy_cursor: Rc::clone(&legacy_cursor),
            page_flip_events: Rc::clone(&page_flip_events),
            hotplug: Rc::clone(&hotplug),
        });
        Self {
            mode_blobs,
            cursor_buffers,
            legacy_cursor,
            atomic,
            page_flip_events,
            hotplug,
            monotonic_clock: Rc::new(Instant::now),
            test_recorders: Some(test_recorders),
        }
    }
}
