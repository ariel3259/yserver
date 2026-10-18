use crate::kms::{
    cursor_plane::CursorPlaneFactory,
    render::{
        platform::{
            CrtcKey, CursorFailureDisposition, CursorMoveOutcome, KmsCursorState, KmsDevice,
            PlatformBackend, TransientCursorFallback,
            cursor_state::{CursorBinding, CursorIntent, CursorPlaneUpdate},
        },
        scene::{CursorEntry, CursorFrameAssignment, CursorUploadOutcome},
    },
};
use std::{
    collections::{HashMap, HashSet},
    io,
    rc::Rc,
    time::Instant,
};

/// True iff a cursor-plane ioctl error means the driver does not
/// implement the (legacy) cursor ioctls at all — a permanent,
/// per-driver condition that warrants latching the HW cursor strategy
/// off and falling back to the SW composite path.
///
/// Apple's DCP display driver (Asahi) returns `ENXIO` from
/// `DRM_IOCTL_MODE_CURSOR2`; other atomic-only drivers may return
/// `ENODEV` / `EOPNOTSUPP`. Recoverable / ambiguous errors (`EBUSY`,
/// `EINVAL`, non-OS errors) must NOT latch — `EBUSY` is transient and
/// latching it would needlessly kill the HW cursor on drivers that do
/// support the ioctl (e.g. amdgpu). This mirrors Xorg's modesetting
/// driver, which clears `use_hw_cursor` when the cursor ioctl fails.
pub(super) fn cursor_err_disables_hw(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENXIO | libc::ENODEV | libc::EOPNOTSUPP)
    )
}

fn cursor_error_is_transient_fallback(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidInput
}

fn classify_cursor_failure_pair(
    operation_error: &io::Error,
    rollback_error: Option<&io::Error>,
) -> CursorFailureDisposition {
    if cursor_err_disables_hw(operation_error) || rollback_error.is_some_and(cursor_err_disables_hw)
    {
        CursorFailureDisposition::Permanent
    } else if cursor_error_is_transient_fallback(operation_error)
        || rollback_error.is_some_and(cursor_error_is_transient_fallback)
    {
        CursorFailureDisposition::Transient
    } else {
        CursorFailureDisposition::Unchanged
    }
}

pub(super) fn cursor_dimensions_fit(
    plane_width: u32,
    plane_height: u32,
    width: u32,
    height: u32,
) -> bool {
    width <= plane_width && height <= plane_height
}

impl KmsCursorState {
    pub(crate) fn new() -> Self {
        Self {
            plane: None,
            crtc_states: HashMap::new(),
            pending_move: None,
            pending_move_timestamp: None,
            atomic_cursor_commits_in_flight: HashMap::new(),
            cursor_retry_waits_for_flip: HashSet::new(),
            cursor_hide_recovery: HashMap::new(),
            permanently_disabled: false,
            initialization_retryable: false,
            headless_deferred: true,
            transient_fallback_crtcs: HashMap::new(),
            cursor_topology_epoch: 0,
            cursor_topology_crtcs: HashSet::new(),
            sprite_signature: None,
        }
    }

    fn clear_atomic_position(&mut self, crtc: ::drm::control::crtc::Handle) {
        if let Some(state) = self.crtc_states.get_mut(&crtc) {
            state.intent_motion_timestamp = None;
        }
    }

    fn available_on(&self, crtc: ::drm::control::crtc::Handle) -> bool {
        self.plane.is_some()
            && !self.permanently_disabled
            && self
                .plane
                .as_ref()
                .is_some_and(|plane| plane.properties_for(crtc).is_some())
            && self
                .transient_fallback_crtcs
                .get(&crtc)
                .is_none_or(|retry| retry.remaining_sw_retires == 0)
    }

    pub(super) fn note_einval(&mut self, crtc: ::drm::control::crtc::Handle) {
        self.note_transient_fallback(crtc);
    }

    fn note_transient_fallback(&mut self, crtc: ::drm::control::crtc::Handle) {
        let retry = self
            .transient_fallback_crtcs
            .entry(crtc)
            .or_insert(TransientCursorFallback {
                remaining_sw_retires: 0,
                failures: 0,
            });
        retry.failures = retry.failures.saturating_add(1);
        let shift = retry.failures.saturating_sub(1).min(3);
        retry.remaining_sw_retires = 1_u8 << shift;
    }

    fn note_cursor_success(&mut self, crtc: ::drm::control::crtc::Handle) {
        self.transient_fallback_crtcs.remove(&crtc);
        self.cursor_hide_recovery.remove(&crtc);
    }

    pub(super) fn note_cursor_failure_pair(
        &mut self,
        crtc: ::drm::control::crtc::Handle,
        operation_error: &io::Error,
        rollback_error: Option<&io::Error>,
    ) -> CursorFailureDisposition {
        let disposition = classify_cursor_failure_pair(operation_error, rollback_error);
        match disposition {
            CursorFailureDisposition::Unchanged => {}
            CursorFailureDisposition::Transient => {
                self.note_transient_fallback(crtc);
                // Once eligibility changes, the cursorless scene handoff owns
                // recovery. A position-only retry must not race it or clear a
                // failed bind/hotspot/upload observation.
                self.pending_move = None;
                self.pending_move_timestamp = None;
            }
            CursorFailureDisposition::Permanent => {
                self.permanently_disabled = true;
                self.pending_move = None;
                self.pending_move_timestamp = None;
                self.transient_fallback_crtcs.clear();
            }
        }
        disposition
    }

    pub(super) fn note_initialization_failure(&mut self, error: &io::Error) {
        self.headless_deferred = false;
        let permanent = cursor_err_disables_hw(error);
        self.permanently_disabled |= permanent;
        self.initialization_retryable = !permanent;
    }

    pub(super) fn should_initialize_headless_deferred(&self, has_active_crtcs: bool) -> bool {
        has_active_crtcs
            && self.headless_deferred
            && self.plane.is_none()
            && !self.permanently_disabled
            && !self.initialization_retryable
    }

    pub(super) fn should_retry_initialization(&self, has_active_crtcs: bool) -> bool {
        has_active_crtcs
            && self.plane.is_none()
            && !self.permanently_disabled
            && self.initialization_retryable
    }
}

impl CursorMoveOutcome {
    pub(super) fn merge(&mut self, other: Self) {
        self.ebusy_count = self.ebusy_count.saturating_add(other.ebusy_count);
        self.fallback_changed |= other.fallback_changed;
        self.retry_required |= other.retry_required;
    }
}

pub(super) fn apply_cursor_move_rollback_result(
    state: &mut KmsCursorState,
    crtc: ::drm::control::crtc::Handle,
    move_error: &io::Error,
    rollback: io::Result<()>,
    outcome: &mut CursorMoveOutcome,
) -> bool {
    let disposition = state.note_cursor_failure_pair(crtc, move_error, rollback.as_ref().err());
    if disposition != CursorFailureDisposition::Unchanged {
        outcome.fallback_changed = true;
    }
    match rollback {
        Ok(()) => false,
        Err(_) => {
            outcome.retry_required = true;
            // A classified eligibility failure is recovered by the scene's
            // visibility-aware cursorless hide transaction, not by a stale
            // position-only retry. Keep pending only for an unclassified
            // ownership uncertainty.
            disposition == CursorFailureDisposition::Unchanged
        }
    }
}

pub(super) fn apply_cursor_show_failure_state(
    state: &mut KmsCursorState,
    crtc: ::drm::control::crtc::Handle,
    error: &crate::kms::cursor_plane::CursorShowError,
    desired_move: (i32, i32, u16, u16),
) -> CursorFailureDisposition {
    let disposition =
        state.note_cursor_failure_pair(crtc, error.operation_error(), error.rollback_error());
    if error.remains_visible()
        && disposition == CursorFailureDisposition::Unchanged
        && !error.needs_full_rebind()
    {
        state.pending_move = Some(desired_move);
        state.pending_move_timestamp = Some(Instant::now());
    }
    disposition
}

pub(super) fn apply_cursor_operation_result(
    state: &mut KmsCursorState,
    crtc: ::drm::control::crtc::Handle,
    result: &io::Result<()>,
) -> CursorFailureDisposition {
    result
        .as_ref()
        .err()
        .map_or(CursorFailureDisposition::Unchanged, |error| {
            state.note_cursor_failure_pair(crtc, error, None)
        })
}

pub(super) fn install_cursor_plane_for_device(
    kms_device: &mut KmsDevice,
    crtcs: &[::drm::control::crtc::Handle],
    boundary: &str,
    plane: crate::kms::cursor_plane::CursorPlane,
) {
    let active_crtcs: HashSet<_> = crtcs.iter().copied().collect();
    let assignments_changed = crtcs.iter().any(|crtc| {
        kms_device
            .cursor
            .plane
            .as_ref()
            .and_then(|previous| previous.properties_for(*crtc))
            != plane.properties_for(*crtc)
    });
    if kms_device.cursor.cursor_topology_crtcs != active_crtcs || assignments_changed {
        kms_device.cursor.cursor_topology_epoch =
            kms_device.cursor.cursor_topology_epoch.saturating_add(1);
        kms_device.cursor.cursor_topology_crtcs = active_crtcs;
    }
    kms_device.cursor.initialization_retryable = false;
    kms_device.cursor.headless_deferred = false;
    let eligible_crtcs = crtcs
        .iter()
        .filter(|crtc| plane.properties_for(**crtc).is_some())
        .count();
    log::info!(
        "render cursor: device {} initialized {}x{} ARGB8888 for {} active CRTC(s) at {boundary}; atomic eligible on {eligible_crtcs}",
        kms_device.key,
        plane.width(),
        plane.height(),
        crtcs.len(),
    );
    kms_device.cursor.plane = Some(plane);
}

pub(super) fn initialize_cursor_plane_for_device(
    kms_device: &mut KmsDevice,
    crtcs: &[::drm::control::crtc::Handle],
    boundary: &str,
    factory: &dyn CursorPlaneFactory,
) {
    kms_device.cursor.headless_deferred = false;
    match factory.create(
        Rc::clone(&kms_device.device),
        crtcs,
        Rc::clone(&kms_device.cursor_buffer_io),
        Rc::clone(&kms_device.legacy_cursor_io),
    ) {
        Ok(plane) => install_cursor_plane_for_device(kms_device, crtcs, boundary, plane),
        Err(error) => {
            kms_device.cursor.note_initialization_failure(&error);
            let retry = if kms_device.cursor.initialization_retryable {
                "will retry at an explicit topology/resume boundary"
            } else {
                "cursor support is permanently unavailable on this device"
            };
            log::warn!(
                "render cursor: device {} initialization failed at {boundary} ({error}); using software cursor, {retry}",
                kms_device.key
            );
        }
    }
}

#[cfg(test)]
impl PlatformBackend {
    /// Initialise synthetic active outputs with the production cursor
    /// allocation path and recorded raw cursor operations. Cursor topology
    /// is synthetic at this fixture boundary; production discovery stays DRM-backed.
    pub(crate) fn for_tests_initialize_cursor_planes(&mut self) {
        self.for_tests_initialize_cursor_planes_with_capabilities(Some(64), Some(64));
    }

    #[cfg(test)]
    pub(crate) fn for_tests_initialize_cursor_planes_with_capabilities(
        &mut self,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) {
        self.for_tests_initialize_cursor_planes_with_configuration(
            cursor_width,
            cursor_height,
            false,
        );
    }

    #[cfg(test)]
    pub(crate) fn for_tests_initialize_cursor_planes_with_hotspot_properties(
        &mut self,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) {
        self.for_tests_initialize_cursor_planes_with_configuration(
            cursor_width,
            cursor_height,
            true,
        );
    }

