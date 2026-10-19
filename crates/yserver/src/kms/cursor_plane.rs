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
pub(crate) mod ops;
pub(crate) mod sprites;
pub(crate) mod topology;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    io,
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
    control::{crtc, plane, property},
};

use crate::drm::Device;
#[cfg(test)]
use ops::RecordingCursorBufferIo;
use ops::{CursorBufferIo, DrmCursorBufferIo};
use sprites::{SpriteBufferLease, SpritePool};
use topology::{
    CursorPlaneDiscovery, CursorPlaneDiscoveryIo, CursorPlaneProperties, DrmCursorPlaneDiscovery,
    cursor_plane_topology,
};

#[cfg(test)]
pub(crate) use topology::{CursorPlaneTestPlane, CursorPlaneTestSnapshot};
#[cfg(test)]
use topology::{CursorPlaneTopology, DiscoveredCursorPlane};

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

/// Cursor image storage, sprite leases, and per-CRTC atomic plane properties.
pub struct CursorPlane {
    device: Rc<Device>,
    buffer_io: Rc<dyn CursorBufferIo>,
    sprite_pool: SpritePool,
    lease_failures: CursorLeaseFailureScript,
    /// Cursor buffer dimensions in pixels. Sourced from
    /// `DriverCapability::CursorWidth/Height`. Mandatory match with the
    /// sprite framebuffer geometry — see [`HW_CURSOR_FALLBACK_W`].
    width: u32,
    height: u32,
    /// Candidate image version last staged in the sprite pool. This avoids
    /// rewriting an identical candidate while the applied binding remains
    /// independently tracked by the CRTC state.
    uploaded_version: Option<u64>,
    deferred_upload: Option<(u64, u16, u16)>,
    /// Once retryable, eligibility remains until the wanted image is staged;
    /// pool pressure only delays the first attempt until a lease is released.
    deferred_upload_waiting_for_buffer_release: bool,
    /// Prevent duplicate attempts for one sprite during a scheduler pass.
    deferred_upload_attempted_this_pass: Option<(u64, u16, u16)>,
    /// Qualified, exclusive assignment for each CRTC. A missing entry means
    /// this output must use the software cursor.
    properties_by_crtc: HashMap<crtc::Handle, CursorPlaneProperties>,
    discovery: CursorPlaneDiscovery,
    discovery_io: Rc<dyn CursorPlaneDiscoveryIo>,
    active_crtcs: HashSet<crtc::Handle>,
    pending_discovery: Option<CursorPlaneDiscovery>,
    #[cfg(test)]
    test_discovery: Option<Rc<RecordingCursorPlaneDiscovery>>,
}

