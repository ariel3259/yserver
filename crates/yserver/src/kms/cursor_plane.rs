//! Hardware cursor plane — replaces the Vulkan-composited cursor
//! quad with a kernel-managed DRM cursor overlay.
//!
//! Why: the cursor quad was tied to compositor cadence. Every cursor
//! position change waited for the next `composite_and_flip`, which
//! is stalled by per-op `vkQueueWaitIdle` in the paint pipeline
//! (notably when hovering over GTK widgets that schedule
//! gradient/emboss repaints — observed as severe pointer lag in
//! mate-control-center on fuji). The DRM hardware cursor plane is
//! a separate overlay the kernel positions independently —
//! an atomic position commit is microseconds and doesn't touch the GPU.
//!
//! Atomic cursor eligibility is discovered per assigned CRTC. A plane must
//! expose every property needed to bind and position a cursor with atomic
//! state; virtual display drivers also need accepted hotspot-cap negotiation
//! and both hotspot properties.
//!
//! Stage 5 Phase B (per-CRTC visibility + upload/show split): the
//! shared dumb buffer is mutated by `load_image` and bound to each
//! CRTC independently. Per-CRTC visibility tracking lets the
//! per-output `PendingAck` design queue a Sw→Hw transition for one
//! output without prematurely binding the plane on outputs that
//! haven't retired the transition yet (the multi-output double-cursor
//! hazard).

pub(crate) mod ops;
pub(crate) mod sprites;
pub(crate) mod topology;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    io,
    ptr::NonNull,
    rc::Rc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CursorLeaseBoundary {
    Reserve,
    Accept,
}

#[derive(Default)]
struct CursorLeaseFailureScript(RefCell<VecDeque<(CursorLeaseBoundary, usize)>>);

impl CursorLeaseFailureScript {
    fn fail_next(&self, boundary: CursorLeaseBoundary) -> bool {
        let mut script = self.0.borrow_mut();
        let Some((next, remaining)) = script.front_mut() else {
            return false;
        };
        if *next != boundary {
            return false;
        }
        if *remaining > 0 {
            *remaining -= 1;
            return false;
        }
        script.pop_front();
        true
    }

    #[cfg(test)]
    fn push(&mut self, boundary: CursorLeaseBoundary, matching_calls_to_skip: usize) {
        self.0
            .get_mut()
            .push_back((boundary, matching_calls_to_skip));
    }
}

use drm::{
    Device as DrmDevice, DriverCapability,
    control::{Device as ControlDevice, crtc, dumbbuffer::DumbBuffer, plane, property},
};

use crate::drm::Device;
#[cfg(test)]
use ops::RecordingCursorBufferIo;
use ops::{CursorBuffer, CursorBufferIo, DrmCursorBufferIo, DrmLegacyCursorIo, LegacyCursorIo};
use sprites::{SpriteBufferLease, SpritePool};
use topology::{
    CursorPlaneDiscovery, CursorPlaneDiscoveryIo, CursorPlaneProperties, DrmCursorPlaneDiscovery,
    cursor_plane_topology,
};

#[cfg(test)]
pub(crate) use topology::{CursorPlaneTestPlane, CursorPlaneTestSnapshot};
#[cfg(test)]
use topology::{CursorPlaneTopology, DiscoveredCursorPlane};

/// Legacy cursor-plane hide operation retained for compatibility cleanup and
/// tests. The KMS platform path submits atomic cursor state instead.
#[allow(dead_code)]
pub(crate) trait CursorHideExecutor {
    fn hide(&self, device: &Device, crtc: crtc::Handle) -> io::Result<()>;
}

/// The backing required for a cursor-plane bind. `Test` represents in-memory
/// fixture storage and is only accepted by the injected test executor.
#[allow(dead_code)]
pub(crate) enum CursorPlaneBuffer {
    Dumb(DumbBuffer),
    #[allow(dead_code)]
    Test,
}

/// Executor for the legacy cursor show + initial bind operation.
#[allow(dead_code)]
pub(crate) trait CursorShowExecutor {
    fn show(
        &self,
        device: &Device,
        buffer: &CursorPlaneBuffer,
        crtc: crtc::Handle,
        hotspot: (i32, i32),
        img_x: i32,
        img_y: i32,
    ) -> io::Result<()>;

    fn move_to(
        &self,
        device: &Device,
        crtc: crtc::Handle,
        img_x: i32,
        img_y: i32,
    ) -> io::Result<()>;
}

#[allow(dead_code)]
#[derive(Default)]
pub(crate) struct DrmCursorShowExecutor;

impl CursorShowExecutor for DrmCursorShowExecutor {
    #[allow(deprecated)]
    fn show(
        &self,
        device: &Device,
        buffer: &CursorPlaneBuffer,
        crtc: crtc::Handle,
        hotspot: (i32, i32),
        _img_x: i32,
        _img_y: i32,
    ) -> io::Result<()> {
        let CursorPlaneBuffer::Dumb(dumb) = buffer else {
            return Err(io::Error::other("test cursor backing used by DRM executor"));
        };
        device.set_cursor2(crtc, Some(dumb), hotspot)
    }

    #[allow(deprecated)]
    fn move_to(
        &self,
        device: &Device,
        crtc: crtc::Handle,
        img_x: i32,
        img_y: i32,
    ) -> io::Result<()> {
        device.move_cursor(crtc, (img_x, img_y))
    }
}

#[allow(dead_code)]
#[derive(Default)]
pub(crate) struct DrmCursorHideExecutor;

impl CursorHideExecutor for DrmCursorHideExecutor {
    #[allow(deprecated)]
    fn hide(&self, device: &Device, crtc: crtc::Handle) -> io::Result<()> {
        device.set_cursor2::<DumbBuffer>(crtc, None, (0, 0))
    }
}

/// A failed cursor bind/reposition operation, including whether the cursor is
/// still bound after best-effort rollback. Callers must not switch an output
/// to software composition while `remains_visible()` is true: doing so would
/// display both the old hardware cursor and the new software sprite.
#[derive(Debug)]
pub enum CursorShowError {
    Unbound(io::Error),
    Deferred {
        operation_error: io::Error,
        remains_visible: bool,
    },
    StillVisible {
        operation_error: io::Error,
        rollback_error: Option<io::Error>,
    },
}

impl CursorShowError {
    #[must_use]
    pub fn remains_visible(&self) -> bool {
        match self {
            Self::Unbound(_) => false,
            Self::Deferred {
                remains_visible, ..
            } => *remains_visible,
            Self::StillVisible { .. } => true,
        }
    }

    #[must_use]
    pub fn is_deferred(&self) -> bool {
        matches!(self, Self::Deferred { .. })
    }

    #[must_use]
    pub fn operation_error(&self) -> &io::Error {
        match self {
            Self::Unbound(error) => error,
            Self::Deferred {
                operation_error, ..
            } => operation_error,
            Self::StillVisible {
                operation_error, ..
            } => operation_error,
        }
    }

    #[must_use]
    pub fn rollback_error(&self) -> Option<&io::Error> {
        match self {
            Self::Unbound(_) | Self::Deferred { .. } => None,
            Self::StillVisible { rollback_error, .. } => rollback_error.as_ref(),
        }
    }

