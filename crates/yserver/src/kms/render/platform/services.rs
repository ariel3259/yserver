//! Construction-selected cursor, commit, and event operations for KMS.

use std::{rc::Rc, time::Instant};

use crate::{
    drm::{
        atomic::{AtomicCommitExecutor, DrmAtomicCommitExecutor},
        io::{DrmModeBlobIo, ModeBlobIo, RecordingModeBlobIo},
    },
    kms::{
        cursor_plane::{
            CursorPlaneFactory, DrmCursorPlaneFactory,
            ops::{
                CursorBufferIo, DrmCursorBufferIo, DrmLegacyCursorIo, LegacyCursorIo,
                RecordingCursorBufferIo, RecordingLegacyCursorIo,
            },
        },
        hotplug::{DisplayHotplugSource, DrmDisplayHotplugSource, RecordingDisplayHotplugSource},
        render::platform::{
            connectors::{
                ConnectorSnapshotIo, DrmConnectorSnapshotIo, RecordingConnectorSnapshotIo,
            },
            flip_events::{DrmPageFlipEventSource, PageFlipEventSource},
        },
    },
};

/// Raw operations retained by a KMS backend. Production algorithms keep
/// constructing requests, scheduling cursor work, and retiring events.
#[derive(Clone)]
pub struct KmsServices {
    pub(crate) mode_blobs: Rc<dyn ModeBlobIo>,
    pub(crate) cursor_buffers: Rc<dyn CursorBufferIo>,
    pub(crate) legacy_cursor: Rc<dyn LegacyCursorIo>,
    pub(crate) cursor_plane_factory: Rc<dyn CursorPlaneFactory>,
    pub(crate) crtc_output_discovery: Rc<dyn CrtcOutputDiscovery>,
    pub(crate) atomic: Rc<dyn AtomicCommitExecutor>,
    pub(crate) page_flip_events: Rc<dyn PageFlipEventSource>,
    pub(crate) hotplug: Rc<dyn DisplayHotplugSource>,
    pub(crate) connector_snapshot: Rc<dyn ConnectorSnapshotIo>,
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
    #[cfg(test)]
    pub(crate) cursor_plane_factory: Rc<crate::kms::cursor_plane::RecordingCursorPlaneFactory>,
    #[cfg(test)]
    pub(crate) crtc_output_discovery: Rc<RecordingCrtcOutputDiscovery>,
    #[cfg(test)]
    pub(crate) output_power: Rc<super::power::RecordingOutputPowerExecutor>,
    pub(crate) page_flip_events:
        Rc<crate::kms::render::platform::flip_events::RecordingPageFlipEventSource>,
    pub(crate) hotplug: Rc<RecordingDisplayHotplugSource>,
    pub(crate) connector_snapshot: Rc<RecordingConnectorSnapshotIo>,
}

pub(crate) trait CrtcOutputDiscovery {
    fn discover(
        &self,
        device: &crate::drm::Device,
        connector_name: &str,
        reserved_routes: &[(
            ::drm::control::encoder::Handle,
            ::drm::control::crtc::Handle,
            ::drm::control::plane::Handle,
        )],
    ) -> std::io::Result<crate::platform::drm::Output>;
}

pub(crate) struct DrmCrtcOutputDiscovery;

