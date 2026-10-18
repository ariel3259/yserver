//! Construction-selected cursor, commit, and event operations for KMS.

use std::{
    os::fd::{BorrowedFd, OwnedFd},
    rc::Rc,
    time::Instant,
};

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
            flip_events::{
                DrmPageFlipEventSource, PageFlipEventSource, PollRenderCompletionReadiness,
                RenderCompletionReadiness,
            },
        },
        vk::scanout::CopiedScanoutPool,
    },
};

#[cfg(test)]
use crate::kms::render::platform::flip_events::RecordingRenderCompletionReadiness;

/// Submits the sink-side copy for a real copied-render completion. This
/// construction-selected operation boundary lets tests inject a copy failure
/// without replacing rendering, synchronization, or Backend completion flow.
pub(crate) trait CopiedScanoutCopySubmitter {
    fn submit_copy(
        &self,
        pool: &mut CopiedScanoutPool,
        bo_idx: usize,
        render_completion: Option<OwnedFd>,
    ) -> std::io::Result<Option<OwnedFd>>;
}

pub(crate) struct VulkanCopiedScanoutCopySubmitter;

impl CopiedScanoutCopySubmitter for VulkanCopiedScanoutCopySubmitter {
    fn submit_copy(
        &self,
        pool: &mut CopiedScanoutPool,
        bo_idx: usize,
        render_completion: Option<OwnedFd>,
    ) -> std::io::Result<Option<OwnedFd>> {
        pool.submit_copy(bo_idx, render_completion)
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingCopiedScanoutCopySubmitter {
    fail_next: std::cell::Cell<bool>,
    attempts: std::cell::Cell<u64>,
    failures: std::cell::Cell<u64>,
}

#[cfg(test)]
impl RecordingCopiedScanoutCopySubmitter {
    pub(crate) fn fail_next(&self) {
        self.fail_next.set(true);
    }

    pub(crate) fn attempts(&self) -> u64 {
        self.attempts.get()
    }

    pub(crate) fn failures(&self) -> u64 {
        self.failures.get()
    }
}

#[cfg(test)]
impl CopiedScanoutCopySubmitter for RecordingCopiedScanoutCopySubmitter {
    fn submit_copy(
        &self,
        pool: &mut CopiedScanoutPool,
        bo_idx: usize,
        render_completion: Option<OwnedFd>,
    ) -> std::io::Result<Option<OwnedFd>> {
        self.attempts.set(self.attempts.get().saturating_add(1));
        if self.fail_next.replace(false) {
            self.failures.set(self.failures.get().saturating_add(1));
            return Err(std::io::Error::from_raw_os_error(libc::EIO));
        }
        pool.submit_copy(bo_idx, render_completion)
    }
}

/// The low-level DRM `TEST_ONLY` probe used after Backend validates the
/// drawable metadata, output geometry, and cursor state. Tests replace only
/// this device operation; probe eligibility and accepted-shape publication
/// stay on the same Backend path.
pub(crate) trait DirectScanoutTestOnlyExecutor {
    fn source(
        &self,
        source_id: crate::kms::render::store::DrawableId,
        drawable: Option<&crate::kms::render::store::Drawable>,
    ) -> std::io::Result<Option<DirectScanoutProbeSource>>;

    #[allow(clippy::too_many_arguments)]
    fn test_only(
        &self,
        device: Rc<crate::drm::Device>,
        dma_buf: BorrowedFd<'_>,
        width: u32,
        height: u32,
        fourcc_code: u32,
        modifier: u64,
        offset: u64,
        pitch: u32,
        atomic_commit_sink: &crate::drm::atomic::AtomicCommitSink,
        planes: &[crate::drm::modeset::DirectScanoutPlaneState<'_>],
        cursor_updates: &[crate::kms::render::platform::cursor_state::CursorPlaneUpdate],
    ) -> std::io::Result<crate::drm::modeset::DirectScanoutTestResult>;
}

pub(crate) struct DirectScanoutProbeSource {
    pub(crate) metadata: yserver_render::kms::render::store::ImportedDmabufMetadata,
    pub(crate) dma_buf: OwnedFd,
}

pub(crate) struct DrmDirectScanoutTestOnlyExecutor;

impl DirectScanoutTestOnlyExecutor for DrmDirectScanoutTestOnlyExecutor {
    fn source(
        &self,
        _source_id: crate::kms::render::store::DrawableId,
        drawable: Option<&crate::kms::render::store::Drawable>,
    ) -> std::io::Result<Option<DirectScanoutProbeSource>> {
        let Some(drawable) = drawable else {
            return Ok(None);
        };
        let Some(metadata) = drawable.storage.imported_dmabuf.as_ref() else {
            return Ok(None);
        };
        if metadata.implicit_layout {
            return Ok(None);
        }
        let Some(fd) = drawable
            .storage
            .imported_drawable
            .as_ref()
            .and_then(|image| image.imported_dma_buf_fd())
        else {
            return Ok(None);
        };
        Ok(Some(DirectScanoutProbeSource {
            metadata: metadata.clone(),
            dma_buf: fd.try_clone_to_owned()?,
        }))
    }

    fn test_only(
        &self,
        device: Rc<crate::drm::Device>,
        dma_buf: BorrowedFd<'_>,
        width: u32,
        height: u32,
        fourcc_code: u32,
        modifier: u64,
        offset: u64,
        pitch: u32,
        atomic_commit_sink: &crate::drm::atomic::AtomicCommitSink,
        planes: &[crate::drm::modeset::DirectScanoutPlaneState<'_>],
        cursor_updates: &[crate::kms::render::platform::cursor_state::CursorPlaneUpdate],
    ) -> std::io::Result<crate::drm::modeset::DirectScanoutTestResult> {
        crate::drm::modeset::probe_direct_scanout_test_only(
            device,
            dma_buf,
            width,
            height,
            fourcc_code,
            modifier,
            offset,
            pitch,
            atomic_commit_sink,
            planes,
            cursor_updates,
        )
    }
}

/// Observes calls at the `Backend` trait boundary. Production installs a
/// no-op observer; core-loop tests count calls independently of renderer work
/// that may return early.
pub(crate) trait BackendEntryObserver {
    fn put_image_entered(&self);
}

pub(crate) struct NoopBackendEntryObserver;

impl BackendEntryObserver for NoopBackendEntryObserver {
    fn put_image_entered(&self) {}
}

#[cfg(test)]
pub(crate) type RecordedDirectScanoutCursorDeltas = Vec<
    Vec<(
        ::drm::control::crtc::Handle,
        Vec<crate::drm::atomic::AtomicPropertyUpdate>,
    )>,
>;

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingDirectScanoutTestOnlyExecutor {
    results: std::cell::RefCell<
        std::collections::VecDeque<crate::drm::modeset::DirectScanoutTestResult>,
    >,
    cursor_deltas: std::cell::RefCell<RecordedDirectScanoutCursorDeltas>,
    sources: std::cell::RefCell<
        std::collections::HashMap<crate::kms::render::store::DrawableId, DirectScanoutProbeSource>,
    >,
}

#[cfg(test)]
impl RecordingDirectScanoutTestOnlyExecutor {
    pub(crate) fn cursor_deltas(&self) -> RecordedDirectScanoutCursorDeltas {
        self.cursor_deltas.borrow().clone()
    }

    pub(crate) fn queue_source(
        &self,
        source_id: crate::kms::render::store::DrawableId,
        metadata: yserver_render::kms::render::store::ImportedDmabufMetadata,
        dma_buf: OwnedFd,
    ) {
        self.sources
            .borrow_mut()
            .insert(source_id, DirectScanoutProbeSource { metadata, dma_buf });
    }

    pub(crate) fn queue_accepted(&self, framebuffer: u32) {
        let framebuffer = ::drm::control::from_u32(framebuffer)
            .expect("synthetic direct-scanout FB handle is nonzero");
        self.results.borrow_mut().push_back(
            crate::drm::modeset::DirectScanoutTestResult::Accepted(
                crate::drm::modeset::DirectScanoutProbeFramebuffer::for_tests(framebuffer),
            ),
        );
    }
}

#[cfg(test)]
impl DirectScanoutTestOnlyExecutor for RecordingDirectScanoutTestOnlyExecutor {
    fn source(
        &self,
        source_id: crate::kms::render::store::DrawableId,
        _drawable: Option<&crate::kms::render::store::Drawable>,
    ) -> std::io::Result<Option<DirectScanoutProbeSource>> {
        Ok(self.sources.borrow_mut().remove(&source_id))
    }

    fn test_only(
        &self,
        _device: Rc<crate::drm::Device>,
        _dma_buf: BorrowedFd<'_>,
        _width: u32,
        _height: u32,
        _fourcc_code: u32,
        _modifier: u64,
        _offset: u64,
        _pitch: u32,
        _atomic_commit_sink: &crate::drm::atomic::AtomicCommitSink,
        _planes: &[crate::drm::modeset::DirectScanoutPlaneState<'_>],
        cursor_updates: &[crate::kms::render::platform::cursor_state::CursorPlaneUpdate],
    ) -> std::io::Result<crate::drm::modeset::DirectScanoutTestResult> {
        self.cursor_deltas.borrow_mut().push(
            cursor_updates
                .iter()
                .map(|update| {
                    (
                        update.crtc,
                        update
                            .prepared
                            .updates
                            .iter()
                            .map(|property| crate::drm::atomic::AtomicPropertyUpdate {
                                object: property.object,
                                property: property.property,
                                value: property.value,
                            })
                            .collect(),
                    )
                })
                .collect(),
        );
        self.results.borrow_mut().pop_front().ok_or_else(|| {
            std::io::Error::other("recording direct-scanout TEST_ONLY result was not queued")
        })
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingBackendEntryObserver {
    put_image_entries: std::cell::Cell<u64>,
}

#[cfg(test)]
impl RecordingBackendEntryObserver {
    pub(crate) fn put_image_entries(&self) -> u64 {
        self.put_image_entries.get()
    }
}

#[cfg(test)]
impl BackendEntryObserver for RecordingBackendEntryObserver {
    fn put_image_entered(&self) {
        self.put_image_entries
            .set(self.put_image_entries.get().saturating_add(1));
    }
}

/// Raw operations retained by a KMS backend. Production algorithms keep
/// constructing requests, scheduling cursor work, and retiring events.
#[derive(Clone)]
pub struct KmsServices {
    pub(crate) mode_blobs: Rc<dyn ModeBlobIo>,
    pub(crate) gamma: Rc<dyn GammaIo>,
    pub(crate) cursor_buffers: Rc<dyn CursorBufferIo>,
    pub(crate) legacy_cursor: Rc<dyn LegacyCursorIo>,
    pub(crate) cursor_plane_factory: Rc<dyn CursorPlaneFactory>,
    pub(crate) crtc_output_discovery: Rc<dyn CrtcOutputDiscovery>,
    pub(crate) atomic: Rc<dyn AtomicCommitExecutor>,
    pub(crate) page_flip_events: Rc<dyn PageFlipEventSource>,
    pub(crate) render_completion_readiness: Rc<dyn RenderCompletionReadiness>,
    pub(crate) copied_scanout_copy: Rc<dyn CopiedScanoutCopySubmitter>,
    pub(crate) hotplug: Rc<dyn DisplayHotplugSource>,
    pub(crate) connector_snapshot: Rc<dyn ConnectorSnapshotIo>,
    pub(crate) monotonic_clock: Rc<dyn Fn() -> Instant>,
    pub(crate) direct_scanout_test_only: Rc<dyn DirectScanoutTestOnlyExecutor>,
    pub(crate) backend_entry_observer: Rc<dyn BackendEntryObserver>,
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
    #[cfg(test)]
    pub(crate) gamma: Rc<RecordingGammaIo>,
    pub(crate) page_flip_events:
        Rc<crate::kms::render::platform::flip_events::RecordingPageFlipEventSource>,
    #[cfg(test)]
    pub(crate) render_completion_readiness: Rc<RecordingRenderCompletionReadiness>,
    #[cfg(test)]
    pub(crate) copied_scanout_copy: Rc<RecordingCopiedScanoutCopySubmitter>,
    pub(crate) hotplug: Rc<RecordingDisplayHotplugSource>,
    pub(crate) connector_snapshot: Rc<RecordingConnectorSnapshotIo>,
    #[cfg(test)]
    pub(crate) direct_scanout_test_only: Rc<RecordingDirectScanoutTestOnlyExecutor>,
    #[cfg(test)]
    pub(crate) backend_entry_observer: Rc<RecordingBackendEntryObserver>,
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

/// Device gamma operations selected with the other KMS services so RandR
/// cache restoration follows the same path with real devices and fixtures.
pub(crate) trait GammaIo {
    fn gamma_size(
        &self,
        device: &crate::drm::Device,
        crtc: ::drm::control::crtc::Handle,
    ) -> std::io::Result<u16>;

    fn set_gamma(
        &self,
        device: &crate::drm::Device,
        crtc: ::drm::control::crtc::Handle,
        red: &[u16],
        green: &[u16],
        blue: &[u16],
    ) -> std::io::Result<()>;
}

pub(crate) struct DrmGammaIo;

impl GammaIo for DrmGammaIo {
    fn gamma_size(
        &self,
        device: &crate::drm::Device,
        crtc: ::drm::control::crtc::Handle,
    ) -> std::io::Result<u16> {
        use ::drm::control::Device as _;

        let info = device
            .get_crtc(crtc)
            .map_err(|error| std::io::Error::other(format!("get_crtc failed: {error}")))?;
        Ok(u16::try_from(info.gamma_length()).unwrap_or(u16::MAX))
    }

    fn set_gamma(
        &self,
        device: &crate::drm::Device,
        crtc: ::drm::control::crtc::Handle,
        red: &[u16],
        green: &[u16],
        blue: &[u16],
    ) -> std::io::Result<()> {
        use ::drm::control::Device as _;

        device
            .set_gamma(crtc, red, green, blue)
            .map_err(|error| std::io::Error::other(format!("set_gamma failed: {error}")))
    }
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GammaCommit {
    pub(crate) crtc: u32,
    pub(crate) red: Vec<u16>,
    pub(crate) green: Vec<u16>,
    pub(crate) blue: Vec<u16>,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingGammaIo {
    commits: std::cell::RefCell<Vec<GammaCommit>>,
}

#[cfg(test)]
impl RecordingGammaIo {
    pub(crate) fn commits(&self) -> Vec<GammaCommit> {
        self.commits.borrow().clone()
    }
}

#[cfg(test)]
impl GammaIo for RecordingGammaIo {
    fn gamma_size(
        &self,
        _device: &crate::drm::Device,
        _crtc: ::drm::control::crtc::Handle,
    ) -> std::io::Result<u16> {
        Ok(256)
    }

    fn set_gamma(
        &self,
        _device: &crate::drm::Device,
        crtc: ::drm::control::crtc::Handle,
        red: &[u16],
        green: &[u16],
        blue: &[u16],
    ) -> std::io::Result<()> {
        self.commits.borrow_mut().push(GammaCommit {
            crtc: u32::from(crtc),
            red: red.to_vec(),
            green: green.to_vec(),
            blue: blue.to_vec(),
        });
        Ok(())
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
            gamma: Rc::new(DrmGammaIo),
            cursor_buffers: Rc::new(DrmCursorBufferIo),
            legacy_cursor: Rc::new(DrmLegacyCursorIo),
            cursor_plane_factory: Rc::new(DrmCursorPlaneFactory),
            crtc_output_discovery: Rc::new(DrmCrtcOutputDiscovery),
            atomic: Rc::new(DrmAtomicCommitExecutor),
            page_flip_events: Rc::new(DrmPageFlipEventSource),
            render_completion_readiness: Rc::new(PollRenderCompletionReadiness),
            copied_scanout_copy: Rc::new(VulkanCopiedScanoutCopySubmitter),
            hotplug: Rc::new(DrmDisplayHotplugSource::new()),
            connector_snapshot: Rc::new(DrmConnectorSnapshotIo),
            monotonic_clock: Rc::new(Instant::now),
            direct_scanout_test_only: Rc::new(DrmDirectScanoutTestOnlyExecutor),
            backend_entry_observer: Rc::new(NoopBackendEntryObserver),
            test_recorders: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::for_tests_with_clock(Rc::new(Instant::now))
    }

    #[cfg(test)]
    pub(crate) fn for_tests_with_clock(monotonic_clock: Rc<dyn Fn() -> Instant>) -> Self {
        let atomic =
            Rc::new(crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default());
        let crtc_output_discovery = Rc::new(RecordingCrtcOutputDiscovery::default());
        let output_power = Rc::new(super::power::RecordingOutputPowerExecutor::default());
        let gamma = Rc::new(RecordingGammaIo::default());
        let mode_blobs = Rc::new(RecordingModeBlobIo::default());
        let (cursor_buffers, cursor_buffer_ledger) = RecordingCursorBufferIo::new(256);
        let cursor_buffers = Rc::new(cursor_buffers);
        let legacy_cursor = Rc::new(RecordingLegacyCursorIo::default());
        let cursor_plane_factory =
            Rc::new(crate::kms::cursor_plane::RecordingCursorPlaneFactory::default());
        let page_flip_events = Rc::new(
            crate::kms::render::platform::flip_events::RecordingPageFlipEventSource::default(),
        );
        let render_completion_readiness = Rc::new(RecordingRenderCompletionReadiness::default());
        let copied_scanout_copy = Rc::new(RecordingCopiedScanoutCopySubmitter::default());
        let hotplug =
            Rc::new(RecordingDisplayHotplugSource::new().expect("recording hotplug socket pair"));
        let connector_snapshot = Rc::new(RecordingConnectorSnapshotIo::new());
        let direct_scanout_test_only = Rc::new(RecordingDirectScanoutTestOnlyExecutor::default());
        let backend_entry_observer = Rc::new(RecordingBackendEntryObserver::default());
        let test_recorders = Rc::new(KmsTestRecorders {
            atomic: Rc::clone(&atomic),
            mode_blobs: Rc::clone(&mode_blobs),
            cursor_buffer_ledger,
            legacy_cursor: Rc::clone(&legacy_cursor),
            #[cfg(test)]
            cursor_plane_factory: Rc::clone(&cursor_plane_factory),
            crtc_output_discovery: Rc::clone(&crtc_output_discovery),
            output_power,
            gamma: Rc::clone(&gamma),
            page_flip_events: Rc::clone(&page_flip_events),
            render_completion_readiness: Rc::clone(&render_completion_readiness),
            copied_scanout_copy: Rc::clone(&copied_scanout_copy),
            hotplug: Rc::clone(&hotplug),
            connector_snapshot: Rc::clone(&connector_snapshot),
            direct_scanout_test_only: Rc::clone(&direct_scanout_test_only),
            backend_entry_observer: Rc::clone(&backend_entry_observer),
        });
        Self {
            mode_blobs,
            gamma,
            cursor_buffers,
            legacy_cursor,
            cursor_plane_factory,
            crtc_output_discovery,
            atomic,
            page_flip_events,
            render_completion_readiness,
            copied_scanout_copy,
            hotplug,
            connector_snapshot,
            monotonic_clock,
            direct_scanout_test_only,
            backend_entry_observer,
            test_recorders: Some(test_recorders),
        }
    }

    #[cfg(not(test))]
    pub(crate) fn for_tests() -> Self {
        Self::for_tests_with_clock(Rc::new(Instant::now))
    }

    #[cfg(not(test))]
    pub(crate) fn for_tests_with_clock(monotonic_clock: Rc<dyn Fn() -> Instant>) -> Self {
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
            gamma: Rc::new(DrmGammaIo),
            cursor_buffers,
            legacy_cursor,
            cursor_plane_factory: Rc::new(DrmCursorPlaneFactory),
            crtc_output_discovery: Rc::new(DrmCrtcOutputDiscovery),
            atomic,
            page_flip_events,
            render_completion_readiness: Rc::new(PollRenderCompletionReadiness),
            copied_scanout_copy: Rc::new(VulkanCopiedScanoutCopySubmitter),
            hotplug,
            connector_snapshot: Rc::new(RecordingConnectorSnapshotIo::new()),
            monotonic_clock,
            direct_scanout_test_only: Rc::new(DrmDirectScanoutTestOnlyExecutor),
            backend_entry_observer: Rc::new(NoopBackendEntryObserver),
            test_recorders: None,
        }
    }
}