    /// A failed `set_cursor2` on an already-visible CRTC leaves the prior
    /// buffer/hotspot binding intact. A position-only retry cannot repair it;
    /// the caller must retry the full show operation.
    #[must_use]
    pub fn needs_full_rebind(&self) -> bool {
        matches!(
            self,
            Self::StillVisible {
                rollback_error: None,
                ..
            }
        )
    }
}

impl std::fmt::Display for CursorShowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unbound(error) => write!(f, "cursor remains unbound: {error}"),
            Self::Deferred {
                operation_error,
                remains_visible,
            } => write!(
                f,
                "cursor operation deferred ({operation_error}); prior binding visible={remains_visible}"
            ),
            Self::StillVisible {
                operation_error,
                rollback_error,
            } => match rollback_error {
                Some(rollback_error) => write!(
                    f,
                    "cursor operation failed ({operation_error}) and hide rollback failed ({rollback_error}); prior binding remains visible"
                ),
                None => write!(
                    f,
                    "cursor rebind failed ({operation_error}); prior binding remains visible"
                ),
            },
        }
    }
}

impl std::error::Error for CursorShowError {}

#[allow(
    dead_code,
    reason = "legacy cursor error adapter remains for tasks B/C cleanup"
)]
fn bind_failure(prior_visible: bool, error: io::Error) -> CursorShowError {
    if prior_visible {
        CursorShowError::StillVisible {
            operation_error: error,
            rollback_error: None,
        }
    } else {
        CursorShowError::Unbound(error)
    }
}

#[allow(
    dead_code,
    reason = "legacy cursor error adapter remains for tasks B/C cleanup"
)]
fn move_failure(move_error: io::Error, rollback: io::Result<()>) -> CursorShowError {
    match rollback {
        Ok(()) => CursorShowError::Unbound(move_error),
        Err(rollback_error) => CursorShowError::StillVisible {
            operation_error: move_error,
            rollback_error: Some(rollback_error),
        },
    }
}

#[allow(
    dead_code,
    reason = "legacy cursor adapter remains for tasks B/C cleanup"
)]
fn show_with_rollback(
    visible: &mut HashMap<crtc::Handle, bool>,
    crtc: crtc::Handle,
    prior_visible: bool,
    bind: impl FnOnce() -> io::Result<()>,
    position: impl FnOnce() -> io::Result<()>,
    hide: impl FnOnce() -> io::Result<()>,
) -> Result<(), CursorShowError> {
    bind().map_err(|error| bind_failure(prior_visible, error))?;
    visible.insert(crtc, true);
    match position() {
        Ok(()) => Ok(()),
        Err(move_error) => {
            let rollback = hide();
            if rollback.is_ok() {
                visible.insert(crtc, false);
            }
            Err(move_failure(move_error, rollback))
        }
    }
}

/// Fallback cursor size when `DRM_CAP_CURSOR_WIDTH/HEIGHT` query fails
/// (very old drivers, broken devices). Every Intel / AMD / mainstream-
/// Mali iGPU since ~2010 supports at least 64×64.
///
/// The ACTUAL dumb buffer is allocated at the dimensions the driver
/// reports via `DriverCapability::CursorWidth/Height` (typically 64 on
/// Intel i915, 128 or 256 on amdgpu, varies on others). Using the
/// driver-reported size is load-bearing: amdgpu's display engine
/// interprets the cursor framebuffer as if it were `cursor_width ×
/// cursor_height`, so allocating smaller causes it to read past our
/// data → cursor vertically squished + intermittent corruption.
pub const HW_CURSOR_FALLBACK_W: u32 = 64;
pub const HW_CURSOR_FALLBACK_H: u32 = 64;

fn cursor_cap_dimension(capability: io::Result<u64>, fallback: u32) -> u32 {
    capability
        .ok()
        .filter(|value| *value != 0)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(fallback)
}

/// A single shared DRM dumb buffer holding the current cursor image,
/// plus per-CRTC visibility state.
///
/// Per-CRTC visibility (Stage 5 Phase B refactor): each CRTC tracks
/// whether the plane is currently bound to it. v1's pre-Phase-B global
/// `visible: bool` was correct only on single-output systems and exposed
/// the multi-output double-cursor hazard when one output retired a
/// Sw→Hw transition before another.
pub struct CursorPlane {
    device: Rc<Device>,
    cursor_buffer: Option<CursorBuffer>,
    sprite_pool: SpritePool,
    lease_failures: CursorLeaseFailureScript,
    operations: CursorPlaneOperations,
    dumb: Option<CursorPlaneBuffer>,
    ptr: NonNull<u8>,
    len: usize,
    stride: u32,
    /// Cursor buffer dimensions in pixels. Sourced from
    /// `DriverCapability::CursorWidth/Height`. Mandatory match with the
    /// dumb buffer geometry — see [`HW_CURSOR_FALLBACK_W`].
    width: u32,
    height: u32,
    /// Per-CRTC binding state — `Some(true)` when the plane is shown on
    /// that CRTC; `Some(false)` when hidden; absent until first show/hide.
    visible: HashMap<crtc::Handle, bool>,
    /// Stage 5 Phase B — `CursorRecord.version` last memcpy'd into
    /// the dumb buffer. `cursor_plane_upload_image` compares the
    /// requested version against this for upload dedup; `None` after
    /// init / VT-leave / full modeset (forces the next show to
    /// re-upload).
    uploaded_version: Option<u64>,
    /// Qualified, exclusive assignment for each CRTC. A missing entry means
    /// this output must use the software cursor; it is never a legacy probe.
    properties_by_crtc: HashMap<crtc::Handle, CursorPlaneProperties>,
    discovery: CursorPlaneDiscovery,
    discovery_io: Rc<dyn CursorPlaneDiscoveryIo>,
    active_crtcs: HashSet<crtc::Handle>,
    pending_discovery: Option<CursorPlaneDiscovery>,
    #[cfg(test)]
    test_discovery: Option<Rc<RecordingCursorPlaneDiscovery>>,
    /// In-memory storage for the ioctl-free test factory. Production cursor
    /// planes own an mmap through `dumb`; tests need a safe buffer to exercise
    /// lazy-init upload/version behavior without a real DRM node.
    #[cfg(test)]
    _test_backing: Option<Box<[u8]>>,
}

enum CursorPlaneOperations {
    Legacy(Rc<dyn LegacyCursorIo>),
    #[allow(dead_code)]
    Executors {
        hide: Rc<dyn CursorHideExecutor>,
        show: Rc<dyn CursorShowExecutor>,
    },
}

/// Constructs cursor-plane state at startup and topology retry boundaries.
/// Production and test implementations share the same caller and lifecycle.
pub(crate) trait CursorPlaneFactory {
    fn create(
        &self,
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
    ) -> io::Result<CursorPlane>;
}

#[derive(Default)]
pub(crate) struct DrmCursorPlaneFactory;