impl CrtcOutputDiscovery for DrmCrtcOutputDiscovery {
    fn discover(
        &self,
        device: &crate::drm::Device,
        connector_name: &str,
        reserved_routes: &[(
            ::drm::control::encoder::Handle,
            ::drm::control::crtc::Handle,
            ::drm::control::plane::Handle,
        )],
    ) -> std::io::Result<crate::platform::drm::Output> {
        crate::drm::modeset::discover_output_for_connector(device, connector_name, reserved_routes)
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingCrtcOutputDiscovery {
    outputs: std::cell::RefCell<std::collections::VecDeque<crate::platform::drm::Output>>,
    calls: std::cell::Cell<usize>,
}

#[cfg(test)]
impl RecordingCrtcOutputDiscovery {
    pub(crate) fn queue_output(&self, output: crate::platform::drm::Output) {
        self.outputs.borrow_mut().push_back(output);
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.get()
    }
}

#[cfg(test)]
impl CrtcOutputDiscovery for RecordingCrtcOutputDiscovery {
    fn discover(
        &self,
        _device: &crate::drm::Device,
        connector_name: &str,
        _reserved_routes: &[(
            ::drm::control::encoder::Handle,
            ::drm::control::crtc::Handle,
            ::drm::control::plane::Handle,
        )],
    ) -> std::io::Result<crate::platform::drm::Output> {
        self.calls.set(self.calls.get() + 1);
        self.outputs.borrow_mut().pop_front().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no recorded CRTC output discovery for {connector_name}"),
            )
        })
    }
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
            cursor_plane_factory: Rc::new(DrmCursorPlaneFactory),
            crtc_output_discovery: Rc::new(DrmCrtcOutputDiscovery),
            atomic: Rc::new(DrmAtomicCommitExecutor),
            page_flip_events: Rc::new(DrmPageFlipEventSource),
            hotplug: Rc::new(DrmDisplayHotplugSource::new()),
            connector_snapshot: Rc::new(DrmConnectorSnapshotIo),
            monotonic_clock: Rc::new(Instant::now),
            test_recorders: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        let atomic =
            Rc::new(crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default());
        let crtc_output_discovery = Rc::new(RecordingCrtcOutputDiscovery::default());
        let output_power = Rc::new(super::power::RecordingOutputPowerExecutor::default());
        let mode_blobs = Rc::new(RecordingModeBlobIo::default());
        let (cursor_buffers, cursor_buffer_ledger) = RecordingCursorBufferIo::new(256);
        let cursor_buffers = Rc::new(cursor_buffers);
        let legacy_cursor = Rc::new(RecordingLegacyCursorIo::default());
        let cursor_plane_factory =
            Rc::new(crate::kms::cursor_plane::RecordingCursorPlaneFactory::default());
        let page_flip_events = Rc::new(
            crate::kms::render::platform::flip_events::RecordingPageFlipEventSource::default(),
        );
        let hotplug =
            Rc::new(RecordingDisplayHotplugSource::new().expect("recording hotplug socket pair"));
        let connector_snapshot = Rc::new(RecordingConnectorSnapshotIo::new());
        let test_recorders = Rc::new(KmsTestRecorders {
            atomic: Rc::clone(&atomic),
            mode_blobs: Rc::clone(&mode_blobs),
            cursor_buffer_ledger,
            legacy_cursor: Rc::clone(&legacy_cursor),
            #[cfg(test)]
            cursor_plane_factory: Rc::clone(&cursor_plane_factory),
            crtc_output_discovery: Rc::clone(&crtc_output_discovery),
            output_power,
            page_flip_events: Rc::clone(&page_flip_events),
            hotplug: Rc::clone(&hotplug),
            connector_snapshot: Rc::clone(&connector_snapshot),
        });
        Self {
            mode_blobs,
            cursor_buffers,
            legacy_cursor,
            cursor_plane_factory,
            crtc_output_discovery,
            atomic,
            page_flip_events,
            hotplug,
            connector_snapshot,
            monotonic_clock: Rc::new(Instant::now),
            test_recorders: Some(test_recorders),
        }
    }

    #[cfg(not(test))]
    pub(crate) fn for_tests() -> Self {
        let atomic =
            Rc::new(crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default());
        let mode_blobs = Rc::new(RecordingModeBlobIo::default());
        let (cursor_buffers, _) = RecordingCursorBufferIo::new(256);
        let cursor_buffers = Rc::new(cursor_buffers);
        let legacy_cursor = Rc::new(RecordingLegacyCursorIo::default());
        let page_flip_events = Rc::new(
            crate::kms::render::platform::flip_events::RecordingPageFlipEventSource::default(),
        );
        let hotplug =
            Rc::new(RecordingDisplayHotplugSource::new().expect("recording hotplug socket pair"));
        Self {
            mode_blobs,
            cursor_buffers,
            legacy_cursor,
            cursor_plane_factory: Rc::new(DrmCursorPlaneFactory),
            crtc_output_discovery: Rc::new(DrmCrtcOutputDiscovery),
            atomic,
            page_flip_events,
            hotplug,
            connector_snapshot: Rc::new(RecordingConnectorSnapshotIo::new()),
            monotonic_clock: Rc::new(Instant::now),
            test_recorders: None,
        }
    }
}