/// Constructs cursor-plane state at startup and topology retry boundaries.
/// Production and test implementations share the same caller and lifecycle.
pub(crate) trait CursorPlaneFactory {
    fn create(
        &self,
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
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
    ) -> io::Result<CursorPlane> {
        CursorPlane::new_with_buffer_io(device, crtcs, buffer_io)
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
    ) -> io::Result<CursorPlane> {
        self.calls.set(self.calls.get() + 1);
        match self.queued.borrow_mut().pop_front() {
            Some(CursorPlaneFactoryResult::Error(raw_os_error)) => {
                Err(io::Error::from_raw_os_error(raw_os_error))
            }
            Some(CursorPlaneFactoryResult::Snapshot(snapshot)) => {
                CursorPlane::for_tests_with_discovery_snapshot(device, crtcs, buffer_io, snapshot)
                    .map(|(plane, _)| plane)
            }
            None => DrmCursorPlaneFactory.create(device, crtcs, buffer_io),
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
    /// Qualify discovered atomic cursor planes independently for each
    /// assigned CRTC. Cursor framebuffers are allocated lazily when a sprite
    /// is staged. CRTCs without an eligible assignment use software
    /// composition.
    ///
    /// # Errors
    /// Cursor-plane topology discovery failure.
    #[allow(dead_code)]
    pub fn new(device: Rc<Device>, crtcs: &[crtc::Handle]) -> io::Result<Self> {
        Self::new_with_buffer_io(device, crtcs, Rc::new(DrmCursorBufferIo))
    }

    #[allow(dead_code)]
    pub(crate) fn new_with_buffer_io(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
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
        discovery_io: Rc<dyn CursorPlaneDiscoveryIo>,
        discovery: CursorPlaneDiscovery,
        width: u32,
        height: u32,
    ) -> io::Result<Self> {
        let properties_by_crtc =
            cursor_plane_topology(&discovery.crtcs, &discovery.planes, discovery.caps.as_ref());
        log::info!("cursor: driver reports CursorWidth={width} CursorHeight={height}");

        let mut sprite_pool = SpritePool::new(width, height);
        sprite_pool.set_active_crtc_count(crtcs.len());
        let plane = Self {
            device,
            buffer_io,
            sprite_pool,
            lease_failures: CursorLeaseFailureScript::default(),
            width,
            height,
            uploaded_version: None,
            deferred_upload: None,
            deferred_upload_waiting_for_buffer_release: false,
            deferred_upload_attempted_this_pass: None,
            properties_by_crtc,
            discovery,
            discovery_io,
            active_crtcs: crtcs.iter().copied().collect(),
            pending_discovery: None,
            #[cfg(test)]
            test_discovery: None,
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
    ) -> io::Result<Self> {
        Self::for_tests_with_capabilities(device, crtcs, buffer_io, Some(64), Some(64))
    }

    #[cfg(test)]
    pub(crate) fn for_tests_with_capabilities(
        device: Rc<Device>,
        crtcs: &[crtc::Handle],
        buffer_io: Rc<dyn CursorBufferIo>,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) -> io::Result<Self> {
        Self::for_tests_with_capabilities_and_hotspot_properties(
            device,
            crtcs,
            buffer_io,
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
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) -> io::Result<Self> {
        Self::for_tests_with_capabilities_and_hotspot_properties(
            device,
            crtcs,
            buffer_io,
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
        snapshot: CursorPlaneTestSnapshot,
    ) -> io::Result<(Self, Rc<RecordingCursorPlaneDiscovery>)> {
        let discovery = snapshot.into_discovery();
        let discovery_io = Rc::new(RecordingCursorPlaneDiscovery::new(discovery.clone()));
        let mut plane = Self::from_discovery(
            device,
            crtcs,
            buffer_io,
            discovery_io.clone(),
            discovery,
            HW_CURSOR_FALLBACK_W,
            HW_CURSOR_FALLBACK_H,
        )?;
        plane.test_discovery = Some(discovery_io.clone());
        Ok((plane, discovery_io))
    }

    /// Build an ioctl-free cursor plane for platform state-machine tests.
    #[cfg(test)]
    pub(crate) fn for_tests_stub(device: Rc<Device>, width: u32, height: u32) -> Self {
        let (buffer_io, _) = RecordingCursorBufferIo::new(1);
        Self {
            device: Rc::clone(&device),
            buffer_io: Rc::new(buffer_io),
            sprite_pool: SpritePool::new(width, height),
            lease_failures: CursorLeaseFailureScript::default(),
            width,
            height,
            uploaded_version: None,
            deferred_upload: None,
            deferred_upload_waiting_for_buffer_release: false,
            deferred_upload_attempted_this_pass: None,
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
        }
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

    /// Upload a versioned image into a candidate sprite buffer. Binding stays
    /// owned by the atomic platform path.
    ///
    /// # Errors
    /// Driver buffer allocation or sprite image upload failure.
    pub fn upload_image(
        &mut self,
        version: u64,
        hot_x: u16,
        hot_y: u16,
        image_w: u32,
        image_h: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<yserver_render::kms::render::scene::CursorUploadOutcome> {
        if self.deferred_upload_attempted_this_pass == Some((version, hot_x, hot_y)) {
            return Ok(yserver_render::kms::render::scene::CursorUploadOutcome::Deferred);
        }
        if self.uploaded_image_matches(version, hot_x, hot_y) {
            self.clear_deferred_upload();
            return Ok(yserver_render::kms::render::scene::CursorUploadOutcome::Ready);
        }
        let prepared = self.sprite_pool.prepare(
            Rc::clone(&self.device),
            Rc::clone(&self.buffer_io),
            version,
            hot_x,
            hot_y,
            image_w,
            image_h,
            bgra_bytes,
        );
        match prepared {
            Ok(yserver_render::kms::render::scene::CursorUploadOutcome::Ready) => {
                self.clear_deferred_upload();
            }
            Ok(deferred @ yserver_render::kms::render::scene::CursorUploadOutcome::Deferred) => {
                self.deferred_upload = Some((version, hot_x, hot_y));
                self.deferred_upload_waiting_for_buffer_release =
                    self.sprite_pool.full_with_live_users();
                self.deferred_upload_attempted_this_pass = Some((version, hot_x, hot_y));
                return Ok(deferred);
            }
            Err(error) => {
                // Keep the newest failed upload eligible. The caller clears it
                // only when the error rejects this hardware path or sprite key.
                self.deferred_upload = Some((version, hot_x, hot_y));
                self.deferred_upload_waiting_for_buffer_release =
                    self.sprite_pool.full_with_live_users();
                self.deferred_upload_attempted_this_pass = Some((version, hot_x, hot_y));
                return Err(error);
            }
        }
        self.uploaded_version = Some(version);
        Ok(yserver_render::kms::render::scene::CursorUploadOutcome::Ready)
    }

    pub(crate) fn clear_deferred_upload(&mut self) {
        self.deferred_upload = None;
        self.deferred_upload_waiting_for_buffer_release = false;
        self.deferred_upload_attempted_this_pass = None;
    }

    pub(crate) fn reset_deferred_upload_attempt(&mut self) {
        self.deferred_upload_attempted_this_pass = None;
    }

    pub(crate) fn deferred_upload_matches(&self, version: u64, hot_x: u16, hot_y: u16) -> bool {
        self.deferred_upload == Some((version, hot_x, hot_y))
            && self.deferred_upload_attempted_this_pass != Some((version, hot_x, hot_y))
            && (!self.deferred_upload_waiting_for_buffer_release
                || self.sprite_pool.has_available_buffer())
    }

    /// The image version most recently staged as a candidate, if any.
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

    #[cfg(test)]
    pub(crate) fn for_tests_prepared_sprite_user_count(&self) -> usize {
        self.sprite_pool.prepared_user_count()
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
    pub fn invalidate_uploaded_version(&mut self) {
        self.uploaded_version = None;
    }

    pub(crate) fn set_active_crtc_count(&mut self, count: usize) {
        self.sprite_pool.set_active_crtc_count(count);
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

    /// Dump the exact sprite buffer retained by an accepted CRTC binding.
    pub(crate) fn dump_applied_sprite_to_ppm(
        &self,
        lease: SpriteBufferLease,
        path: &str,
    ) -> io::Result<()> {
        self.sprite_pool.dump_sprite_to_ppm(lease, path)
    }

    #[cfg(test)]
    pub(crate) fn for_tests_sprite_bytes(&self, lease: SpriteBufferLease) -> Option<Vec<u8>> {
        self.sprite_pool.for_tests_sprite_bytes(lease)
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
    // its own exclusive plane 22 for CRTC a.
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

        plane.set_active_crtc_count(0);
        plane.set_active_crtc_count(1);

        assert_eq!(
            plane.atomic_position_properties(crtc),
            Some((plane_handle, crtc_x, crtc_y))
        );
        assert!(plane.supports_atomic_positions(&[crtc]));
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