impl CursorPlaneFactory for DrmCursorPlaneFactory {
    fn create(
        &self,
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
    ) -> io::Result<CursorPlane> {
        CursorPlane::new_with_ops(device, crtcs, buffer_io, legacy_io)
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingCursorPlaneFactory {
    queued: std::cell::RefCell<std::collections::VecDeque<CursorPlaneFactoryResult>>,
    calls: std::cell::Cell<usize>,
}

#[cfg(test)]
enum CursorPlaneFactoryResult {
    Error(i32),
    Snapshot(CursorPlaneTestSnapshot),
}

#[cfg(test)]
impl RecordingCursorPlaneFactory {
    pub(crate) fn queue_error(&self, raw_os_error: i32) {
        self.queued
            .borrow_mut()
            .push_back(CursorPlaneFactoryResult::Error(raw_os_error));
    }

    pub(crate) fn queue_snapshot(&self, snapshot: CursorPlaneTestSnapshot) {
        self.queued
            .borrow_mut()
            .push_back(CursorPlaneFactoryResult::Snapshot(snapshot));
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.get()
    }
}

#[cfg(test)]
impl CursorPlaneFactory for RecordingCursorPlaneFactory {
    fn create(
        &self,
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
    ) -> io::Result<CursorPlane> {
        self.calls.set(self.calls.get() + 1);
        match self.queued.borrow_mut().pop_front() {
            Some(CursorPlaneFactoryResult::Error(raw_os_error)) => {
                Err(io::Error::from_raw_os_error(raw_os_error))
            }
            Some(CursorPlaneFactoryResult::Snapshot(snapshot)) => {
                CursorPlane::for_tests_with_discovery_snapshot(
                    device, crtcs, buffer_io, legacy_io, snapshot,
                )
                .map(|(plane, _)| plane)
            }
            None => DrmCursorPlaneFactory.create(device, crtcs, buffer_io, legacy_io),
        }
    }
}

#[cfg(test)]
pub(crate) struct RecordingCursorPlaneDiscovery {
    latest: std::cell::RefCell<CursorPlaneDiscovery>,
    queued: std::cell::RefCell<std::collections::VecDeque<CursorPlaneDiscovery>>,
    calls: std::cell::Cell<usize>,
}

#[cfg(test)]
impl RecordingCursorPlaneDiscovery {
    fn new(snapshot: CursorPlaneDiscovery) -> Self {
        Self {
            latest: std::cell::RefCell::new(snapshot),
            queued: std::cell::RefCell::new(std::collections::VecDeque::new()),
            calls: std::cell::Cell::new(0),
        }
    }

    pub(crate) fn queue_snapshot(&self, snapshot: CursorPlaneTestSnapshot) {
        self.queued
            .borrow_mut()
            .push_back(snapshot.into_discovery());
    }

    pub(crate) fn calls(&self) -> usize {
        self.calls.get()
    }
}

#[cfg(test)]
impl CursorPlaneTestSnapshot {
    fn into_discovery(self) -> CursorPlaneDiscovery {
        self.into()
    }
}

#[cfg(test)]
impl CursorPlaneDiscoveryIo for RecordingCursorPlaneDiscovery {
    fn discover(&self, _device: &Device) -> io::Result<CursorPlaneDiscovery> {
        self.calls.set(self.calls.get() + 1);
        if let Some(snapshot) = self.queued.borrow_mut().pop_front() {
            *self.latest.borrow_mut() = snapshot;
        }
        Ok(self.latest.borrow().clone())
    }
}

impl CursorPlane {
    /// Allocate the cursor dumb buffer + mmap it and qualify discovered
    /// atomic cursor planes independently for each assigned CRTC. CRTCs
    /// without an eligible assignment use software composition.
    ///
    /// # Errors
    /// Cursor-plane topology discovery, `create_dumb_buffer`, or
    /// `map_dumb_buffer` ioctl failures.
    #[allow(dead_code)]
    pub fn new(device: Rc<Device>, crtcs: &[crtc::Handle]) -> io::Result<Self> {
        Self::new_with_ops(
            device,
            crtcs,
            Rc::new(DrmCursorBufferIo),
            Rc::new(DrmLegacyCursorIo),
        )
    }

    #[allow(dead_code)]
    pub(crate) fn new_with_buffer_io(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
    ) -> io::Result<Self> {
        Self::new_with_ops(device, crtcs, buffer_io, Rc::new(DrmLegacyCursorIo))
    }

    pub(crate) fn new_with_ops(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
    ) -> io::Result<Self> {
        let discovery_io: Rc<dyn CursorPlaneDiscoveryIo> = Rc::new(DrmCursorPlaneDiscovery);
        let discovery = discovery_io.discover(&device)?;
        let width = cursor_cap_dimension(
            device.get_driver_capability(DriverCapability::CursorWidth),
            HW_CURSOR_FALLBACK_W,
        );
        let height = cursor_cap_dimension(
            device.get_driver_capability(DriverCapability::CursorHeight),
            HW_CURSOR_FALLBACK_H,
        );
        Self::from_discovery(
            device,
            crtcs,
            buffer_io,
            legacy_io,
            discovery_io,
            discovery,
            width,
            height,
        )
    }

    fn from_discovery(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
        discovery_io: Rc<dyn CursorPlaneDiscoveryIo>,
        discovery: CursorPlaneDiscovery,
        width: u32,
        height: u32,
    ) -> io::Result<Self> {
        let properties_by_crtc =
            cursor_plane_topology(&discovery.crtcs, &discovery.planes, discovery.caps.as_ref());
        log::info!("cursor: driver reports CursorWidth={width} CursorHeight={height}");

        let cursor_buffer = CursorBuffer::allocate(Rc::clone(&device), buffer_io, width, height)?;
        let stride = cursor_buffer.stride();
        let len = cursor_buffer.len();
        let ptr = cursor_buffer.ptr();
        let dumb = cursor_buffer
            .dumb()
            .map_or(CursorPlaneBuffer::Test, CursorPlaneBuffer::Dumb);

        let mut sprite_pool = SpritePool::new(width, height);
        sprite_pool.set_active_crtc_count(crtcs.len());
        let plane = Self {
            device,
            cursor_buffer: Some(cursor_buffer),
            sprite_pool,
            lease_failures: CursorLeaseFailureScript::default(),
            operations: CursorPlaneOperations::Legacy(legacy_io),
            dumb: Some(dumb),
            ptr,
            len,
            stride,
            width,
            height,
            visible: HashMap::new(),
            uploaded_version: None,
            properties_by_crtc,
            discovery,
            discovery_io,
            active_crtcs: crtcs.iter().copied().collect(),
            pending_discovery: None,
            #[cfg(test)]
            test_discovery: None,
            #[cfg(test)]
            _test_backing: None,
        };
        if !plane.supports_crtcs(crtcs) {
            log::warn!(
                "cursor: no distinct cursor plane is assigned to every one of the {} active CRTCs",
                crtcs.len()
            );
        }
        Ok(plane)
    }

    /// Construct the same cursor allocation and topology state used by
    /// production, with synthetic kernel topology at the fixture boundary.
    #[cfg(test)]
    pub(crate) fn for_tests_with_ops(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
    ) -> io::Result<Self> {
        Self::for_tests_with_capabilities(device, crtcs, buffer_io, legacy_io, Some(64), Some(64))
    }

    #[cfg(test)]
    pub(crate) fn for_tests_with_capabilities(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) -> io::Result<Self> {
        Self::for_tests_with_capabilities_and_hotspot_properties(
            device,
            crtcs,
            buffer_io,
            legacy_io,
            cursor_width,
            cursor_height,
            false,
        )
    }

    #[cfg(test)]
    pub(crate) fn for_tests_with_hotspot_properties(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) -> io::Result<Self> {
        Self::for_tests_with_capabilities_and_hotspot_properties(
            device,
            crtcs,
            buffer_io,
            legacy_io,
            cursor_width,
            cursor_height,
            true,
        )
    }

    #[cfg(test)]
    fn for_tests_with_capabilities_and_hotspot_properties(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
        hotspot_properties: bool,
    ) -> io::Result<Self> {
        let planes = crtcs
            .iter()
            .enumerate()
            .map(|(index, crtc)| {
                let base = 0x8000_u32
                    .checked_add(u32::try_from(index).expect("test CRTC index fits") * 3)
                    .expect("test cursor handle fits");
                let mut property_names = vec![
                    "FB_ID", "CRTC_ID", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X", "SRC_Y",
                    "SRC_W", "SRC_H",
                ];
                if hotspot_properties {
                    property_names.extend(["HOTSPOT_X", "HOTSPOT_Y"]);
                }
                DiscoveredCursorPlane {
                    handle: ::drm::control::from_u32(base).expect("test plane handle"),
                    crtcs: HashSet::from([*crtc]),
                    properties: property_names
                        .into_iter()
                        .enumerate()
                        .map(|(offset, name)| {
                            (
                                name.to_owned(),
                                ::drm::control::from_u32(base + 1 + offset as u32)
                                    .expect("test cursor property handle"),
                            )
                        })
                        .collect(),
                }
            })
            .collect();
        let discovery = CursorPlaneDiscovery {
            crtcs: crtcs.to_vec(),
            planes,
            caps: Some(crate::drm::CursorDeviceCaps::new(
                true,
                "atomic-cursor-test".to_owned(),
            )),
        };
        let discovery_io = Rc::new(RecordingCursorPlaneDiscovery::new(discovery.clone()));
        let mut plane = Self::from_discovery(
            device,
            crtcs,
            buffer_io,
            legacy_io,
            discovery_io.clone(),
            discovery,
            cursor_cap_dimension(
                cursor_width.ok_or_else(|| io::Error::other("cursor width query unavailable")),
                HW_CURSOR_FALLBACK_W,
            ),
            cursor_cap_dimension(
                cursor_height.ok_or_else(|| io::Error::other("cursor height query unavailable")),
                HW_CURSOR_FALLBACK_H,
            ),
        )?;
        plane.test_discovery = Some(discovery_io);
        Ok(plane)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn for_tests_with_discovery_snapshot(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        legacy_io: Rc<dyn LegacyCursorIo>,
        snapshot: CursorPlaneTestSnapshot,
    ) -> io::Result<(Self, Rc<RecordingCursorPlaneDiscovery>)> {
        let discovery = snapshot.into_discovery();
        let discovery_io = Rc::new(RecordingCursorPlaneDiscovery::new(discovery.clone()));
        let mut plane = Self::from_discovery(
            device,
            crtcs,
            buffer_io,
            legacy_io,
            discovery_io.clone(),
            discovery,
            HW_CURSOR_FALLBACK_W,
            HW_CURSOR_FALLBACK_H,
        )?;
        plane.test_discovery = Some(discovery_io.clone());
        Ok((plane, discovery_io))
    }

    /// Build an ioctl-free cursor plane for platform state-machine tests.
    /// The recording backing exercises the production allocation/upload path;
    /// show remains unavailable because there is deliberately no DRM dumb
    /// buffer.
    #[cfg(test)]
    pub(crate) fn for_tests_stub(device: Rc<Device>, width: u32, height: u32) -> Self {
        let (buffer_io, _) = RecordingCursorBufferIo::new(1);
        let cursor_buffer =
            CursorBuffer::allocate(Rc::clone(&device), Rc::new(buffer_io), width, height)
                .expect("recording cursor backing allocation");
        let stride = cursor_buffer.stride();
        let len = cursor_buffer.len();
        let ptr = cursor_buffer.ptr();
        Self {
            device: Rc::clone(&device),
            cursor_buffer: Some(cursor_buffer),
            sprite_pool: SpritePool::new(width, height),
            lease_failures: CursorLeaseFailureScript::default(),
            operations: CursorPlaneOperations::Executors {
                hide: Rc::new(DrmCursorHideExecutor),
                show: Rc::new(DrmCursorShowExecutor),
            },
            dumb: None,
            ptr,
            len,
            stride,
            width,
            height,
            visible: HashMap::new(),
            uploaded_version: None,
            properties_by_crtc: HashMap::new(),
            discovery: CursorPlaneDiscovery {
                crtcs: Vec::new(),
                planes: Vec::new(),
                caps: None,
            },
            discovery_io: Rc::new(DrmCursorPlaneDiscovery),
            active_crtcs: HashSet::new(),
            pending_discovery: None,
            #[cfg(test)]
            test_discovery: None,
            _test_backing: None,
        }
    }

    /// Test constructor that exercises the same checked show path as
    /// production while redirecting the final bind through an injected
    /// executor. `has_buffer` models whether a dumb buffer was allocated.
    #[cfg(test)]
    pub(crate) fn for_tests_with_show_executor(
        device: Rc<Device>,
        width: u32,
        height: u32,
        has_buffer: bool,
        show_executor: Rc<dyn CursorShowExecutor>,
    ) -> Self {
        let mut plane = Self::for_tests_stub(device, width, height);
        if let CursorPlaneOperations::Executors { show, .. } = &mut plane.operations {
            *show = show_executor;
        }
        if has_buffer {
            plane.dumb = Some(CursorPlaneBuffer::Test);
        }
        plane
    }

    #[cfg(test)]
    pub(crate) fn for_tests_set_atomic_position_properties(
        &mut self,
        crtc: crtc::Handle,
        plane: plane::Handle,
        crtc_x: property::Handle,
        crtc_y: property::Handle,
    ) {
        let base = 0x2a00_u32;
        let property = |offset| {
            ::drm::control::from_u32(base.wrapping_add(offset)).expect("test property handle")
        };
        self.properties_by_crtc.insert(
            crtc,
            CursorPlaneProperties {
                plane,
                fb_id: property(1),
                crtc_id: property(2),
                crtc_x,
                crtc_y,
                crtc_w: property(3),
                crtc_h: property(4),
                src_x: property(5),
                src_y: property(6),
                src_w: property(7),
                src_h: property(8),
                hotspot_x: None,
                hotspot_y: None,
            },
        );
        self.active_crtcs.insert(crtc);
    }

    #[cfg(test)]
    pub(crate) fn for_tests_queue_discovery_snapshot(
        &self,
        snapshot: CursorPlaneTestSnapshot,
    ) -> bool {
        let Some(discovery) = &self.test_discovery else {
            return false;
        };
        discovery.queue_snapshot(snapshot);
        true
    }

    #[cfg(test)]
    pub(crate) fn for_tests_discovery_calls(&self) -> Option<usize> {
        self.test_discovery
            .as_ref()
            .map(|discovery| discovery.calls())
    }

    /// Rediscover property handles and the maximum matching at each device
    /// topology boundary. Returns true only when the raw CRTC/plane snapshot
    /// or active CRTC set actually changed.
    pub(crate) fn rediscover_topology(
        &mut self,
        active_crtcs: &[crtc::Handle],
    ) -> io::Result<bool> {
        let changed = self.probe_topology(active_crtcs)?;
        Ok(self.apply_probed_topology(active_crtcs) || changed)
    }

    pub(crate) fn probe_topology(&mut self, active_crtcs: &[crtc::Handle]) -> io::Result<bool> {
        let discovery = self.discovery_io.discover(&self.device)?;
        let active_crtcs: HashSet<_> = active_crtcs.iter().copied().collect();
        let changed = discovery != self.discovery || active_crtcs != self.active_crtcs;
        if changed {
            self.pending_discovery = Some(discovery);
        } else {
            self.pending_discovery = None;
        }
        Ok(changed)
    }

    pub(crate) fn apply_probed_topology(&mut self, active_crtcs: &[crtc::Handle]) -> bool {
        let active_crtcs: HashSet<_> = active_crtcs.iter().copied().collect();
        let discovery = self
            .pending_discovery
            .take()
            .unwrap_or_else(|| self.discovery.clone());
        let changed = discovery != self.discovery || active_crtcs != self.active_crtcs;
        if changed {
            self.properties_by_crtc =
                cursor_plane_topology(&discovery.crtcs, &discovery.planes, discovery.caps.as_ref());
            self.discovery = discovery;
            self.active_crtcs = active_crtcs;
        }
        changed
    }

    pub(crate) fn has_pending_topology(&self) -> bool {
        self.pending_discovery.is_some()
    }

    /// Set only the in-memory binding observation. Production never calls
    /// this; atomic scene-path tests use it to model a cursor that was already
    /// shown without issuing `set_cursor2` against the test DRM fd.
    #[cfg(test)]
    pub(crate) fn for_tests_set_visible(&mut self, crtc: crtc::Handle, visible: bool) {
        self.visible.insert(crtc, visible);
    }

    #[cfg(test)]
    pub(crate) fn for_tests_set_hide_executor(&mut self, executor: Rc<dyn CursorHideExecutor>) {
        match &mut self.operations {
            CursorPlaneOperations::Legacy(_) => {
                self.operations = CursorPlaneOperations::Executors {
                    hide: executor,
                    show: Rc::new(DrmCursorShowExecutor),
                };
            }
            CursorPlaneOperations::Executors { hide, .. } => *hide = executor,
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn for_tests_set_legacy_io(&mut self, io: Rc<dyn LegacyCursorIo>) {
        self.operations = CursorPlaneOperations::Legacy(io);
    }

    /// Whether every requested CRTC has its own assigned atomic cursor plane.
    /// A CRTC without a complete eligible assignment uses software composition.
    #[must_use]
    pub fn supports_crtcs(&self, crtcs: &[crtc::Handle]) -> bool {
        crtcs
            .iter()
            .all(|crtc| self.properties_for(*crtc).is_some())
    }

    /// Complete atomic property handles for this CRTC's uniquely assigned,
    /// eligible cursor plane.
    #[must_use]
    pub(crate) fn properties_for(&self, crtc: crtc::Handle) -> Option<CursorPlaneProperties> {
        self.properties_by_crtc.get(&crtc).copied()
    }

    /// Atomic plane and position properties for `crtc`, if its assigned
    /// cursor plane exposes both `CRTC_X` and `CRTC_Y`.
    #[allow(dead_code)] // consumed by the scene commit path in Task 2
    #[must_use]
    pub(crate) fn atomic_position_properties(
        &self,
        crtc: crtc::Handle,
    ) -> Option<(plane::Handle, property::Handle, property::Handle)> {
        let properties = self.properties_for(crtc)?;
        Some((properties.plane, properties.crtc_x, properties.crtc_y))
    }

    /// Whether every requested CRTC has a cursor plane with atomic position
    /// properties. An empty topology is supported vacuously; active topology
    /// selection always supplies its complete CRTC list.
    #[allow(dead_code)] // retained for topology diagnostics and unit fixtures
    #[must_use]
    pub(crate) fn supports_atomic_positions(&self, crtcs: &[crtc::Handle]) -> bool {
        crtcs
            .iter()
            .all(|crtc| self.properties_for(*crtc).is_some())
    }

    /// Copy a cursor image into the plane buffer. `bgra_bytes` is a
    /// tightly-packed `width × height × 4` BGRA8 buffer matching the
    /// DRM `ARGB8888` byte order in little-endian. The image lands at
    /// (0, 0); the remainder of the 64×64 buffer is zero-filled
    /// (transparent).
    ///
    /// Returns `Err(InvalidInput)` if the image is larger than
    /// `HW_CURSOR_W × HW_CURSOR_H` — caller falls back to the
    /// compositor cursor path.
    pub fn load_image(&mut self, image_w: u32, image_h: u32, bgra_bytes: &[u8]) -> io::Result<()> {
        if image_w == 0 || image_h == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "zero-sized cursor",
            ));
        }
        if image_w > self.width || image_h > self.height {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cursor exceeds hardware plane size",
            ));
        }
        let img_stride = (image_w as usize) * 4;
        let expected_bytes = img_stride * image_h as usize;
        if bgra_bytes.len() < expected_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cursor bytes shorter than width*height*4",
            ));
        }
        // Clear so a smaller cursor doesn't leave previous pixels.
        unsafe { std::ptr::write_bytes(self.ptr.as_ptr(), 0, self.len) };
        for row in 0..(image_h as usize) {
            let src_off = row * img_stride;
            let dst_off = row * (self.stride as usize);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bgra_bytes.as_ptr().add(src_off),
                    self.ptr.as_ptr().add(dst_off),
                    img_stride,
                );
            }
        }
        Ok(())
    }

    /// Versioned legacy-buffer upload. It never binds the image; the atomic
    /// platform path obtains a separate candidate sprite lease to submit.
    ///
    /// # Errors
    /// Same as [`Self::load_image`].
    pub fn upload_image(
        &mut self,
        version: u64,
        hot_x: u16,
        hot_y: u16,
        image_w: u32,
        image_h: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<yserver_render::kms::render::scene::CursorUploadOutcome> {
        if self.uploaded_image_matches(version, hot_x, hot_y) {
            return Ok(yserver_render::kms::render::scene::CursorUploadOutcome::Ready);
        }
        let Some(buffer) = self.cursor_buffer.as_mut() else {
            return Err(io::Error::other("cursor plane already destroyed"));
        };
        let buffer_io = buffer.io();
        match self.sprite_pool.prepare(
            Rc::clone(&self.device),
            buffer_io,
            version,
            hot_x,
            hot_y,
            image_w,
            image_h,
            bgra_bytes,
        )? {
            yserver_render::kms::render::scene::CursorUploadOutcome::Ready => {}
            deferred @ yserver_render::kms::render::scene::CursorUploadOutcome::Deferred => {
                return Ok(deferred);
            }
        }
        self.load_image(image_w, image_h, bgra_bytes)?;
        self.uploaded_version = Some(version);
        Ok(yserver_render::kms::render::scene::CursorUploadOutcome::Ready)
    }

    /// The version currently held in the dumb buffer, if any.
    #[must_use]
    pub fn uploaded_version(&self) -> Option<u64> {
        self.uploaded_version
    }

    pub(crate) fn uploaded_image_matches(&self, version: u64, hot_x: u16, hot_y: u16) -> bool {
        self.uploaded_version == Some(version)
            && self
                .sprite_pool
                .candidate_lease()
                .is_some_and(|lease| lease.matches(version, hot_x, hot_y))
    }

    pub(crate) fn candidate_sprite(&self) -> Option<(SpriteBufferLease, u32)> {
        let lease = self.sprite_pool.candidate_lease()?;
        Some((lease, self.sprite_pool.framebuffer_for_lease(lease)?))
    }

    pub(crate) fn reserve_sprite(&mut self, lease: SpriteBufferLease) -> bool {
        !self.lease_failures.fail_next(CursorLeaseBoundary::Reserve)
            && self.sprite_pool.reserve(lease)
    }

    pub(crate) fn accept_reserved_sprite(&mut self, lease: SpriteBufferLease) -> bool {
        self.sprite_pool.accept_reserved(lease)
    }

    pub(crate) fn can_accept_reserved_sprite(&self, lease: SpriteBufferLease) -> bool {
        !self.lease_failures.fail_next(CursorLeaseBoundary::Accept)
            && self.sprite_pool.can_accept_reserved(lease)
    }

    #[cfg(test)]
    pub(crate) fn for_tests_script_lease_failure_after(
        &mut self,
        boundary: CursorLeaseBoundary,
        matching_calls_to_skip: usize,
    ) {
        self.lease_failures.push(boundary, matching_calls_to_skip);
    }

    pub(crate) fn reject_reserved_sprite(&mut self, lease: SpriteBufferLease) -> bool {
        self.sprite_pool.reject_reserved(lease)
    }

    pub(crate) fn release_sprite_user(&mut self, lease: SpriteBufferLease) -> bool {
        self.sprite_pool.release_user(lease)
    }

    #[cfg(test)]
    pub(crate) fn sprite_user_count(&self, lease: SpriteBufferLease) -> Option<usize> {
        self.sprite_pool.user_count(lease)
    }

    #[cfg(test)]
    pub(crate) fn sprite_pool_users_are_zero(&self) -> bool {
        self.sprite_pool.all_users_zero()
    }

    pub(crate) fn disarm_sprite_pool(&mut self) {
        self.sprite_pool.disarm();
    }

    pub(crate) fn recover_sprite_pool_after_all_off(&mut self) {
        self.sprite_pool.recover_after_all_off();
    }

    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "A4 Backend tests inspect staged and retryable cursor versions"
    )]
    pub(crate) fn for_tests_sprite_pool_versions(&self) -> (Option<u64>, Option<u64>) {
        (
            self.sprite_pool.wanted_version(),
            self.sprite_pool.candidate_version(),
        )
    }

    /// Invalidate the tracked uploaded version. The next
    /// `upload_image` will memcpy unconditionally regardless of
    /// version. Used by global recovery paths (VT-leave, full
    /// modeset, `drain_all`).
    #[allow(
        dead_code,
        reason = "legacy cursor upload invalidation remains for tasks B/C cleanup"
    )]
    pub fn invalidate_uploaded_version(&mut self) {
        self.uploaded_version = None;
    }

    /// Legacy adapter: make the cursor visible on `crtc` at image-top-left
    /// position `(img_x, img_y)` with the given `hotspot`.
    /// Idempotent — repeated calls just re-bind and reposition.
    ///
    /// # Errors
    /// Ioctl failure.
    #[allow(
        dead_code,
        reason = "legacy cursor adapter remains for tasks B/C cleanup"
    )]
    pub fn show(
        &mut self,
        crtc: crtc::Handle,
        hotspot: (i32, i32),
        img_x: i32,
        img_y: i32,
    ) -> Result<(), CursorShowError> {
        log::debug!(
            "cursor_plane::show CRTC={crtc:?} hotspot=({},{}) pos=({img_x},{img_y}) \
             prior_visible={}",
            hotspot.0,
            hotspot.1,
            self.visible.get(&crtc).copied().unwrap_or(false),
        );
        match &self.operations {
            CursorPlaneOperations::Legacy(io) => {
                let Some(buffer) = self.cursor_buffer.as_ref() else {
                    return Err(CursorShowError::Unbound(io::Error::other(
                        "cursor plane already destroyed",
                    )));
                };
                let prior_visible = self.is_visible_on(crtc);
                show_with_rollback(
                    &mut self.visible,
                    crtc,
                    prior_visible,
                    || io.bind(&self.device, crtc, Some(buffer), hotspot),
                    || io.position(&self.device, crtc, img_x, img_y),
                    || io.bind(&self.device, crtc, None, (0, 0)),
                )
            }
            CursorPlaneOperations::Executors { hide, show } => {
                let Some(buffer) = self.dumb.as_ref() else {
                    return Err(CursorShowError::Unbound(io::Error::other(
                        "cursor plane already destroyed",
                    )));
                };
                let prior_visible = self.is_visible_on(crtc);
                show_with_rollback(
                    &mut self.visible,
                    crtc,
                    prior_visible,
                    || show.show(&self.device, buffer, crtc, hotspot, img_x, img_y),
                    || show.move_to(&self.device, crtc, img_x, img_y),
                    || hide.hide(&self.device, crtc),
                )
            }
        }
    }

    /// Legacy adapter: detach the cursor from `crtc` while retaining its
    /// backing buffer for a later legacy `show`.
    ///
    /// # Errors
    /// Legacy bind executor failure.
    pub fn hide(&mut self, crtc: crtc::Handle) -> io::Result<()> {
        log::debug!(
            "cursor_plane::hide CRTC={crtc:?} prior_visible={}",
            self.visible.get(&crtc).copied().unwrap_or(false),
        );
        self.hide_legacy(crtc)?;
        if !self.is_visible()
            && let Some(lease) = self.sprite_pool.candidate_lease()
        {
            self.sprite_pool.cancel_candidate(lease);
        }
        Ok(())
    }

    #[allow(deprecated)]
    fn hide_legacy(&mut self, crtc: crtc::Handle) -> io::Result<()> {
        match &self.operations {
            CursorPlaneOperations::Legacy(io) => io.bind(&self.device, crtc, None, (0, 0))?,
            CursorPlaneOperations::Executors { hide, .. } => hide.hide(&self.device, crtc)?,
        }
        self.visible.insert(crtc, false);
        Ok(())
    }

    /// Legacy adapter: move the cursor on `crtc` to image-top-left `(x, y)` in
    /// CRTC-local coordinates. The KMS platform uses its atomic cursor path.
    ///
    /// # Errors
    /// `move_cursor` ioctl failure.
    #[allow(dead_code)] // definition retained until Task B deletes legacy cursor ioctls
    #[allow(deprecated)]
    pub fn move_to(&self, crtc: crtc::Handle, x: i32, y: i32) -> io::Result<()> {
        match &self.operations {
            CursorPlaneOperations::Legacy(io) => io.position(&self.device, crtc, x, y),
            CursorPlaneOperations::Executors { show, .. } => show.move_to(&self.device, crtc, x, y),
        }
    }

    /// True iff the plane is currently bound (via `show`) on `crtc`.
    #[allow(
        dead_code,
        reason = "legacy visibility map remains for tasks B/C cleanup"
    )]
    #[must_use]
    pub fn is_visible_on(&self, crtc: crtc::Handle) -> bool {
        self.visible.get(&crtc).copied().unwrap_or(false)
    }

    /// True iff the plane is currently bound on at least one CRTC.
    #[allow(dead_code)] // diagnostic accessor; no v2 production callers
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.visible.values().any(|&v| v)
    }

    /// Iterate every CRTC the plane has ever been bound or hidden
    /// against.
    pub fn known_crtcs(&self) -> impl Iterator<Item = crtc::Handle> + '_ {
        self.visible.keys().copied()
    }

    /// Forget visibility records for routes no longer active on this device.
    /// Raw CRTC handles may be reused after a topology change; carrying an old
    /// `true` into the new route would skip the required show transition.
    pub fn retain_crtcs(&mut self, active: &HashSet<crtc::Handle>) {
        self.visible.retain(|crtc, _| active.contains(crtc));
        self.sprite_pool.set_active_crtc_count(active.len());
    }

    /// Cursor plane width in pixels (driver-reported).
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Cursor plane height in pixels (driver-reported).
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Diagnostic: write the current dumb buffer contents to a PPM file
    /// at `path`. Reads the kernel-visible bytes (respecting `self.stride`)
    /// so a stride/pitch mismatch between what `load_image` writes and
    /// what the display engine samples shows up as a visible distortion
    /// in the dump.
    ///
    /// # Errors
    /// File I/O failure.
    pub fn dump_to_ppm(&self, path: &str) -> io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(path)?;
        let w = self.width;
        let h = self.height;
        file.write_all(format!("P6\n{w} {h}\n255\n").as_bytes())?;
        let mut row_buf = vec![0u8; (w as usize) * 3];
        for y in 0..h as usize {
            let row_start = y * (self.stride as usize);
            for x in 0..w as usize {
                let pi = row_start + x * 4;
                // ARGB8888 in little-endian on the wire is B, G, R, A bytes.
                let b = unsafe { *self.ptr.as_ptr().add(pi) };
                let g = unsafe { *self.ptr.as_ptr().add(pi + 1) };
                let r = unsafe { *self.ptr.as_ptr().add(pi + 2) };
                row_buf[x * 3] = r;
                row_buf[x * 3 + 1] = g;
                row_buf[x * 3 + 2] = b;
            }
            file.write_all(&row_buf)?;
        }
        log::info!("cursor: dumped {path} ({w}x{h}, stride={})", self.stride);
        Ok(())
    }
}