    #[cfg(test)]
    fn for_tests_initialize_cursor_planes_with_configuration(
        &mut self,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
        hotspot_properties: bool,
    ) {
        self.active_cursor_crtcs = self.outputs.iter().map(CrtcKey::for_output).collect();
        for device in &mut self.devices {
            let crtcs: Vec<_> = self
                .outputs
                .iter()
                .filter(|output| output.key.device_key == device.key)
                .map(|output| output.output.crtc)
                .collect();
            if !crtcs.is_empty() {
                let plane = if hotspot_properties {
                    crate::kms::cursor_plane::CursorPlane::for_tests_with_hotspot_properties(
                        Rc::clone(&device.device),
                        &crtcs,
                        Rc::clone(&device.cursor_buffer_io),
                        Rc::clone(&device.legacy_cursor_io),
                        cursor_width,
                        cursor_height,
                    )
                } else {
                    crate::kms::cursor_plane::CursorPlane::for_tests_with_capabilities(
                        Rc::clone(&device.device),
                        &crtcs,
                        Rc::clone(&device.cursor_buffer_io),
                        Rc::clone(&device.legacy_cursor_io),
                        cursor_width,
                        cursor_height,
                    )
                };
                match plane {
                    Ok(plane) => {
                        install_cursor_plane_for_device(device, &crtcs, "Vk backend fixture", plane)
                    }
                    Err(error) => {
                        device.cursor.note_initialization_failure(&error);
                        log::warn!(
                            "render cursor: device {} fixture initialization failed ({error}); using software cursor",
                            device.key
                        );
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests_queue_cursor_discovery_snapshot(
        &mut self,
        device_key: crate::platform::drm::DrmDeviceKey,
        snapshot: crate::kms::cursor_plane::CursorPlaneTestSnapshot,
    ) -> bool {
        self.devices
            .iter()
            .find(|device| device.key == device_key)
            .and_then(|device| device.cursor.plane.as_ref())
            .is_some_and(|plane| plane.for_tests_queue_discovery_snapshot(snapshot))
    }

    #[cfg(test)]
    pub(crate) fn for_tests_cursor_discovery_calls(
        &self,
        device_key: crate::platform::drm::DrmDeviceKey,
    ) -> Option<usize> {
        self.devices
            .iter()
            .find(|device| device.key == device_key)
            .and_then(|device| device.cursor.plane.as_ref())
            .and_then(crate::kms::cursor_plane::CursorPlane::for_tests_discovery_calls)
    }

    pub(crate) fn for_tests_cursor_initialization_state(
        &self,
        device_key: crate::platform::drm::DrmDeviceKey,
    ) -> Option<(bool, bool)> {
        self.devices
            .iter()
            .find(|device| device.key == device_key)
            .map(|device| {
                (
                    device.cursor.headless_deferred,
                    device.cursor.initialization_retryable,
                )
            })
    }
}

impl PlatformBackend {
    // Scene retirement and pointer scheduling use these per-CRTC adapters to
    // submit owned atomic cursor state. The legacy CursorPlane show/move/hide
    // methods remain for compatibility cleanup but are not called here.

    /// True iff the cursor plane was successfully initialised at
    /// boot AND hasn't been disabled by an auto-fallback latch. The
    /// scene strategy decision (`CursorAssignment`) gates on this
    /// without holding a `PlatformBackend` borrow.
    #[must_use]
    pub(crate) fn cursor_plane_available(&self) -> bool {
        self.outputs
            .iter()
            .enumerate()
            .any(|(output_idx, _)| self.cursor_plane_available_for_output(output_idx))
    }

    /// True iff this output's owning DRM device currently has an eligible
    /// cursor plane. Raw CRTC handles never participate in device selection.
    #[must_use]
    pub(crate) fn cursor_plane_available_for_output(&self, output_idx: usize) -> bool {
        let Some(layout) = self.outputs.get(output_idx) else {
            return false;
        };
        self.device_for_key(layout.key.device_key)
            .is_some_and(|device| device.cursor.available_on(layout.output.crtc))
    }

    #[must_use]
    pub(crate) fn cursor_output_is_active(&self, output_idx: usize) -> bool {
        self.outputs.get(output_idx).is_some_and(|output| {
            self.active_cursor_crtcs
                .contains(&CrtcKey::for_output(output))
        })
    }

    /// Invalidate only the accepted-state comparison for one device-qualified
    /// CRTC. Accepted bindings and their sprite leases remain real owners until
    /// a detach/replacement commit and its required drain settle them.
    pub(crate) fn invalidate_cursor_applied(&mut self, key: CrtcKey) {
        if let Some(state) = self
            .devices
            .iter_mut()
            .find(|device| device.key == key.device_key)
            .and_then(|device| device.cursor.crtc_states.get_mut(&key.crtc))
        {
            super::cursor_lifecycle::invalidate_applied_comparison(state);
        }
    }

    pub(crate) fn invalidate_live_cursor_applied(&mut self) {
        let routes: Vec<_> = self.outputs.iter().map(CrtcKey::for_output).collect();
        for key in routes {
            self.invalidate_cursor_applied(key);
        }
    }

    /// Reconcile the cursor policy after a resume when the effective cursor is
    /// hidden. Every eligible live route receives a full detached state after
    /// invalidation, including routes whose blocking all-off already cleared
    /// their accepted owner. Keep failed detaches' accepted bindings and leases.
    pub(crate) fn recover_hidden_cursor_on_resume(&mut self) {
        let routes: Vec<_> = self.outputs.iter().map(CrtcKey::for_output).collect();
        for key in &routes {
            self.invalidate_cursor_applied(*key);
        }
        for key in routes {
            let Some(output_idx) = self.output_index_for_crtc(key) else {
                continue;
            };
            if !self.cursor_output_is_active(output_idx) {
                continue;
            }
            if !self.cursor_plane_available_for_output(output_idx) {
                continue;
            }
            if let Err(error) = self.cursor_plane_hide_on_crtc(output_idx) {
                log::warn!(
                    "render resume: hidden cursor plane detach failed on device {} CRTC {:?}: {error}",
                    key.device_key,
                    key.crtc
                );
            }
        }
    }

    fn prepare_cursor_update_for_output(
        &self,
        output_idx: usize,
        primary_successor: bool,
    ) -> Option<CursorPlaneUpdate> {
        let layout = self.outputs.get(output_idx)?;
        if !self.cursor_output_is_active(output_idx) {
            return None;
        }
        let device = self.device_for_key(layout.key.device_key)?;
        let crtc = layout.output.crtc;
        let cursor = &device.cursor;
        let plane = cursor.plane.as_ref()?;
        let properties = plane.properties_for(crtc)?;
        let state = cursor.crtc_states.get(&crtc)?;
        if state.in_flight.is_some() && !primary_successor {
            return None;
        }
        let intent = state.intent?;
        if (cursor.permanently_disabled && matches!(intent, CursorIntent::Show(_)))
            || (cursor.cursor_hide_recovery.contains_key(&crtc)
                && matches!(intent, CursorIntent::Show(_)))
            || (!primary_successor && cursor.atomic_cursor_commits_in_flight.contains_key(&crtc))
        {
            return None;
        }
        let mut next = match intent {
            CursorIntent::Show(binding) => Some(binding),
            CursorIntent::Hide => None,
        };
        // An EBUSY cursor show may leave a replacement image staged while a
        // primary direct/unflip successor is already waiting for this CRTC.
        // Let that successor carry only the motion of the accepted binding;
        // the replacement remains the newest intent and is submitted after
        // the successor's event retires.
        if primary_successor
            && cursor.cursor_retry_waits_for_flip.contains(&crtc)
            && let (Some(prior), Some(wanted)) = (state.applied, next)
            && prior.lease != wanted.lease
        {
            next = Some(CursorBinding {
                x: wanted.x + i32::from(wanted.hot_x) - i32::from(prior.hot_x),
                y: wanted.y + i32::from(wanted.hot_y) - i32::from(prior.hot_y),
                ..prior
            });
        }
        let prepared = super::cursor_commit::prepare_cursor_commit(
            properties,
            crtc,
            state.applied,
            state.valid,
            next,
            plane.width(),
            plane.height(),
        );
        Some(CursorPlaneUpdate {
            owner: super::cursor_frames::CursorCommitOwner::HardwareOnly,
            device_key: layout.key.device_key,
            crtc,
            motion_timestamp: prepared.next.and(state.intent_motion_timestamp),
            prepared,
        })
    }

    /// Prepare the cursor delta owned by one shared scene frame. The frame's
    /// staged assignment is deliberately kept out of `CursorCrtcState.intent`,
    /// which is read only by cursor-only scheduling.
    pub(crate) fn prepare_scene_cursor_frame_update(
        &mut self,
        output_idx: usize,
        frame: crate::kms::render::scene::CursorFrameId,
        assignment: CursorFrameAssignment,
        sprite: Option<&CursorEntry>,
    ) -> io::Result<Option<CursorPlaneUpdate>> {
        let (device_idx, crtc, layout_x, layout_y) = self.cursor_output_route(output_idx)?;
        if !self.cursor_output_is_active(output_idx) {
            return Err(io::Error::other("scene cursor output is not active"));
        }

        let next = match assignment {
            CursorFrameAssignment::Hardware => {
                let sprite = sprite.ok_or_else(|| {
                    io::Error::other("hardware cursor frame has no staged sprite")
                })?;
                if !self.cursor_plane_available_for_output(output_idx)
                    || !self.cursor_plane_fits_for_output(
                        output_idx,
                        sprite.extent.width,
                        sprite.extent.height,
                    )
                {
                    return Err(io::Error::other(
                        "scene selected an unavailable hardware cursor plane",
                    ));
                }
                let bytes = sprite
                    .bgra_bytes
                    .as_ref()
                    .ok_or_else(|| io::Error::other("hardware cursor frame has no pixel data"))?;
                if !self.cursor_plane_uploaded_image_matches_for_output(
                    output_idx,
                    sprite.record_version,
                    u16::try_from(sprite.hot_x).unwrap_or(u16::MAX),
                    u16::try_from(sprite.hot_y).unwrap_or(u16::MAX),
                ) {
                    match self.cursor_plane_upload_frame_image_for_output(
                        output_idx,
                        sprite.record_version,
                        u16::try_from(sprite.hot_x).unwrap_or(u16::MAX),
                        u16::try_from(sprite.hot_y).unwrap_or(u16::MAX),
                        sprite.extent.width,
                        sprite.extent.height,
                        bytes.as_ref(),
                    )? {
                        CursorUploadOutcome::Ready => {}
                        CursorUploadOutcome::Deferred => {
                            return Err(io::Error::new(
                                io::ErrorKind::WouldBlock,
                                "scene cursor sprite buffer is still in use",
                            ));
                        }
                    }
                }
                let device = &self.devices[device_idx];
                let plane = device
                    .cursor
                    .plane
                    .as_ref()
                    .ok_or_else(|| io::Error::other("scene cursor plane disappeared"))?;
                let (lease, framebuffer) = plane
                    .candidate_sprite()
                    .ok_or_else(|| io::Error::other("scene cursor upload has no framebuffer"))?;
                let properties = plane
                    .properties_for(crtc)
                    .ok_or_else(|| io::Error::other("scene cursor properties disappeared"))?;
                let pointer = self.cursor_pointer_root;
                let (x, y) = super::cursor::cursor_root_to_crtc_local(
                    pointer.0,
                    pointer.1,
                    layout_x,
                    layout_y,
                    u16::try_from(sprite.hot_x).unwrap_or(u16::MAX),
                    u16::try_from(sprite.hot_y).unwrap_or(u16::MAX),
                );
                Some(CursorBinding {
                    properties,
                    lease,
                    framebuffer,
                    x,
                    y,
                    hot_x: u16::try_from(sprite.hot_x).unwrap_or(u16::MAX),
                    hot_y: u16::try_from(sprite.hot_y).unwrap_or(u16::MAX),
                })
            }
            CursorFrameAssignment::Software { .. } | CursorFrameAssignment::Hidden => None,
        };

        let device = &self.devices[device_idx];
        let state = device.cursor.crtc_states.get(&crtc);
        if state.is_some_and(|state| state.in_flight.is_some()) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "scene cursor frame waits for the prior cursor event",
            ));
        }
        let prior = state.and_then(|state| state.applied);
        let valid = state.is_some_and(|state| state.valid);
        let motion_timestamp = state.and_then(|state| state.intent_motion_timestamp);
        let device_key = device.key;
        let properties = next
            .map(|binding| binding.properties)
            .or_else(|| {
                device
                    .cursor
                    .plane
                    .as_ref()
                    .and_then(|plane| plane.properties_for(crtc))
            })
            .or_else(|| prior.map(|binding| binding.properties));
        let Some(properties) = properties else {
            // There is no cursor plane to detach on a software/hidden frame.
            return Ok(None);
        };
        let (width, height) = device
            .cursor
            .plane
            .as_ref()
            .map_or((0, 0), |plane| (plane.width(), plane.height()));
        let prepared = super::cursor_commit::prepare_cursor_commit(
            properties, crtc, prior, valid, next, width, height,
        );
        if let Some(state) = self.devices[device_idx].cursor.crtc_states.get_mut(&crtc) {
            // The scene frame now owns this exact observation. A rejected or
            // cancelled frame must leave retry to a later scene frame instead
            // of publishing the same transition through cursor-only work.
            state.intent = None;
            state.intent_motion_timestamp = None;
        }
        if prepared.updates.is_empty() {
            return Ok(None);
        }
        Ok(Some(CursorPlaneUpdate {
            owner: super::cursor_frames::CursorCommitOwner::Scene(frame),
            device_key,
            crtc,
            motion_timestamp,
            prepared,
        }))
    }

    /// Upload a frame-owned sprite candidate without changing any ordinary
    /// cursor-only intent. The candidate becomes applied only with the scene
    /// frame's atomic commit.
    fn cursor_plane_upload_frame_image_for_output(
        &mut self,
        output_idx: usize,
        version: u64,
        hot_x: u16,
        hot_y: u16,
        width: u32,
        height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<CursorUploadOutcome> {
        let (device_idx, crtc, _, _) = self.cursor_output_route(output_idx)?;
        let state = &mut self.devices[device_idx].cursor;
        if !state.available_on(crtc) {
            return Err(io::Error::other("cursor plane unavailable for scene frame"));
        }
        let result = state
            .plane
            .as_mut()
            .expect("available scene cursor state has a plane")
            .upload_image(version, hot_x, hot_y, width, height, bgra_bytes);
        if let Err(error) = &result {
            let failure = error.raw_os_error().map_or_else(
                || io::Error::new(error.kind(), error.to_string()),
                io::Error::from_raw_os_error,
            );
            apply_cursor_operation_result(state, crtc, &Err(failure));
        }
        result
    }

    /// Prepare full-state deltas for one or more producer-owned commits. An
    /// intent equal to accepted state is consumed without producing a DRM
    /// request, so idle scheduling stays quiescent after coalesced motion.
    pub(crate) fn cursor_plane_updates_for_outputs(
        &mut self,
        output_indices: &[usize],
    ) -> Vec<CursorPlaneUpdate> {
        self.cursor_plane_updates_for_outputs_with_policy(output_indices, false)
    }

    /// Prepare updates for producer-owned primary successors. A pending
    /// replacement waiting for an older event may contribute position only
    /// from the accepted binding; its staged lease stays in `intent`.
    pub(crate) fn cursor_plane_updates_for_primary_outputs(
        &mut self,
        output_indices: &[usize],
    ) -> Vec<CursorPlaneUpdate> {
        self.cursor_plane_updates_for_outputs_with_policy(output_indices, true)
    }

    fn cursor_plane_updates_for_outputs_with_policy(
        &mut self,
        output_indices: &[usize],
        primary_successor: bool,
    ) -> Vec<CursorPlaneUpdate> {
        let mut updates = Vec::new();
        let mut noops = Vec::new();
        for output_idx in output_indices {
            if let Some(update) =
                self.prepare_cursor_update_for_output(*output_idx, primary_successor)
            {
                if update.prepared.updates.is_empty() {
                    noops.push((update.device_key, update.crtc));
                } else {
                    updates.push(update);
                }
            }
        }
        for (device_key, crtc) in noops {
            if let Some(state) = self
                .devices
                .iter_mut()
                .find(|device| device.key == device_key)
                .and_then(|device| device.cursor.crtc_states.get_mut(&crtc))
                && state.intent
                    == Some(state.applied.map_or(CursorIntent::Hide, CursorIntent::Show))
            {
                state.intent = None;
                state.intent_motion_timestamp = None;
            }
        }
        updates
    }

    /// Read-only preparation used by TEST_ONLY direct probes. It cannot
    /// reserve leases, publish accepted state, or create event obligations.
    pub(crate) fn preview_cursor_plane_updates_for_outputs(
        &self,
        output_indices: &[usize],
    ) -> Vec<CursorPlaneUpdate> {
        output_indices
            .iter()
            .filter_map(|output_idx| self.prepare_cursor_update_for_output(*output_idx, true))
            .filter(|update| !update.prepared.updates.is_empty())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn cursor_plane_atomic_positions_for_outputs(
        &self,
        output_indices: &[usize],
    ) -> Vec<crate::drm::atomic::CursorPlanePosition> {
        output_indices
            .iter()
            .filter_map(|output_idx| {
                let layout = self.outputs.get(*output_idx)?;
                let device = self.device_for_key(layout.key.device_key)?;
                let crtc = layout.output.crtc;
                let state = device.cursor.crtc_states.get(&crtc)?;
                if device.cursor.permanently_disabled || state.applied.is_none() {
                    return None;
                }
                let binding = match state.intent {
                    Some(CursorIntent::Show(binding)) => binding,
                    Some(CursorIntent::Hide) | None => state.applied?,
                };
                let (plane, crtc_x, crtc_y) = device
                    .cursor
                    .plane
                    .as_ref()?
                    .atomic_position_properties(crtc)?;
                Some(crate::drm::atomic::CursorPlanePosition {
                    device_key: layout.key.device_key,
                    crtc,
                    plane,
                    crtc_x,
                    crtc_y,
                    x: binding.x,
                    y: binding.y,
                    motion_timestamp: state.intent_motion_timestamp,
                })
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn has_eligible_atomic_cursor_move(&self) -> bool {
        self.has_eligible_atomic_cursor_intent()
    }

    /// Whether a full-state cursor intent can make the scheduling pass runnable.
    pub(crate) fn has_eligible_atomic_cursor_intent(&self) -> bool {
        self.devices.iter().any(|device| {
            device.cursor.crtc_states.iter().any(|(crtc, state)| {
                state.intent.is_some()
                    && !device.cursor.cursor_retry_waits_for_flip.contains(crtc)
                    && self
                        .outputs
                        .iter()
                        .position(|layout| {
                            layout.key.device_key == device.key && layout.output.crtc == *crtc
                        })
                        .and_then(|output_idx| {
                            self.prepare_cursor_update_for_output(output_idx, false)
                        })
                        .is_some_and(|update| !update.prepared.updates.is_empty())
            })
        })
    }

    /// Whether a position-only pointer-motion intent can be accepted as a
    /// cursor-only commit independently of scene work on another output.
    pub(crate) fn has_eligible_atomic_cursor_position_intent(&self) -> bool {
        self.devices.iter().any(|device| {
            device.cursor.crtc_states.iter().any(|(crtc, state)| {
                let position_only = match state.intent {
                    Some(CursorIntent::Show(next)) => state.applied.is_some_and(|applied| {
                        let mut moved = applied;
                        moved.x = next.x;
                        moved.y = next.y;
                        moved == next && (applied.x != next.x || applied.y != next.y)
                    }),
                    Some(CursorIntent::Hide) | None => false,
                };
                position_only
                    && state.intent_motion_timestamp.is_some()
                    && !device.cursor.cursor_retry_waits_for_flip.contains(crtc)
                    && self
                        .outputs
                        .iter()
                        .position(|layout| {
                            layout.key.device_key == device.key && layout.output.crtc == *crtc
                        })
                        .and_then(|output_idx| {
                            self.prepare_cursor_update_for_output(output_idx, false)
                        })
                        .is_some_and(|update| !update.prepared.updates.is_empty())
            })
        })
    }

    /// Only an accepted cursor-only commit owes a flip event. An EBUSY wait owes
    /// none of ours: the old-event drain fail-stops on a missing event, so it must
    /// not wait for one; the wait mark is cleared after the drain instead.
    pub(crate) fn cursor_atomic_event_pending_for_output(&self, output_idx: usize) -> bool {
        let Some(layout) = self.outputs.get(output_idx) else {
            return false;
        };
        self.device_for_key(layout.key.device_key)
            .is_some_and(|device| {
                device
                    .cursor
                    .atomic_cursor_commits_in_flight
                    .contains_key(&layout.output.crtc)
            })
    }

    pub(crate) fn cursor_retry_waits_for_flip(&self, output_idx: usize) -> bool {
        let Some(layout) = self.outputs.get(output_idx) else {
            return false;
        };
        self.device_for_key(layout.key.device_key)
            .is_some_and(|device| {
                device
                    .cursor
                    .cursor_retry_waits_for_flip
                    .contains(&layout.output.crtc)
            })
    }

    /// Whether the scene's output gate must wait for an accepted cursor-only
    /// commit on this output.
    pub(crate) fn cursor_commit_in_flight_for_output(&self, output_idx: usize) -> bool {
        let Some(layout) = self.outputs.get(output_idx) else {
            return false;
        };
        self.device_for_key(layout.key.device_key)
            .is_some_and(|device| {
                device
                    .cursor
                    .atomic_cursor_commits_in_flight
                    .contains_key(&layout.output.crtc)
            })
    }

    /// Whether any CRTC in a grouped transaction is still owned by a
    /// cursor-only commit. Direct scanout and composed unflip submit one
    /// atomic request for every listed output, so all of them must be clear.
    pub(crate) fn cursor_commit_in_flight_for_outputs(&self, output_indices: &[usize]) -> bool {
        output_indices
            .iter()
            .any(|output_idx| self.cursor_commit_in_flight_for_output(*output_idx))
    }

    /// Whether every output has settled its newest cursor intent into the
    /// accepted per-CRTC state. An in-flight accepted commit already counts;
    /// an intent waiting behind EBUSY does not.
    pub(crate) fn cursor_intents_settled_for_outputs(&self, output_indices: &[usize]) -> bool {
        output_indices.iter().all(|output_idx| {
            let Some(layout) = self.outputs.get(*output_idx) else {
                return false;
            };
            self.device_for_key(layout.key.device_key)
                .and_then(|device| {
                    device
                        .cursor
                        .crtc_states
                        .get(&layout.output.crtc)
                        .map(|state| (device, state))
                })
                .is_some_and(|(device, state)| {
                    state.intent.is_none()
                        && !device
                            .cursor
                            .cursor_retry_waits_for_flip
                            .contains(&layout.output.crtc)
                })
        })
    }

    pub(crate) fn any_cursor_commit_in_flight(&self) -> bool {
        self.devices
            .iter()
            .any(|device| !device.cursor.atomic_cursor_commits_in_flight.is_empty())
    }

    /// Submit the newest full-state cursor intent for one output. The caller
    /// runs after direct and scene producers have had their turn. Only an
    /// accepted delta creates an event obligation.
    pub(crate) fn submit_atomic_cursor_intent_for_output(
        &mut self,
        output_idx: usize,
    ) -> io::Result<CursorMoveOutcome> {
        use ::drm::control::AtomicCommitFlags;

        let Some(layout) = self.outputs.get(output_idx) else {
            return Ok(CursorMoveOutcome::default());
        };
        if !self.cursor_output_is_active(output_idx) {
            return Ok(CursorMoveOutcome::default());
        }
        let device_key = layout.key.device_key;
        let crtc = layout.output.crtc;
        let Some(device_idx) = self
            .devices
            .iter()
            .position(|device| device.key == device_key)
        else {
            return Ok(CursorMoveOutcome::default());
        };
        if self.devices[device_idx]
            .cursor
            .atomic_cursor_commits_in_flight
            .contains_key(&crtc)
            || self.devices[device_idx]
                .cursor
                .cursor_retry_waits_for_flip
                .contains(&crtc)
        {
            self.devices[device_idx]
                .cursor
                .cursor_retry_waits_for_flip
                .insert(crtc);
            return Ok(CursorMoveOutcome {
                ebusy_count: 1,
                ..CursorMoveOutcome::default()
            });
        }
        let updates = self.cursor_plane_updates_for_outputs(&[output_idx]);
        if updates.is_empty() {
            return Ok(CursorMoveOutcome::default());
        }
        self.reserve_cursor_plane_updates(&updates)?;
        let device = Rc::clone(&self.devices[device_idx].device);
        let sink = Rc::clone(&self.devices[device_idx].atomic_commit_sink);
        let event = crate::drm::atomic::AtomicCommitEvent {
            crtc,
            motion_timestamp: updates
                .iter()
                .find(|update| update.crtc == crtc)
                .and_then(|update| update.motion_timestamp),
        };
        let request = super::cursor_commit::request_for_cursor_updates(&updates);
        if let Err(error) = sink.submit(
            &device,
            AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            request,
            &[event],
        ) {
            let _cursor_rejected = sink.take_last_cursor_rejection();
            if error.raw_os_error() == Some(libc::EBUSY) {
                self.reject_cursor_plane_update_reservations(&updates);
                self.devices[device_idx]
                    .cursor
                    .cursor_retry_waits_for_flip
                    .insert(crtc);
                return Ok(CursorMoveOutcome {
                    ebusy_count: 1,
                    ..CursorMoveOutcome::default()
                });
            }
            let _ = self.reject_atomic_cursor_only_commit(&updates, &error);
            return Err(error);
        }
        self.accept_cursor_plane_updates(&updates)?;
        Ok(CursorMoveOutcome::default())
    }

    pub(crate) fn reserve_cursor_plane_updates(
        &mut self,
        updates: &[CursorPlaneUpdate],
    ) -> io::Result<()> {
        let mut reserved = Vec::new();
        for update in updates {
            let Some(next) = update.prepared.next.filter(|next| {
                update
                    .prepared
                    .prior
                    .is_none_or(|prior| prior.lease != next.lease)
            }) else {
                continue;
            };
            let Some(device) = self
                .devices
                .iter_mut()
                .find(|device| device.key == update.device_key)
            else {
                self.reject_cursor_plane_update_reservations(&reserved);
                return Err(io::Error::other("cursor update device disappeared"));
            };
            if !device
                .cursor
                .plane
                .as_mut()
                .is_some_and(|plane| plane.reserve_sprite(next.lease))
            {
                self.reject_cursor_plane_update_reservations(&reserved);
                return Err(io::Error::other(
                    "cursor sprite lease became stale before submit",
                ));
            }
            reserved.push(update.clone());
        }
        Ok(())
    }

    pub(crate) fn reject_cursor_plane_update_reservations(
        &mut self,
        updates: &[CursorPlaneUpdate],
    ) {
        for update in updates {
            let Some(next) = update.prepared.next.filter(|next| {
                update
                    .prepared
                    .prior
                    .is_none_or(|prior| prior.lease != next.lease)
            }) else {
                continue;
            };
            if let Some(device) = self
                .devices
                .iter_mut()
                .find(|device| device.key == update.device_key)
                && let Some(plane) = device.cursor.plane.as_mut()
            {
                plane.reject_reserved_sprite(next.lease);
            }
        }
    }

    pub(crate) fn accept_cursor_plane_updates(
        &mut self,
        updates: &[CursorPlaneUpdate],
    ) -> io::Result<()> {
        self.accept_cursor_plane_updates_with_validation(updates, true)
    }

    fn accept_cursor_plane_updates_with_validation(
        &mut self,
        updates: &[CursorPlaneUpdate],
        validate_reservations: bool,
    ) -> io::Result<()> {
        // Validate the complete transaction before transferring any lease.
        // A successful ioctl has already changed kernel state, so partial
        // acceptance would leave userspace unable to describe ownership.
        if validate_reservations {
            for update in updates {
                let Some(device) = self
                    .devices
                    .iter()
                    .find(|device| device.key == update.device_key)
                else {
                    return Err(io::Error::other(
                        "accepted cursor update device disappeared",
                    ));
                };
                if let Some(next) = update.prepared.next.filter(|next| {
                    update
                        .prepared
                        .prior
                        .is_none_or(|prior| prior.lease != next.lease)
                }) && !device
                    .cursor
                    .plane
                    .as_ref()
                    .is_some_and(|plane| plane.can_accept_reserved_sprite(next.lease))
                {
                    return Err(io::Error::other(
                        "accepted cursor commit lost its reserved framebuffer lease",
                    ));
                }
            }
        }
        for update in updates {
            let Some(device) = self
                .devices
                .iter_mut()
                .find(|device| device.key == update.device_key)
            else {
                return Err(io::Error::other(
                    "accepted cursor update device disappeared",
                ));
            };
            if let Some(next) = update.prepared.next.filter(|next| {
                update
                    .prepared
                    .prior
                    .is_none_or(|prior| prior.lease != next.lease)
            }) {
                let accepted = device
                    .cursor
                    .plane
                    .as_mut()
                    .is_some_and(|plane| plane.accept_reserved_sprite(next.lease));
                if !accepted {
                    return Err(io::Error::other(
                        "accepted cursor commit lost its reserved framebuffer lease",
                    ));
                }
            }
            let state = device.cursor.crtc_states.entry(update.crtc).or_default();
            state.applied = update.prepared.next;
            state.valid = true;
            if update.prepared.next.is_some() {
                state.resume_visible = false;
            }
            let desired = update
                .prepared
                .next
                .map_or(CursorIntent::Hide, CursorIntent::Show);
            if state.intent == Some(desired) {
                state.intent = None;
                state.intent_motion_timestamp = None;
            }
            state.in_flight = Some(update.prepared.clone());
            device
                .cursor
                .atomic_cursor_commits_in_flight
                .insert(update.crtc, ());
            device
                .cursor
                .cursor_retry_waits_for_flip
                .remove(&update.crtc);
            device.cursor.note_cursor_success(update.crtc);
        }
        Ok(())
    }

    /// Attribute a flip event to this CRTC's cursor-only commit, if present.
    /// The caller still records general scene-commit latency independently.
    pub(crate) fn retire_atomic_cursor_commit_for_output(
        &mut self,
        output_idx: usize,
        retry_wait_ready: bool,
    ) -> bool {
        let Some(layout) = self.outputs.get(output_idx) else {
            return false;
        };
        let device_key = layout.key.device_key;
        let crtc = layout.output.crtc;
        let Some(device) = self
            .devices
            .iter_mut()
            .find(|device| device.key == device_key)
        else {
            return false;
        };
        let retired = device
            .cursor
            .atomic_cursor_commits_in_flight
            .remove(&crtc)
            .is_some();
        if let Some(state) = device.cursor.crtc_states.get_mut(&crtc)
            && let Some(commit) = state.in_flight.take()
            && let Some(prior) = commit.prior
            && commit.next.is_none_or(|next| next.lease != prior.lease)
            && let Some(plane) = device.cursor.plane.as_mut()
        {
            plane.release_sprite_user(prior.lease);
        }
        let retry_reenabled =
            retry_wait_ready && device.cursor.cursor_retry_waits_for_flip.remove(&crtc);
        retired || retry_reenabled
    }

    /// Apply master's rollback rule to every cursor CRTC in a rejected scene
    /// transaction. A successful legacy hide enables transient software
    /// fallback; a failed hide retains hardware ownership and arms motion-led
    /// recovery. In both cases the rejected position is discarded.
    pub(crate) fn reject_atomic_cursor_commit(
        &mut self,
        updates: &[CursorPlaneUpdate],
        error: &io::Error,
    ) -> CursorMoveOutcome {
        self.reject_atomic_cursor_commit_inner(updates, error, false)
    }

    pub(crate) fn reject_atomic_cursor_only_commit(
        &mut self,
        updates: &[CursorPlaneUpdate],
        error: &io::Error,
    ) -> CursorMoveOutcome {
        self.reject_atomic_cursor_commit_inner(updates, error, true)
    }

    fn reject_atomic_cursor_commit_inner(
        &mut self,
        updates: &[CursorPlaneUpdate],
        error: &io::Error,
        fail_closed: bool,
    ) -> CursorMoveOutcome {
        self.reject_cursor_plane_update_reservations(updates);
        let mut outcome = CursorMoveOutcome::default();
        for update in updates {
            let Some(device) = self
                .devices
                .iter_mut()
                .find(|device| device.key == update.device_key)
            else {
                continue;
            };
            let crtc = update.crtc;
            device.cursor.cursor_retry_waits_for_flip.remove(&crtc);
            let disposition = device.cursor.note_cursor_failure_pair(crtc, error, None);
            if fail_closed && disposition == CursorFailureDisposition::Unchanged {
                device.cursor.note_transient_fallback(crtc);
            }
            if let Some(state) = device.cursor.crtc_states.get_mut(&crtc) {
                let rejected_intent = update
                    .prepared
                    .next
                    .map_or(CursorIntent::Hide, CursorIntent::Show);
                if state.intent == Some(rejected_intent) {
                    state.intent = None;
                    state.intent_motion_timestamp = None;
                }
            }
            outcome.retry_required = true;
            self.scene_cursor_hide_failed = true;
            log::warn!(
                "render cursor atomic commit: device {} CRTC {crtc:?} rejected cursor fields ({error}); retaining the last accepted binding",
                device.key
            );
        }
        outcome
    }

    pub(crate) fn defer_cursor_move_outcome(&mut self, outcome: CursorMoveOutcome) {
        self.deferred_cursor_move_outcome.merge(outcome);
    }

    pub(crate) fn take_deferred_cursor_move_outcome(&mut self) -> CursorMoveOutcome {
        std::mem::take(&mut self.deferred_cursor_move_outcome)
    }

    /// Drop old event obligations only after the event drain proves that no
    /// cursor-only flip can be attributed to the next topology epoch. Keep
    /// the accepted detached comparison valid; the following modeset or
    /// resume boundary invalidates it before cursor reconciliation.
    pub(crate) fn clear_atomic_cursor_state_after_old_events_discard(&mut self) {
        for device in &mut self.devices {
            let pending: Vec<_> = device
                .cursor
                .crtc_states
                .iter_mut()
                .filter_map(|(crtc, state)| state.in_flight.take().map(|commit| (*crtc, commit)))
                .collect();
            if let Some(plane) = device.cursor.plane.as_mut() {
                for (_, commit) in pending {
                    if let Some(prior) = commit.prior
                        && commit.next.is_none_or(|next| next.lease != prior.lease)
                    {
                        plane.release_sprite_user(prior.lease);
                    }
                }
                for state in device.cursor.crtc_states.values_mut() {
                    if let Some(binding) = state.applied.take() {
                        plane.release_sprite_user(binding.lease);
                    }
                    if let Some(binding) = state.lifecycle_pending.take() {
                        plane.release_sprite_user(binding.lease);
                    }
                    state.intent = None;
                }
                plane.recover_sprite_pool_after_all_off();
            }
            device.cursor.atomic_cursor_commits_in_flight.clear();
            device.cursor.cursor_retry_waits_for_flip.clear();
            device.cursor.cursor_hide_recovery.clear();
            device.cursor.transient_fallback_crtcs.clear();
        }
    }

    /// Settle the same prepared cursor leases regardless of which producer
    /// carried them in the accepted primary-plane transaction.
    pub(crate) fn cursor_plane_scene_commit_submitted(
        &mut self,
        updates: &[CursorPlaneUpdate],
    ) -> io::Result<()> {
        if updates
            .iter()
            .any(|update| update.owner != super::cursor_frames::CursorCommitOwner::HardwareOnly)
        {
            return Err(io::Error::other(
                "non-hardware cursor update submitted by a direct primary commit",
            ));
        }
        self.accept_cursor_plane_updates(updates)?;
        for update in updates {
            let Some(state) = self
                .devices
                .iter_mut()
                .find(|device| device.key == update.device_key)
                .and_then(|device| device.cursor.crtc_states.get_mut(&update.crtc))
            else {
                continue;
            };
            let accepted_intent = update
                .prepared
                .next
                .map_or(CursorIntent::Hide, CursorIntent::Show);
            if state.intent == Some(accepted_intent) {
                state.intent = None;
                state.intent_motion_timestamp = None;
            }
        }
        Ok(())
    }

    /// Requeue a failed scene-carried cursor delta only when it was an update
    /// to an already accepted hardware binding. Ownership transitions remain
    /// with their failed frame and cannot be stolen by the cursor-only path.
    pub(crate) fn restore_failed_hardware_cursor_intents(&mut self, updates: &[CursorPlaneUpdate]) {
        for update in updates {
            if update.owner == super::cursor_frames::CursorCommitOwner::HardwareOnly {
                continue;
            }
            let (Some(prior), Some(next)) = (update.prepared.prior, update.prepared.next) else {
                continue;
            };
            let Some(state) = self
                .devices
                .iter_mut()
                .find(|device| device.key == update.device_key)
                .and_then(|device| device.cursor.crtc_states.get_mut(&update.crtc))
            else {
                continue;
            };
            if state.applied == Some(prior) && state.intent.is_none() {
                state.intent = Some(CursorIntent::Show(next));
                state.intent_motion_timestamp = update.motion_timestamp;
            }
        }
    }

    /// Settle the scene frame's cursor deltas and discard older cursor-only
    /// intent for that CRTC. The frame plan is authoritative for this submit;
    /// it is never inserted into the intent consumed by idle cursor-only work.
    pub(crate) fn cursor_plane_scene_frame_submitted(
        &mut self,
        output_idx: usize,
        frame: crate::kms::render::scene::CursorFrameId,
        updates: &[CursorPlaneUpdate],
    ) -> io::Result<()> {
        if updates
            .iter()
            .any(|update| update.owner != super::cursor_frames::CursorCommitOwner::Scene(frame))
        {
            return Err(io::Error::other(
                "hardware-only cursor intent submitted as a scene frame update",
            ));
        }
        if let Err(error) = self.accept_cursor_plane_updates(updates) {
            // The primary+cursor ioctl has already succeeded. Preserve its
            // actual binding by consuming the reservation without repeating
            // the preflight check that just failed; dropping it would let a
            // later upload overwrite a framebuffer KMS is scanning out.
            log::error!(
                "accepted shared scene cursor settlement preflight failed ({error}); recovering the accepted lease"
            );
            self.accept_cursor_plane_updates_with_validation(updates, false)?;
        }
        let Some((device_key, crtc)) = self
            .outputs
            .get(output_idx)
            .map(|layout| (layout.key.device_key, layout.output.crtc))
        else {
            return Ok(());
        };
        let accepted_sprite = updates
            .iter()
            .find(|update| update.device_key == device_key && update.crtc == crtc)
            .and_then(|update| update.prepared.next);
        if let Some(device) = self
            .devices
            .iter_mut()
            .find(|device| device.key == device_key)
        {
            if let Some(binding) = accepted_sprite {
                for (other_crtc, state) in &mut device.cursor.crtc_states {
                    if *other_crtc == crtc
                        || binding.lease.version()
                            < match state.intent {
                                Some(CursorIntent::Show(pending)) => pending.lease.version(),
                                Some(CursorIntent::Hide) | None => continue,
                            }
                    {
                        continue;
                    }
                    let Some(CursorIntent::Show(pending)) = state.intent.as_mut() else {
                        continue;
                    };
                    // The accepted frame makes this sprite the newest
                    // hardware image. Retarget already queued moves on the
                    // other CRTCs without changing their output-local pointer
                    // position or their own plane properties.
                    pending.x += i32::from(pending.hot_x) - i32::from(binding.hot_x);
                    pending.y += i32::from(pending.hot_y) - i32::from(binding.hot_y);
                    pending.lease = binding.lease;
                    pending.framebuffer = binding.framebuffer;
                    pending.hot_x = binding.hot_x;
                    pending.hot_y = binding.hot_y;
                }
            }
            if let Some(state) = device.cursor.crtc_states.get_mut(&crtc) {
                state.intent = None;
                state.intent_motion_timestamp = None;
            }
        }
        Ok(())
    }

    /// Construct a cursor position record for legacy tests without issuing a
    /// DRM ioctl. Production commit producers consume full-state intents.
    #[cfg(test)]
    pub(crate) fn for_tests_seed_atomic_cursor_position(
        &mut self,
        output_idx: usize,
        plane_handle: ::drm::control::plane::Handle,
        crtc_x: ::drm::control::property::Handle,
        crtc_y: ::drm::control::property::Handle,
        root_x: i32,
        root_y: i32,
        hot_x: u16,
        hot_y: u16,
    ) -> crate::drm::atomic::CursorPlanePosition {
        let layout = &self.outputs[output_idx];
        let device_key = layout.key.device_key;
        let crtc = layout.output.crtc;
        let (crtc_x_local, crtc_y_local) =
            cursor_root_to_crtc_local(root_x, root_y, layout.x, layout.y, hot_x, hot_y);
        let device = self
            .devices
            .iter_mut()
            .find(|device| device.key == device_key)
            .expect("test output has an owning KMS device");
        if device.cursor.plane.is_none() {
            device.cursor.plane = Some(crate::kms::cursor_plane::CursorPlane::for_tests_stub(
                Rc::clone(&device.device),
                64,
                64,
            ));
        }
        let plane = device
            .cursor
            .plane
            .as_mut()
            .expect("test cursor plane was installed");
        plane.for_tests_set_atomic_position_properties(crtc, plane_handle, crtc_x, crtc_y);
        plane.for_tests_set_visible(crtc, true);
        let (plane_handle, crtc_x, crtc_y) = device
            .cursor
            .plane
            .as_ref()
            .and_then(|plane| plane.atomic_position_properties(crtc))
            .expect("test cursor plane exposes atomic position properties");
        let motion_timestamp = Instant::now();
        device
            .cursor
            .crtc_states
            .entry(crtc)
            .or_default()
            .intent_motion_timestamp = Some(motion_timestamp);
        crate::drm::atomic::CursorPlanePosition {
            device_key,
            crtc,
            plane: plane_handle,
            crtc_x,
            crtc_y,
            x: crtc_x_local,
            y: crtc_y_local,
            motion_timestamp: Some(motion_timestamp),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests_atomic_position_is_unsent(
        &self,
        device_key: crate::platform::drm::DrmDeviceKey,
        crtc: ::drm::control::crtc::Handle,
    ) -> bool {
        self.devices
            .iter()
            .find(|device| device.key == device_key)
            .and_then(|device| device.cursor.crtc_states.get(&crtc))
            .is_some_and(|state| state.intent.is_some())
    }

    #[cfg(test)]
    pub(crate) fn for_tests_atomic_cursor_commit_is_in_flight(
        &self,
        device_key: crate::platform::drm::DrmDeviceKey,
        crtc: ::drm::control::crtc::Handle,
    ) -> bool {
        self.devices
            .iter()
            .find(|device| device.key == device_key)
            .is_some_and(|device| {
                device
                    .cursor
                    .atomic_cursor_commits_in_flight
                    .contains_key(&crtc)
            })
    }

    #[cfg(test)]
    pub(crate) fn for_tests_cursor_show_intent(
        &self,
        output_idx: usize,
    ) -> Option<(
        crate::kms::cursor_plane::sprites::SpriteBufferLease,
        u32,
        i32,
        i32,
    )> {
        let (device_idx, crtc, _, _) = self.cursor_output_route(output_idx).ok()?;
        match self.devices[device_idx]
            .cursor
            .crtc_states
            .get(&crtc)?
            .intent?
        {
            CursorIntent::Show(binding) => {
                Some((binding.lease, binding.framebuffer, binding.x, binding.y))
            }
            CursorIntent::Hide => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests_cursor_retry_wait_pending(&self, output_idx: usize) -> bool {
        let Some(layout) = self.outputs.get(output_idx) else {
            return false;
        };
        self.device_for_key(layout.key.device_key)
            .is_some_and(|device| {
                device
                    .cursor
                    .cursor_retry_waits_for_flip
                    .contains(&layout.output.crtc)
            })
    }

    #[cfg(test)]
    pub(crate) fn for_tests_cursor_hide_intent_pending(&self, output_idx: usize) -> bool {
        let Ok((device_idx, crtc, _, _)) = self.cursor_output_route(output_idx) else {
            return false;
        };
        self.devices[device_idx]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_some_and(|state| matches!(state.intent, Some(CursorIntent::Hide)))
    }

    #[cfg(test)]
    pub(crate) fn for_tests_atomic_cursor_pending_position(
        &self,
        device_key: crate::platform::drm::DrmDeviceKey,
        crtc: ::drm::control::crtc::Handle,
    ) -> Option<crate::drm::atomic::CursorPlanePosition> {
        let device = self
            .devices
            .iter()
            .find(|device| device.key == device_key)?;
        let binding = match device.cursor.crtc_states.get(&crtc)?.intent? {
            CursorIntent::Show(binding) => binding,
            CursorIntent::Hide => return None,
        };
        let (plane, crtc_x, crtc_y) = device
            .cursor
            .plane
            .as_ref()?
            .atomic_position_properties(crtc)?;
        Some(crate::drm::atomic::CursorPlanePosition {
            device_key,
            crtc,
            plane,
            crtc_x,
            crtc_y,
            x: binding.x,
            y: binding.y,
            motion_timestamp: device
                .cursor
                .crtc_states
                .get(&crtc)?
                .intent_motion_timestamp,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_tests_cursor_hide_recovery_attempts(
        &self,
        device_key: crate::platform::drm::DrmDeviceKey,
        crtc: ::drm::control::crtc::Handle,
    ) -> Option<u8> {
        self.devices
            .iter()
            .find(|device| device.key == device_key)
            .and_then(|device| device.cursor.cursor_hide_recovery.get(&crtc))
            .map(|recovery| recovery.attempts)
    }

    #[cfg(test)]
    pub(crate) fn for_tests_cursor_permanently_disabled(
        &self,
        device_key: crate::platform::drm::DrmDeviceKey,
    ) -> bool {
        self.devices
            .iter()
            .find(|device| device.key == device_key)
            .is_some_and(|device| device.cursor.permanently_disabled)
    }

    #[cfg(test)]
    pub(crate) fn for_tests_cursor_fallback_remaining(
        &self,
        device_key: crate::platform::drm::DrmDeviceKey,
        crtc: ::drm::control::crtc::Handle,
    ) -> Option<u8> {
        self.devices
            .iter()
            .find(|device| device.key == device_key)
            .and_then(|device| device.cursor.transient_fallback_crtcs.get(&crtc))
            .map(|fallback| fallback.remaining_sw_retires)
    }

    #[must_use]
    pub(crate) fn cursor_plane_fits_for_output(
        &self,
        output_idx: usize,
        width: u32,
        height: u32,
    ) -> bool {
        let Some(layout) = self.outputs.get(output_idx) else {
            return false;
        };
        let Some(device) = self.device_for_key(layout.key.device_key) else {
            return false;
        };
        device.cursor.available_on(layout.output.crtc)
            && device.cursor.plane.as_ref().is_some_and(|plane| {
                cursor_dimensions_fit(plane.width(), plane.height(), width, height)
            })
    }

    pub(super) fn cursor_output_route(
        &self,
        output_idx: usize,
    ) -> io::Result<(usize, ::drm::control::crtc::Handle, i32, i32)> {
        let layout = self
            .outputs
            .get(output_idx)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such output"))?;
        let device_idx = self
            .devices
            .iter()
            .position(|device| device.key == layout.key.device_key)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "no DRM device {} for output {output_idx}",
                        layout.key.device_key
                    ),
                )
            })?;
        Ok((device_idx, layout.output.crtc, layout.x, layout.y))
    }

    pub(super) fn note_unbound_cursor_failure(
        &mut self,
        device_idx: usize,
        crtc: ::drm::control::crtc::Handle,
        error: &io::Error,
    ) -> bool {
        let device_key = self.devices[device_idx].key;
        let was_permanently_disabled = self.devices[device_idx].cursor.permanently_disabled;
        let disposition = self.devices[device_idx]
            .cursor
            .note_cursor_failure_pair(crtc, error, None);
        match disposition {
            CursorFailureDisposition::Permanent => {
                if !was_permanently_disabled {
                    log::warn!(
                        "render cursor: device {device_key} permanently rejected cursor ioctls ({error}); using software cursor on that device"
                    );
                }
                true
            }
            CursorFailureDisposition::Transient => {
                log::warn!(
                    "render cursor: device {device_key} CRTC {crtc:?} rejected a temporary cursor state ({error}); rate-limited software fallback"
                );
                true
            }
            CursorFailureDisposition::Unchanged => false,
        }
    }

    /// Diagnostic/test hook: whether one particular device is permanently
    /// latched to software cursor composition.
    #[must_use]
    pub(super) fn hw_cursor_disabled_for_device(
        &self,
        key: crate::platform::drm::DrmDeviceKey,
    ) -> bool {
        self.device_for_key(key)
            .is_some_and(|device| device.cursor.permanently_disabled)
    }

    /// A new sprite or hotspot invalidates EINVAL observations made against a
    /// previous parameter set. This is deliberately a global sprite fanout,
    /// but every device clears only its own CRTC retry records.
    pub(crate) fn cursor_plane_note_sprite_hotspot(
        &mut self,
        width: u16,
        height: u16,
        hot_x: u16,
        hot_y: u16,
    ) {
        let signature = (width, height, hot_x, hot_y);
        for device in &mut self.devices {
            if device.cursor.sprite_signature != Some(signature) {
                device.cursor.sprite_signature = Some(signature);
                device.cursor.transient_fallback_crtcs.clear();
            }
        }
    }

    /// Retain the newest effective cursor observation as per-output atomic
    /// intent. This deliberately does not publish a scene assignment: an
    /// accepted HW binding, a hidden output, or a held direct frame can settle
    /// independently of a primary scene frame.
    pub(crate) fn queue_cursor_intent(
        &mut self,
        record: Option<&crate::kms::render::cursor::CursorRecord>,
        visible: bool,
        root: (i32, i32),
        tearing: bool,
        direct_scanout: bool,
        hardware_cursor_outputs: &[bool],
    ) -> bool {
        self.cursor_pointer_root = root;
        let tearing = tearing || self.tearing_on;
        let visible = visible && !tearing;
        let mut uploaded_devices = HashSet::new();
        let mut ready_devices = HashSet::new();
        let mut deferred_devices = HashSet::new();
        let mut hardware_handles_visible_outputs = true;
        let mut has_visible_footprint = false;
        let intersects_by_output: Vec<_> = self
            .outputs
            .iter()
            .enumerate()
            .map(|(output_idx, layout)| {
                let intersects = record.is_some_and(|record| {
                    visible
                        && cursor_footprint_intersects_output(
                            root.0 - i32::from(record.hot_x) - layout.x,
                            root.1 - i32::from(record.hot_y) - layout.y,
                            i32::from(record.width),
                            i32::from(record.height),
                            i32::from(layout.width),
                            i32::from(layout.height),
                        )
                });
                let actionable = intersects || direct_scanout;
                has_visible_footprint |= actionable;
                if actionable
                    && (!self.cursor_output_is_active(output_idx)
                        || !self.cursor_plane_available_for_output(output_idx)
                        || !hardware_cursor_outputs
                            .get(output_idx)
                            .copied()
                            .unwrap_or(false))
                {
                    hardware_handles_visible_outputs = false;
                }
                intersects
            })
            .collect();

        // Only prepare an image on devices where the visible cursor actually
        // intersects an eligible hardware output. Hidden and off-screen
        // observations keep their newest bytes in the portable record until
        // a later position makes them actionable.
        if let Some(record) = record {
            for (output_idx, intersects) in intersects_by_output.iter().copied().enumerate() {
                let Some(layout) = self.outputs.get(output_idx) else {
                    continue;
                };
                let device_key = layout.key.device_key;
                if (!intersects && !direct_scanout)
                    || !self.cursor_output_is_active(output_idx)
                    || !self.cursor_plane_available_for_output(output_idx)
                    || !hardware_cursor_outputs
                        .get(output_idx)
                        .copied()
                        .unwrap_or(false)
                    || !uploaded_devices.insert(device_key)
                {
                    continue;
                }
                match self.cursor_plane_upload_image_for_output(
                    output_idx,
                    record.version,
                    record.hot_x,
                    record.hot_y,
                    u32::from(record.width),
                    u32::from(record.height),
                    &record.bgra_bytes,
                ) {
                    Ok(CursorUploadOutcome::Ready) => {
                        ready_devices.insert(device_key);
                    }
                    Ok(CursorUploadOutcome::Deferred) => {
                        deferred_devices.insert(device_key);
                        log::debug!(
                            "render cursor: sprite {} waits for a free atomic framebuffer on device {device_key}",
                            record.version
                        );
                    }
                    Err(error) => {
                        log::debug!(
                            "render cursor: atomic sprite {} unavailable on device {device_key}: {error}",
                            record.version
                        );
                        hardware_handles_visible_outputs = false;
                    }
                }
            }
        }

        for (output_idx, intersects) in intersects_by_output.iter().copied().enumerate() {
            let Some((device_key, crtc, layout_x, layout_y)) =
                self.outputs.get(output_idx).map(|layout| {
                    (
                        layout.key.device_key,
                        layout.output.crtc,
                        layout.x,
                        layout.y,
                    )
                })
            else {
                continue;
            };
            if !self.cursor_output_is_active(output_idx) {
                continue;
            }
            let hardware_owner = hardware_cursor_outputs
                .get(output_idx)
                .copied()
                .unwrap_or(false);
            if !self.cursor_plane_available_for_output(output_idx) || !hardware_owner {
                if intersects {
                    hardware_handles_visible_outputs = false;
                }
                // An eligibility fallback blocks new attachments, but it
                // cannot make an already accepted framebuffer disappear.
                // Preserve an explicit hide for the next eligible primary or
                // cursor transaction so the real plane owner is detached.
                if !visible
                    && hardware_owner
                    && let Some(state) = self
                        .devices
                        .iter_mut()
                        .find(|device| device.key == device_key)
                        .and_then(|device| device.cursor.crtc_states.get_mut(&crtc))
                    && (state.applied.is_some()
                        || matches!(state.intent, Some(CursorIntent::Show(_)))
                        || matches!(state.intent, Some(CursorIntent::Hide)))
                {
                    state.intent = Some(CursorIntent::Hide);
                    state.intent_motion_timestamp = None;
                }
                continue;
            }

            let candidate = if visible && (intersects || direct_scanout) {
                self.device_for_key(device_key)
                    .filter(|_| ready_devices.contains(&device_key))
                    .and_then(|device| {
                        device
                            .cursor
                            .plane
                            .as_ref()
                            .and_then(|plane| plane.candidate_sprite())
                    })
            } else {
                None
            };
            if visible && (intersects || direct_scanout) && candidate.is_none() {
                if deferred_devices.contains(&device_key) {
                    let current = self
                        .devices
                        .iter()
                        .find(|device| device.key == device_key)
                        .and_then(|device| device.cursor.crtc_states.get(&crtc))
                        .and_then(|state| match state.intent {
                            Some(CursorIntent::Show(binding)) => Some(binding),
                            Some(CursorIntent::Hide) => None,
                            None => state.applied,
                        });
                    if let Some(mut binding) = current {
                        let (x, y) = cursor_root_to_crtc_local(
                            root.0,
                            root.1,
                            layout_x,
                            layout_y,
                            binding.hot_x,
                            binding.hot_y,
                        );
                        binding.x = x;
                        binding.y = y;
                        if let Some(state) = self
                            .devices
                            .iter_mut()
                            .find(|device| device.key == device_key)
                            .and_then(|device| device.cursor.crtc_states.get_mut(&crtc))
                        {
                            state.intent = Some(CursorIntent::Show(binding));
                            state.intent_motion_timestamp = Some((self.services.monotonic_clock)());
                        }
                    }
                } else {
                    hardware_handles_visible_outputs = false;
                    if let Some(state) = self
                        .devices
                        .iter_mut()
                        .find(|device| device.key == device_key)
                        .and_then(|device| device.cursor.crtc_states.get_mut(&crtc))
                        && matches!(state.intent, Some(CursorIntent::Show(binding)) if state.applied.is_none_or(|applied| applied.lease != binding.lease))
                    {
                        state.intent = None;
                        state.intent_motion_timestamp = None;
                    }
                }
                continue;
            }

            let binding = candidate.and_then(|(lease, framebuffer)| {
                let record = record?;
                let (device_idx, _, _, _) = self.cursor_output_route(output_idx).ok()?;
                let device = self.devices.get(device_idx)?;
                let plane = device.cursor.plane.as_ref()?;
                let properties = plane.properties_for(crtc)?;
                let (x, y) = cursor_root_to_crtc_local(
                    root.0,
                    root.1,
                    layout_x,
                    layout_y,
                    record.hot_x,
                    record.hot_y,
                );
                Some(CursorBinding {
                    properties,
                    lease,
                    framebuffer,
                    x,
                    y,
                    hot_x: record.hot_x,
                    hot_y: record.hot_y,
                })
            });
            if visible && (intersects || direct_scanout) {
                let Some(binding) = binding else {
                    hardware_handles_visible_outputs = false;
                    continue;
                };
                if let Some(state) = self
                    .devices
                    .iter_mut()
                    .find(|device| device.key == device_key)
                    .map(|device| device.cursor.crtc_states.entry(crtc).or_default())
                {
                    if state.applied == Some(binding) {
                        state.intent = None;
                        state.intent_motion_timestamp = None;
                    } else {
                        state.intent = Some(CursorIntent::Show(binding));
                        state.intent_motion_timestamp = Some((self.services.monotonic_clock)());
                    }
                }
            } else if let Some(state) = self
                .devices
                .iter_mut()
                .find(|device| device.key == device_key)
                .map(|device| device.cursor.crtc_states.entry(crtc).or_default())
                && (state.applied.is_some() || matches!(state.intent, Some(CursorIntent::Show(_))))
            {
                state.intent = Some(CursorIntent::Hide);
                state.intent_motion_timestamp = None;
            } else if let Some(state) = self
                .devices
                .iter_mut()
                .find(|device| device.key == device_key)
                .and_then(|device| device.cursor.crtc_states.get_mut(&crtc))
                && state.applied.is_none()
            {
                state.intent = None;
                state.intent_motion_timestamp = None;
            }
        }

        !visible || !has_visible_footprint || hardware_handles_visible_outputs
    }

    pub(crate) fn cursor_plane_upload_image_for_output(
        &mut self,
        output_idx: usize,
        version: u64,
        hot_x: u16,
        hot_y: u16,
        width: u32,
        height: u32,
        bgra_bytes: &[u8],
    ) -> io::Result<yserver_render::kms::render::scene::CursorUploadOutcome> {
        let (device_idx, crtc, _, _) = self.cursor_output_route(output_idx)?;
        let state = &mut self.devices[device_idx].cursor;
        if !state.available_on(crtc) {
            return Err(io::Error::other("cursor plane unavailable for output"));
        }
        let result = state
            .plane
            .as_mut()
            .expect("available cursor state has a plane")
            .upload_image(version, hot_x, hot_y, width, height, bgra_bytes);
        // Only a real upload attempt is classified. The availability
        // precheck above and scene-side version races are not driver
        // observations and must not extend this output's backoff.
        if let Err(error) = &result {
            let failure = error.raw_os_error().map_or_else(
                || io::Error::new(error.kind(), error.to_string()),
                io::Error::from_raw_os_error,
            );
            apply_cursor_operation_result(state, crtc, &Err(failure));
        }
        if matches!(
            result,
            Ok(yserver_render::kms::render::scene::CursorUploadOutcome::Ready)
        ) && let Some((lease, framebuffer)) = state
            .plane
            .as_ref()
            .and_then(|plane| plane.candidate_sprite())
        {
            // Upload acceptance switches the pixels and their hotspot as one
            // identity. Preserve the pointer position while translating from
            // the old hotspot used by this intent to the uploaded hotspot.
            for crtc_state in state.crtc_states.values_mut() {
                if let Some(CursorIntent::Show(binding)) = crtc_state.intent.as_mut() {
                    binding.x += i32::from(binding.hot_x) - i32::from(hot_x);
                    binding.y += i32::from(binding.hot_y) - i32::from(hot_y);
                    binding.lease = lease;
                    binding.framebuffer = framebuffer;
                    binding.hot_x = hot_x;
                    binding.hot_y = hot_y;
                }
            }
        }
        result
    }

    #[must_use]
    pub(crate) fn cursor_plane_uploaded_version_for_output(
        &self,
        output_idx: usize,
    ) -> Option<u64> {
        let (device_idx, _, _, _) = self.cursor_output_route(output_idx).ok()?;
        self.devices[device_idx]
            .cursor
            .plane
            .as_ref()
            .and_then(|plane| plane.uploaded_version())
    }

    pub(crate) fn cursor_plane_uploaded_image_matches_for_output(
        &self,
        output_idx: usize,
        version: u64,
        hot_x: u16,
        hot_y: u16,
    ) -> bool {
        let Ok((device_idx, crtc, _, _)) = self.cursor_output_route(output_idx) else {
            return false;
        };
        let state = &self.devices[device_idx].cursor;
        state.available_on(crtc)
            && state
                .plane
                .as_ref()
                .is_some_and(|plane| plane.uploaded_image_matches(version, hot_x, hot_y))
    }

    /// Submit an atomic binding on `output_idx`'s CRTC at root position `(x, y)`.
    /// The accepted state and its sprite lease remain owned until retirement.
    ///
    /// # Errors
    /// Atomic submission failure or an unavailable output/plane/sprite.
    pub(crate) fn cursor_plane_show_on_crtc(
        &mut self,
        output_idx: usize,
        hot_x: u16,
        hot_y: u16,
        x: i32,
        y: i32,
    ) -> Result<(), crate::kms::cursor_plane::CursorShowError> {
        let (device_idx, crtc, layout_x, layout_y) = self
            .cursor_output_route(output_idx)
            .map_err(crate::kms::cursor_plane::CursorShowError::Unbound)?;
        let (cx, cy) = cursor_root_to_crtc_local(x, y, layout_x, layout_y, hot_x, hot_y);
        if !self.devices[device_idx].cursor.available_on(crtc) {
            return Err(self.cursor_show_failure(
                device_idx,
                crtc,
                io::Error::other("cursor plane unavailable for output"),
                false,
            ));
        }
        let candidate = self.devices[device_idx]
            .cursor
            .plane
            .as_ref()
            .and_then(|plane| plane.candidate_sprite())
            .ok_or_else(|| {
                self.cursor_show_failure(
                    device_idx,
                    crtc,
                    io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "cursor image has no prepared framebuffer lease",
                    ),
                    true,
                )
            })?;
        let properties = self.devices[device_idx]
            .cursor
            .plane
            .as_ref()
            .and_then(|plane| plane.properties_for(crtc))
            .ok_or_else(|| {
                self.cursor_show_failure(
                    device_idx,
                    crtc,
                    io::Error::other("atomic cursor plane properties unavailable"),
                    false,
                )
            })?;
        let binding = CursorBinding {
            properties,
            lease: candidate.0,
            framebuffer: candidate.1,
            x: cx,
            y: cy,
            hot_x,
            hot_y,
        };
        let state = self.devices[device_idx]
            .cursor
            .crtc_states
            .entry(crtc)
            .or_default();
        state.intent = Some(CursorIntent::Show(binding));
        state.intent_motion_timestamp = None;
        let result = self.submit_atomic_cursor_intent_for_output(output_idx);
        match result {
            Ok(outcome) if outcome.ebusy_count == 0 => {
                self.devices[device_idx].cursor.note_cursor_success(crtc);
                Ok(())
            }
            Ok(_) => Err(self.cursor_show_failure(
                device_idx,
                crtc,
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "cursor show waits for the prior CRTC event",
                ),
                true,
            )),
            Err(error) => {
                let deferred = error.kind() == io::ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(libc::EBUSY);
                Err(self.cursor_show_failure(device_idx, crtc, error, deferred))
            }
        }
    }

    fn cursor_show_failure(
        &self,
        device_idx: usize,
        crtc: ::drm::control::crtc::Handle,
        operation_error: io::Error,
        deferred: bool,
    ) -> crate::kms::cursor_plane::CursorShowError {
        let remains_visible = self.devices[device_idx]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_some_and(|state| state.applied.is_some());
        if deferred {
            crate::kms::cursor_plane::CursorShowError::Deferred {
                operation_error,
                remains_visible,
            }
        } else if remains_visible {
            crate::kms::cursor_plane::CursorShowError::StillVisible {
                operation_error,
                rollback_error: None,
            }
        } else {
            crate::kms::cursor_plane::CursorShowError::Unbound(operation_error)
        }
    }

    /// Cursor move per visible CRTC. Hidden CRTCs are skipped — the
    /// kernel naturally clips off-output coords on the visible ones, so no
    /// per-output geometry test is needed beyond the visibility filter.
    /// Atomic-capable devices record the latest full-state intent for the
    /// scheduling pass.
    ///
    /// Returns the legacy move's EBUSY count, if any. Other errors are logged
    /// per-CRTC and not counted.
    ///
    /// # Errors
    /// `Err` only when the plane is unavailable; per-CRTC ioctl
    /// failures are logged + counted (EBUSY) or logged (other).
    pub(crate) fn cursor_plane_move(&mut self, x: i32, y: i32) -> io::Result<CursorMoveOutcome> {
        let motion_timestamp = Instant::now();
        let mut aggregate = CursorMoveOutcome::default();
        let mut found = false;
        for device_idx in 0..self.devices.len() {
            if self.devices[device_idx].cursor.plane.is_none() {
                continue;
            }
            found = true;
            let outcome =
                self.try_cursor_plane_move_for_device(device_idx, x, y, motion_timestamp)?;
            aggregate.merge(outcome);
        }
        for device in &self.devices {
            device.atomic_commit_sink.emit_telemetry_if_due();
        }
        found
            .then_some(aggregate)
            .ok_or_else(|| io::Error::other("cursor plane unavailable"))
    }

    /// Retry the most recent pending cursor move, if any. Called from
    /// the backend's page-flip-complete handler — the just-retired
    /// flip means the primary atomic-commit queue freed up for this
    /// CRTC, so the cursor commit that lost the race a few ms ago has
    /// a fresh window to land. Latest-wins: only the most recent
    /// position is retried, intermediate motions are discarded.
    ///
    /// Returns the EBUSY count from this retry (typically 0 if the
    /// commit landed; >0 means the cursor commit raced another
    /// pending primary commit and stays queued for the next page-flip
    /// retire).
    ///
    /// # Errors
    /// `Err` only when the plane is unavailable.
    pub(crate) fn cursor_plane_drain_pending_move_for_output(
        &mut self,
        output_idx: usize,
    ) -> io::Result<CursorMoveOutcome> {
        let (_, crtc, _, _) = self.cursor_output_route(output_idx)?;
        let (device_idx, _, _, _) = self.cursor_output_route(output_idx)?;
        if !self.devices[device_idx]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_some_and(|state| state.intent.is_some())
        {
            return Ok(CursorMoveOutcome::default());
        }
        match self.submit_atomic_cursor_intent_for_output(output_idx) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                log::warn!("render cursor pending atomic intent rejected: {error}");
                Ok(CursorMoveOutcome {
                    retry_required: true,
                    ..CursorMoveOutcome::default()
                })
            }
        }
    }

    /// Internal helper: per-CRTC `move_to` iteration that returns the
    /// number of CRTCs whose move ioctl returned `EBUSY`. Shared by
    /// `cursor_plane_move` (first-attempt path) and
    /// `cursor_plane_drain_pending_move` (retry path).
    fn try_cursor_plane_move_for_device(
        &mut self,
        device_idx: usize,
        x: i32,
        y: i32,
        motion_timestamp: Instant,
    ) -> io::Result<CursorMoveOutcome> {
        let device_key = self.devices[device_idx].key;
        let layouts: Vec<(::drm::control::crtc::Handle, i32, i32)> = self
            .outputs
            .iter()
            .filter(|layout| layout.key.device_key == device_key)
            .map(|l| (l.output.crtc, l.x, l.y))
            .collect();
        let state = &mut self.devices[device_idx].cursor;
        if state.plane.is_none() {
            return Err(io::Error::other("cursor plane unavailable"));
        }
        if state.permanently_disabled {
            return Ok(CursorMoveOutcome::default());
        }
        for (crtc, layout_x, layout_y) in layouts {
            if state.permanently_disabled {
                break;
            }
            let Some(crtc_state) = state.crtc_states.get(&crtc) else {
                continue;
            };
            // Motion updates the position of the newest show intent. The
            // binding's framebuffer and hotspot form one identity, so use
            // that hotspot until an upload installs the matching replacement.
            // Keep its staged lease; a pending detach also wins over motion.
            let binding = match crtc_state.intent {
                Some(CursorIntent::Show(binding)) => binding,
                Some(CursorIntent::Hide) => continue,
                None => {
                    let Some(applied) = crtc_state.applied else {
                        continue;
                    };
                    applied
                }
            };
            if !state
                .plane
                .as_ref()
                .is_some_and(|plane| plane.properties_for(crtc).is_some())
            {
                continue;
            }
            let (cx, cy) =
                cursor_root_to_crtc_local(x, y, layout_x, layout_y, binding.hot_x, binding.hot_y);
            if let Some(crtc_state) = state.crtc_states.get_mut(&crtc) {
                crtc_state.intent = Some(CursorIntent::Show(CursorBinding {
                    x: cx,
                    y: cy,
                    ..binding
                }));
                crtc_state.intent_motion_timestamp = Some(motion_timestamp);
            }
        }
        state.pending_move = None;
        state.pending_move_timestamp = None;
        Ok(CursorMoveOutcome::default())
    }

    /// Submit a per-CRTC atomic detach for output-local recovery. A rejected
    /// request retains the accepted binding and marks the hide as failed.
    ///
    /// # Errors
    /// `NotFound` if `output_idx` or the atomic cursor plane is unavailable;
    /// the atomic executor's error otherwise.
    pub(crate) fn cursor_plane_hide_on_crtc(&mut self, output_idx: usize) -> io::Result<()> {
        let (device_idx, crtc, _, _) = self.cursor_output_route(output_idx)?;
        if self.devices[device_idx]
            .cursor
            .plane
            .as_ref()
            .is_none_or(|plane| plane.properties_for(crtc).is_none())
        {
            return Err(io::Error::other("atomic cursor plane unavailable"));
        }
        let state = self.devices[device_idx]
            .cursor
            .crtc_states
            .entry(crtc)
            .or_default();
        state.intent = Some(CursorIntent::Hide);
        state.intent_motion_timestamp = None;
        match self.submit_atomic_cursor_intent_for_output(output_idx) {
            Ok(outcome) if outcome.ebusy_count == 0 => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "cursor hide waits for the prior CRTC event",
            )),
            Err(error) => {
                self.scene_cursor_hide_failed = true;
                Err(error)
            }
        }
    }

    pub(super) fn cursor_plane_detach_for_output(
        &self,
        output_idx: usize,
    ) -> Option<crate::drm::atomic::AtomicCursorPlaneDetach> {
        let (device_idx, crtc, _, _) = self.cursor_output_route(output_idx).ok()?;
        self.devices[device_idx]
            .cursor
            .plane
            .as_ref()
            .and_then(|plane| plane.properties_for(crtc))
            .map(Into::into)
            .or_else(|| {
                self.devices[device_idx]
                    .cursor
                    .crtc_states
                    .get(&crtc)
                    .and_then(|state| state.applied)
                    .map(|binding| binding.properties.into())
            })
    }

    pub(super) fn accept_cursor_lifecycle_detach(&mut self, output_idx: usize) {
        let Ok((device_idx, crtc, _, _)) = self.cursor_output_route(output_idx) else {
            return;
        };
        let state = self.devices[device_idx]
            .cursor
            .crtc_states
            .entry(crtc)
            .or_default();
        super::cursor_lifecycle::accept_blocking_detach(state);
        self.devices[device_idx].cursor.clear_atomic_position(crtc);
        self.devices[device_idx]
            .cursor
            .atomic_cursor_commits_in_flight
            .remove(&crtc);
    }

    pub(super) fn quarantine_cursor_output(&mut self, output_idx: usize) {
        let Ok((device_idx, _, _, _)) = self.cursor_output_route(output_idx) else {
            return;
        };
        if let Some(plane) = self.devices[device_idx].cursor.plane.as_mut() {
            plane.disarm_sprite_pool();
        }
    }

    /// Kernel-side visibility for this output's device-qualified CRTC. Scene
    /// bookkeeping can temporarily lag after an ioctl/rollback failure, so a
    /// compose tick must consult this before deciding it is safe to draw SW.
    #[must_use]
    pub(crate) fn cursor_plane_visible_for_output(&self, output_idx: usize) -> bool {
        let Ok((device_idx, crtc, _, _)) = self.cursor_output_route(output_idx) else {
            return false;
        };
        self.devices[device_idx]
            .cursor
            .crtc_states
            .get(&crtc)
            .is_some_and(|state| state.applied.is_some())
    }

    /// Physical cursor-plane state across the current output inventory.
    #[must_use]
    pub(crate) fn any_cursor_plane_visible(&self) -> bool {
        (0..self.outputs.len()).any(|output_idx| self.cursor_plane_visible_for_output(output_idx))
    }

    /// Advance only this output's EINVAL retry budget after its own successful
    /// software-composed retirement. Returns true when another repaint is
    /// needed either to consume more backoff or to perform the now-eligible HW
    /// retry. A retirement on another card cannot touch this record.
    pub(crate) fn cursor_plane_note_composed_retirement(&mut self, output_idx: usize) -> bool {
        let Ok((device_idx, crtc, _, _)) = self.cursor_output_route(output_idx) else {
            return false;
        };
        let Some(retry) = self.devices[device_idx]
            .cursor
            .transient_fallback_crtcs
            .get_mut(&crtc)
        else {
            return false;
        };
        if retry.remaining_sw_retires > 0 {
            retry.remaining_sw_retires -= 1;
            return true;
        }
        false
    }

    /// Revalidate every existing per-device cursor plane after an active CRTC
    /// topology change. A genuinely headless-deferred device is deliberately
    /// skipped here: only the post-success explicit-enable hook may make its
    /// first attempt. A prior transient attempt is retried at this later
    /// topology/resume boundary.
    pub(super) fn refresh_cursor_topology_for_devices(
        &mut self,
        changed_devices: &HashSet<crate::platform::drm::DrmDeviceKey>,
    ) {
        let factory = Rc::clone(&self.services.cursor_plane_factory);
        self.refresh_cursor_topology_for_devices_with(
            changed_devices,
            move |device, crtcs, boundary| {
                initialize_cursor_plane_for_device(device, crtcs, boundary, factory.as_ref());
            },
        );
    }

    /// Probe every opened device at a full display-rescan boundary. This is
    /// intentionally absent from RANDR's lightweight connector reprobe path.
    /// The returned keys identify only cards whose CRTC/plane snapshot changed.
    pub(crate) fn probe_cursor_topology_changes(
        &mut self,
    ) -> io::Result<HashSet<crate::platform::drm::DrmDeviceKey>> {
        let mut crtcs_by_device: HashMap<_, Vec<_>> = HashMap::new();
        for output in &self.outputs {
            crtcs_by_device
                .entry(output.key.device_key)
                .or_default()
                .push(output.output.crtc);
        }
        let mut changed_devices = HashSet::new();
        for device in &mut self.devices {
            let crtcs = crtcs_by_device.remove(&device.key).unwrap_or_default();
            let changed = if let Some(plane) = device.cursor.plane.as_mut() {
                plane.probe_topology(&crtcs)?
            } else {
                let active: HashSet<_> = crtcs.iter().copied().collect();
                active != device.cursor.cursor_topology_crtcs
            };
            if changed {
                changed_devices.insert(device.key);
            }
        }
        Ok(changed_devices)
    }

    pub(super) fn refresh_cursor_topology_for_devices_with<F>(
        &mut self,
        changed_devices: &HashSet<crate::platform::drm::DrmDeviceKey>,
        mut factory: F,
    ) where
        F: FnMut(&mut KmsDevice, &[::drm::control::crtc::Handle], &str),
    {
        let mut crtcs_by_device: HashMap<_, Vec<_>> = HashMap::new();
        for output in &self.outputs {
            crtcs_by_device
                .entry(output.key.device_key)
                .or_default()
                .push(output.output.crtc);
        }
        for device in &mut self.devices {
            if !changed_devices.contains(&device.key) {
                continue;
            }
            let crtcs = crtcs_by_device.remove(&device.key).unwrap_or_default();
            let active_crtcs: HashSet<_> = crtcs.iter().copied().collect();
            let crtc_set_changed = active_crtcs != device.cursor.cursor_topology_crtcs;
            let plane_changed = if let Some(plane) = device.cursor.plane.as_mut() {
                if plane.has_pending_topology() {
                    plane.apply_probed_topology(&crtcs)
                } else {
                    match plane.rediscover_topology(&crtcs) {
                        Ok(changed) => changed,
                        Err(error) => {
                            log::warn!(
                                "render cursor: device {} plane topology rediscovery failed: {error}; retaining last accepted discovery",
                                device.key
                            );
                            false
                        }
                    }
                }
            } else {
                false
            };
            let topology_changed = crtc_set_changed || plane_changed;
            if topology_changed {
                device.cursor.cursor_topology_epoch =
                    device.cursor.cursor_topology_epoch.saturating_add(1);
            }
            device.cursor.pending_move = None;
            device.cursor.pending_move_timestamp = None;
            device.cursor.atomic_cursor_commits_in_flight.clear();
            device.cursor.cursor_retry_waits_for_flip.clear();
            device.cursor.cursor_hide_recovery.clear();
            device.cursor.transient_fallback_crtcs.clear();
            for state in device.cursor.crtc_states.values_mut() {
                if topology_changed {
                    super::cursor_lifecycle::invalidate_applied_comparison(state);
                }
                state.intent = None;
                state.intent_motion_timestamp = None;
            }
            device.cursor.cursor_topology_crtcs = active_crtcs;
            if device.cursor.should_retry_initialization(!crtcs.is_empty()) {
                factory(device, &crtcs, "lifecycle retry");
            }
            if let Some(plane) = device.cursor.plane.as_mut() {
                plane.retain_crtcs(&crtcs.iter().copied().collect());
            }
        }
    }

    /// Run the one-time first-output factory for an opened device that began
    /// genuinely headless. Callers must invoke this only after the successful
    /// ActiveOutput insertion. All post-insertion active CRTCs on the owning
    /// device are passed to the factory, and no other card is touched.
    pub(super) fn initialize_headless_cursor_for_device_with<F>(
        &mut self,
        device_key: crate::platform::drm::DrmDeviceKey,
        boundary: &str,
        factory: F,
    ) -> bool
    where
        F: FnOnce(&mut KmsDevice, &[::drm::control::crtc::Handle], &str),
    {
        let crtcs: Vec<_> = self
            .outputs
            .iter()
            .filter(|output| output.key.device_key == device_key)
            .map(|output| output.output.crtc)
            .collect();
        let Some(device) = self
            .devices
            .iter_mut()
            .find(|device| device.key == device_key)
        else {
            return false;
        };
        if !device
            .cursor
            .should_initialize_headless_deferred(!crtcs.is_empty())
        {
            return false;
        }

        // Consume genuine-deferred before invoking the injectable factory so
        // even a test/factory panic cannot make ordinary probes look like a
        // never-attempted device. The production factory records success or
        // retryable/permanent failure in the remaining state.
        device.cursor.headless_deferred = false;
        factory(device, &crtcs, boundary);
        true
    }

    fn refresh_cursor_topology(&mut self) {
        let all_devices: HashSet<_> = self.devices.iter().map(|device| device.key).collect();
        self.refresh_cursor_topology_for_devices(&all_devices);
    }

    /// Detach the plane on every CRTC the plane has ever been bound
    /// against AND every currently-known output. Global recovery
    /// fallback only — `drain_all`, shutdown, VT-leave, DRM-master
    /// loss. Per Phase D' this also invalidates `uploaded_version`
    /// so the next acquire/modeset re-uploads cleanly.
    ///
    /// # Errors
    /// Per-CRTC failures are logged; this never returns `Err`
    /// unless the plane is unavailable.
    pub(crate) fn cursor_plane_hide_all(&mut self) -> io::Result<()> {
        let mut found = false;
        let mut first_error = None;
        for device in &mut self.devices {
            device.cursor.pending_move = None;
            device.cursor.pending_move_timestamp = None;
            if let Some(plane) = device.cursor.plane.as_mut() {
                plane.invalidate_uploaded_version();
            }
            found |= device.cursor.plane.is_some();
        }
        for output_idx in 0..self.outputs.len() {
            if !self.cursor_plane_visible_for_output(output_idx) {
                continue;
            }
            if let Ok((device_idx, crtc, _, _)) = self.cursor_output_route(output_idx)
                && let Some(state) = self.devices[device_idx].cursor.crtc_states.get_mut(&crtc)
            {
                state.resume_visible = true;
            }
            if let Err(error) = self.cursor_plane_hide_on_crtc(output_idx)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if let Some(error) = first_error {
            Err(error)
        } else if found {
            Ok(())
        } else {
            Err(io::Error::other("cursor plane unavailable"))
        }
    }

    /// Re-arm the hardware cursor plane on every CRTC that was
    /// showing the cursor before the suspend. This restores the
    /// kernel-side cursor binding that the VT switch tore down.
    ///
    /// Called after a connector snapshot has been applied so the output list
    /// is up to date. The cursor position and hotspot come from the
    /// caller (backend passes `core.cursor_x/y` and the effective
    /// cursor's hotspot).
    pub(crate) fn rearm_cursor(&mut self, hot_x: u16, hot_y: u16, x: i32, y: i32) {
        self.refresh_cursor_topology();
        let routes: Vec<_> = self
            .outputs
            .iter()
            .enumerate()
            .filter_map(|(output_idx, layout)| {
                if !self.cursor_output_is_active(output_idx) {
                    return None;
                }
                let device = self.device_for_key(layout.key.device_key)?;
                device
                    .cursor
                    .crtc_states
                    .get(&layout.output.crtc)
                    .is_some_and(|state| state.resume_visible)
                    .then_some((output_idx, device.key, layout.output.crtc))
            })
            .collect();
        log::info!(
            "render resume rearm_cursor: outputs={} initialized_devices={} visible_routes={} hot=({hot_x},{hot_y}) pos=({x},{y})",
            self.outputs.len(),
            self.devices
                .iter()
                .filter(|device| device.cursor.plane.is_some())
                .count(),
            routes.len(),
        );
        let mut shown = 0usize;
        let mut failed = 0usize;
        for (output_idx, device_key, crtc) in routes {
            match self.cursor_plane_show_on_crtc(output_idx, hot_x, hot_y, x, y) {
                Ok(()) => {
                    shown += 1;
                    log::info!(
                        "render resume rearm_cursor: device {device_key} CRTC={crtc:?} show ok"
                    );
                }
                Err(error) => {
                    failed += 1;
                    log::warn!(
                        "render resume rearm_cursor: device {device_key} CRTC={crtc:?} show failed: {error}"
                    );
                }
            }
        }
        log::info!("render resume rearm_cursor: done — shown={shown} failed={failed}");
    }
}

pub(super) fn cursor_root_to_crtc_local(
    x: i32,
    y: i32,
    layout_x: i32,
    layout_y: i32,
    hot_x: u16,
    hot_y: u16,
) -> (i32, i32) {
    (
        x - layout_x - i32::from(hot_x),
        y - layout_y - i32::from(hot_y),
    )
}

/// Whether the cursor footprint `[dx, dx+cw) × [dy, dy+ch)` (in
/// output-local coordinates) overlaps the output's `[0, w) × [0, h)`
/// region. This is the boolean form of `cursor_footprint_rect`'s
/// non-empty condition used by the intent fanout and the scene's
/// output-local cursor assignment.
pub(super) fn cursor_footprint_intersects_output(
    dx: i32,
    dy: i32,
    cw: i32,
    ch: i32,
    w: i32,
    h: i32,
) -> bool {
    dx < w && dx + cw > 0 && dy < h && dy + ch > 0
}