impl Drop for CursorPlane {
    fn drop(&mut self) {
        // Best-effort: hide cursor on all known CRTCs before releasing resources.
        let crtcs: Vec<crtc::Handle> = self.known_crtcs().collect();
        for crtc in crtcs {
            if self.visible.get(&crtc).copied().unwrap_or(false)
                && let Err(e) = self.hide(crtc)
            {
                log::debug!("cursor: hide on drop for {crtc:?} failed: {e}");
            }
        }
        self.dumb.take();
        self.cursor_buffer.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_crtc(raw: u32) -> crtc::Handle {
        ::drm::control::from_u32(raw).unwrap()
    }

    fn physical_caps() -> crate::drm::CursorDeviceCaps {
        crate::drm::CursorDeviceCaps::new(false, "test-physical".to_owned())
    }

    fn discovered(handle: u32, crtcs: &[crtc::Handle], atomic: bool) -> DiscoveredCursorPlane {
        let property_names: &[&str] = if atomic {
            &[
                "FB_ID", "CRTC_ID", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X", "SRC_Y",
                "SRC_W", "SRC_H",
            ]
        } else {
            &["CRTC_X", "CRTC_Y"]
        };
        DiscoveredCursorPlane {
            handle: ::drm::control::from_u32(handle).unwrap(),
            crtcs: crtcs.iter().copied().collect(),
            properties: property_names
                .iter()
                .enumerate()
                .map(|(idx, name)| {
                    (
                        (*name).to_owned(),
                        ::drm::control::from_u32(handle * 10 + idx as u32 + 1).unwrap(),
                    )
                })
                .collect(),
        }
    }

    fn assigned_plane(topology: &CursorPlaneTopology, crtc: crtc::Handle) -> Option<plane::Handle> {
        topology.get(&crtc).map(|properties| properties.plane)
    }

    #[test]
    // Mutation: pick the first compatible plane per CRTC independently. The
    // shared plane 21 would then serve both CRTCs, and CRTC a would not use
    // its own exclusive plane 22 (the kernel's legacy cursor plane for a).
    fn cursor_planes_prefer_exclusive_and_never_serve_two_crtcs() {
        let a = test_crtc(11);
        let b = test_crtc(12);
        let planes = [discovered(21, &[a, b], true), discovered(22, &[a], true)];
        let caps = physical_caps();
        let topology = cursor_plane_topology(&[a, b], &planes, Some(&caps));

        assert_eq!(assigned_plane(&topology, a), Some(planes[1].handle));
        assert_eq!(assigned_plane(&topology, b), Some(planes[0].handle));
        assert_eq!(topology[&a].plane, planes[1].handle);
        assert_eq!(topology[&b].plane, planes[0].handle);
    }

    #[test]
    fn cursor_plane_atomic_positions_come_only_from_the_assigned_plane() {
        let a = test_crtc(11);
        let b = test_crtc(12);
        let planes = [discovered(21, &[a], false), discovered(22, &[a, b], true)];
        let caps = physical_caps();
        let topology = cursor_plane_topology(&[a, b], &planes, Some(&caps));

        assert_eq!(topology.len(), 1);
        assert!(
            topology
                .values()
                .all(|properties| properties.plane == planes[1].handle)
        );
    }

    #[test]
    fn cursor_plane_assignment_leaves_an_uncovered_crtc_unassigned() {
        let a = test_crtc(11);
        let b = test_crtc(12);
        let device = Rc::new(crate::drm::Device::for_tests().expect("test DRM device"));
        let mut plane = CursorPlane::for_tests_stub(device, 64, 64);
        let shared_only = [discovered(21, &[a, b], true)];
        let caps = physical_caps();
        plane.properties_by_crtc = cursor_plane_topology(&[a, b], &shared_only, Some(&caps));

        assert!(plane.supports_crtcs(&[a]));
        assert!(!plane.supports_crtcs(&[b]));
        assert!(!plane.supports_crtcs(&[a, b]));
        assert!(plane.supports_atomic_positions(&[a]));
        assert!(!plane.supports_atomic_positions(&[b]));

        let independently_drivable = [discovered(21, &[a, b], true), discovered(22, &[a, b], true)];
        let topology = cursor_plane_topology(&[a, b], &independently_drivable, Some(&caps));
        assert_ne!(assigned_plane(&topology, a), assigned_plane(&topology, b));
        assert!(assigned_plane(&topology, b).is_some());
        assert!(cursor_plane_topology(&[a, b], &[], None).is_empty());
    }

    fn assert_every_crtc_has_its_own_plane(crtcs: &[crtc::Handle], topology: &CursorPlaneTopology) {
        let assigned: Vec<_> = crtcs
            .iter()
            .map(|&crtc| assigned_plane(topology, crtc).expect("every CRTC gets a cursor plane"))
            .collect();
        let distinct: HashSet<_> = assigned.iter().collect();
        assert_eq!(distinct.len(), assigned.len(), "a plane serves two CRTCs");
    }

    #[test]
    // Mutation: greedy first-fit gives a→21, b→none, c→22; a maximum matching
    // reroutes a to 22 or 23 so that b can take 21.
    fn cursor_plane_assignment_is_a_maximum_matching() {
        let (a, b, c) = (test_crtc(11), test_crtc(12), test_crtc(13));
        let planes = [
            discovered(21, &[a, b], true),
            discovered(22, &[a, c], true),
            discovered(23, &[a, c], true),
        ];
        let caps = physical_caps();
        let topology = cursor_plane_topology(&[a, b, c], &planes, Some(&caps));

        assert_every_crtc_has_its_own_plane(&[a, b, c], &topology);
    }

    #[test]
    // Mutation: let augmenting paths reassign exclusive planes. CRTC c's
    // exclusive plane 23 must stay with c while a and b share the rest.
    fn cursor_plane_matching_keeps_exclusive_planes() {
        let (a, b, c) = (test_crtc(11), test_crtc(12), test_crtc(13));
        let planes = [
            discovered(21, &[a, b, c], true),
            discovered(22, &[a, c], true),
            discovered(23, &[c], true),
        ];
        let caps = physical_caps();
        let topology = cursor_plane_topology(&[a, b, c], &planes, Some(&caps));

        assert_every_crtc_has_its_own_plane(&[a, b, c], &topology);
        assert_eq!(assigned_plane(&topology, c), Some(planes[2].handle));
        assert_eq!(assigned_plane(&topology, a), Some(planes[1].handle));
        assert_eq!(assigned_plane(&topology, b), Some(planes[0].handle));
    }

    #[test]
    fn cursor_keeps_atomic_positions_across_a_zero_crtc_topology_step() {
        let crtc = test_crtc(13);
        let plane_handle = ::drm::control::from_u32(21).unwrap();
        let crtc_x = ::drm::control::from_u32(31).unwrap();
        let crtc_y = ::drm::control::from_u32(32).unwrap();
        let device = Rc::new(crate::drm::Device::for_tests().expect("test DRM device"));
        let mut plane = CursorPlane::for_tests_stub(device, 64, 64);
        plane.for_tests_set_atomic_position_properties(crtc, plane_handle, crtc_x, crtc_y);

        plane.retain_crtcs(&HashSet::new());
        plane.retain_crtcs(&HashSet::from([crtc]));

        assert_eq!(
            plane.atomic_position_properties(crtc),
            Some((plane_handle, crtc_x, crtc_y))
        );
        assert!(plane.supports_atomic_positions(&[crtc]));
        assert!(!plane.is_visible_on(crtc));
    }

    #[test]
    fn failed_rebind_preserves_prior_visibility() {
        let error = bind_failure(true, io::Error::from_raw_os_error(libc::EINVAL));
        assert!(error.remains_visible());
        assert!(error.rollback_error().is_none());

        let error = bind_failure(false, io::Error::from_raw_os_error(libc::EINVAL));
        assert!(!error.remains_visible());
    }

    #[test]
    fn failed_move_reports_hide_rollback_result() {
        let error = move_failure(io::Error::from_raw_os_error(libc::EINVAL), Ok(()));
        assert!(!error.remains_visible());

        let error = move_failure(
            io::Error::from_raw_os_error(libc::EINVAL),
            Err(io::Error::from_raw_os_error(libc::ENODEV)),
        );
        assert!(error.remains_visible());
        assert_eq!(
            error.rollback_error().and_then(io::Error::raw_os_error),
            Some(libc::ENODEV)
        );
    }

    #[test]
    // Mutation: invoke an injected show executor before checking for a dumb buffer.
    fn show_executor_does_not_bypass_missing_dumb_buffer_check() {
        use std::cell::Cell;

        struct CountingShowExecutor(Cell<usize>);

        impl CursorShowExecutor for CountingShowExecutor {
            fn show(
                &self,
                _device: &Device,
                _buffer: &CursorPlaneBuffer,
                _crtc: crtc::Handle,
                _hotspot: (i32, i32),
                _img_x: i32,
                _img_y: i32,
            ) -> io::Result<()> {
                self.0.set(self.0.get() + 1);
                Ok(())
            }

            fn move_to(
                &self,
                _device: &Device,
                _crtc: crtc::Handle,
                _img_x: i32,
                _img_y: i32,
            ) -> io::Result<()> {
                Ok(())
            }
        }

        let device = Rc::new(crate::drm::Device::for_tests().expect("test DRM device"));
        let executor = Rc::new(CountingShowExecutor(Cell::new(0)));
        let mut plane =
            CursorPlane::for_tests_with_show_executor(device, 64, 64, false, executor.clone());

        let error = plane
            .show(test_crtc(11), (0, 0), 10, 10)
            .expect_err("a plane without a dumb buffer cannot be shown");
        assert!(!error.remains_visible());
        assert_eq!(
            executor.0.get(),
            0,
            "executor must not run without a buffer"
        );
        assert!(!plane.is_visible_on(test_crtc(11)));
    }

    /// Phase B regression: `is_visible_on` tracks per-CRTC binding
    /// independently.
    #[test]
    fn visibility_is_per_crtc() {
        let mut visible: HashMap<crtc::Handle, bool> = HashMap::new();
        let crtc_a: crtc::Handle = ::drm::control::from_u32(11).unwrap();
        let crtc_b: crtc::Handle = ::drm::control::from_u32(12).unwrap();

        visible.insert(crtc_a, true);
        assert!(visible.get(&crtc_a).copied().unwrap_or(false));
        assert!(!visible.get(&crtc_b).copied().unwrap_or(false));

        visible.insert(crtc_b, true);
        assert!(visible.get(&crtc_a).copied().unwrap_or(false));
        assert!(visible.get(&crtc_b).copied().unwrap_or(false));

        visible.insert(crtc_a, false);
        assert!(!visible.get(&crtc_a).copied().unwrap_or(false));
        assert!(visible.get(&crtc_b).copied().unwrap_or(false));
    }

    /// Phase B regression test for the unavailable-plane path. The
    /// v2 `PlatformBackend::for_tests()` fixture has no real DRM
    /// device, so `cursor_plane` is `None`. The hooks must surface
    /// that cleanly via `Err(io::Error::other(...))` rather than
    /// panicking — every Phase D' recovery path relies on this so
    /// VT-leave / shutdown / drain_all hooks can fire blindly.
    #[test]
    fn unavailable_plane_returns_err_not_panic() {
        use crate::kms::render::platform::PlatformBackend;

        let mut p = PlatformBackend::for_tests();
        assert!(!p.cursor_plane_available());
        assert!(
            p.cursor_plane_upload_image_for_output(0, 1, 0, 0, 16, 16, &[0u8; 16 * 16 * 4])
                .is_err()
        );
        assert!(p.cursor_plane_show_on_crtc(0, 0, 0, 0, 0).is_err());
        assert!(p.cursor_plane_move(0, 0).is_err());
        assert!(p.cursor_plane_hide_on_crtc(0).is_err());
        assert!(p.cursor_plane_hide_all().is_err());
        assert!(p.cursor_plane_uploaded_version_for_output(0).is_none());
    }
}
