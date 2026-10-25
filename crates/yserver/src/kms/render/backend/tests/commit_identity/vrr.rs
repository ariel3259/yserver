use crate::{
    drm::{
        atomic::{
            AtomicCommitKind, AtomicCommitSink,
            tests_support::{AtomicCommitRecord, RecordingAtomicCommitExecutor, ScriptedResult},
        },
        modeset::PropMap,
    },
    kms::{
        render::{
            backend::KmsBackend,
            platform::{
                ConnectorSnapshot, KmsDevice, OutputPowerExecutor, PlatformBackend, RenderDeviceId,
            },
        },
        scanout_route::{RenderKmsRelationship, ScanoutRoute},
    },
};
use std::{cell::RefCell, io, rc::Rc};
use yserver_core::backend::{Backend, ModeSpec};

const VRR_PROP_ID: u32 = 90;

type PropertySignature = (String, String, u64);
type CommitSignature = (
    AtomicCommitKind,
    ::drm::control::AtomicCommitFlags,
    Vec<PropertySignature>,
);

fn handle<H: ::drm::control::ResourceHandle>(value: u32) -> H {
    ::drm::control::from_u32(value).expect("nonzero VRR test handle")
}

fn vrr_backend() -> KmsBackend {
    let mut backend = KmsBackend::for_tests();
    let display_key = backend.platform.devices[0].key;
    let render_id = RenderDeviceId::DrmRender(super::super::test_device_key(128));
    backend.platform.render_devices = vec![super::super::randr::test_render_device(
        render_id,
        Some(display_key),
    )];
    backend.platform.selected_render_device = Some(render_id);
    backend.platform.outputs[0].scanout_route =
        ScanoutRoute::new(render_id, display_key, RenderKmsRelationship::Same);
    let recorder = Rc::new(RecordingAtomicCommitExecutor::default());
    backend.platform.devices[0].atomic_commit_sink =
        Rc::new(AtomicCommitSink::new(recorder.clone()));
    let output = &mut backend.platform.outputs[0].output;
    output.crtc_vrr_enabled_prop = Some(handle(VRR_PROP_ID));
    output.connector_vrr_capable = true;
    output.vrr_committed.set(false);
    output.vrr_disabled.set(false);
    output.vrr_clear_retry_used.set(false);
    backend.scene.scene_structure_dirty = false;
    backend
        .initialize_provider_output_sources()
        .expect("authorize the test KMS output");
    backend.platform.for_tests_accept_output_power_commits();
    backend
}

fn randr_output_id(backend: &mut KmsBackend, connector: &str) -> u32 {
    Backend::randr_outputs_and_modes(backend)
        .0
        .into_iter()
        .find(|output| output.name == connector)
        .expect("test output is in the RandR projection")
        .output_id
}

fn apply_modeset_replacement(backend: &mut KmsBackend, mode_vrefresh: u32, crtc: u32) {
    let (connector, output_key, x, y) = {
        let layout = &backend.platform.outputs[0];
        (
            layout.key.connector_name.clone(),
            layout.key.clone(),
            layout.x,
            layout.y,
        )
    };
    let output_id = randr_output_id(backend, &connector);
    let mode = ModeSpec {
        width: 800,
        height: 600,
        vrefresh: mode_vrefresh,
    };
    let mut replacement =
        super::super::randr::clone_test_drm_output(&backend.platform.outputs[0].output);
    replacement.crtc = handle(crtc);
    let advertised = super::super::test_advertised_mode(800, 600, mode_vrefresh, false);
    replacement.picked = advertised.clone();
    replacement.modes.push(advertised.clone());
    backend
        .pstate
        .randr_id_alloc
        .entry_mut(&output_key)
        .modes
        .push(advertised);
    backend.pstate.crtc_config_discovery_override = Some(replacement);
    backend
        .initialize_provider_output_sources()
        .expect("authorize the test KMS output");
    assert!(
        Backend::apply_crtc_config(backend, output_id, &connector, Some(mode), x, y,)
            .expect("apply RandR modeset through Backend")
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: drop same-CRTC VRR state carry-over, including the bounded-clear marker.
fn vrr_state_survives_output_replacement_on_same_crtc() {
    let Some(fixture) = super::recording_scene::RecordingSceneFixture::new(1)
        .expect("recording scene fixture construction")
    else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    let backend = &mut fixture.events.backend;
    let output = &mut backend.platform.outputs[0].output;
    output.crtc_vrr_enabled_prop = Some(handle(VRR_PROP_ID));
    output.connector_vrr_capable = true;
    backend.platform.for_tests_accept_output_power_commits();
    let crtc = u32::from(backend.platform.outputs[0].output.crtc);
    backend.platform.outputs[0].output.vrr_committed.set(true);
    backend.platform.outputs[0].output.vrr_disabled.set(true);
    backend.platform.outputs[0]
        .output
        .vrr_clear_retry_used
        .set(true);

    apply_modeset_replacement(backend, 59, crtc);

    let output = &backend.platform.outputs[0].output;
    let same_crtc_state = (
        u32::from(output.crtc),
        output.vrr_committed.get(),
        output.vrr_disabled.get(),
        output.vrr_clear_retry_used.get(),
    );

    output.vrr_committed.set(true);
    output.vrr_disabled.set(true);
    output.vrr_clear_retry_used.set(true);
    apply_modeset_replacement(backend, 58, crtc + 1);
    let output = &backend.platform.outputs[0].output;
    let new_crtc_state = (
        u32::from(output.crtc),
        output.vrr_committed.get(),
        output.vrr_disabled.get(),
        output.vrr_clear_retry_used.get(),
    );
    fixture.finish_after_normal_drain();

    assert_eq!(same_crtc_state.0, crtc, "the replacement keeps its CRTC");
    assert!(same_crtc_state.1, "the kernel-held value carries over");
    assert!(same_crtc_state.2, "the EINVAL latch carries over");
    assert!(same_crtc_state.3, "the bounded-clear marker carries over");
    assert_eq!(
        new_crtc_state.0,
        crtc + 1,
        "the replacement uses a new CRTC"
    );
    assert!(!new_crtc_state.1, "a new CRTC starts with kernel state 0");
    assert!(
        !new_crtc_state.2,
        "a new CRTC starts without an EINVAL latch"
    );
    assert!(
        !new_crtc_state.3,
        "a new CRTC starts with a fresh clear budget"
    );
}

struct VrrSnapshot(Vec<ConnectorSnapshot>);

impl crate::kms::render::backend::kms::session::VtResumeExecutor for VrrSnapshot {
    fn acquire_master(&self, _device: &KmsDevice) -> io::Result<()> {
        Ok(())
    }

    fn release_master(&self, _device: &KmsDevice) -> io::Result<()> {
        Ok(())
    }

    fn probe_connector_snapshot(
        &self,
        _platform: &PlatformBackend,
    ) -> io::Result<Vec<ConnectorSnapshot>> {
        Ok(self.0.clone())
    }
}

#[test]
// Mutation: skip refreshing connector_vrr_capable from the rescan snapshot.
fn vrr_capable_follows_connector_rescan() {
    let mut backend = vrr_backend();
    let live = &backend.platform.outputs[0];
    let snapshot = ConnectorSnapshot {
        key: live.key.clone(),
        modes: vec![live.output.picked.clone()],
        mm_width: live.output.mm_width,
        mm_height: live.output.mm_height,
        edid: live.output.edid.clone(),
        connector_type: live.output.connector_type.clone(),
        vrr_capable: false,
    };
    backend.for_tests_set_vt_resume_executor(Rc::new(VrrSnapshot(vec![snapshot])));
    backend.pstate.hotplug_rescan_deadline = Some(std::time::Instant::now());
    let mut state = yserver_core::server::ServerState::new();

    backend.backend_session_poll_deferred_input(&mut state);

    let output = &backend.platform.outputs[0].output;
    assert!(
        !output.connector_vrr_capable,
        "the live output adopts snapshot value 0"
    );
    assert!(
        !output.vrr_capable(),
        "VRR is unavailable after the monitor replacement"
    );
}

fn set_vrr_capable(fixture: &mut super::recording_scene::RecordingSceneFixture) {
    let output = &mut fixture.events.backend.platform.outputs[0].output;
    output.crtc_vrr_enabled_prop = Some(handle(VRR_PROP_ID));
    output.connector_vrr_capable = true;
    output.vrr_committed.set(false);
    output.vrr_disabled.set(false);
    output.vrr_clear_retry_used.set(false);
}

fn contains_vrr_property(
    record: &AtomicCommitRecord,
    crtc: ::drm::control::crtc::Handle,
    value: u64,
) -> bool {
    record.properties.iter().any(|update| {
        update.object == crtc.into()
            && update.property == handle(VRR_PROP_ID)
            && update.value == value
    })
}

fn has_vrr_property(record: &AtomicCommitRecord, crtc: ::drm::control::crtc::Handle) -> bool {
    record
        .properties
        .iter()
        .any(|update| update.object == crtc.into() && update.property == handle(VRR_PROP_ID))
}

type PropertyRow = (String, ::drm::control::property::Handle, u64);

#[derive(Clone)]
struct ObjectPropertyRows(Vec<PropertyRow>);

impl ObjectPropertyRows {
    fn map(&self) -> PropMap {
        PropMap::from_discovered_properties(self.0.clone())
    }
}

#[derive(Clone)]
struct DevicePropertyRows {
    planes: Vec<(::drm::control::plane::Handle, ObjectPropertyRows)>,
    crtcs: Vec<(::drm::control::crtc::Handle, ObjectPropertyRows)>,
    connectors: Vec<(::drm::control::connector::Handle, ObjectPropertyRows)>,
}

struct DevicePropertyMaps {
    planes: Vec<(::drm::control::plane::Handle, PropMap)>,
    crtcs: Vec<(::drm::control::crtc::Handle, PropMap)>,
    connectors: Vec<(::drm::control::connector::Handle, PropMap)>,
}

impl DevicePropertyRows {
    fn for_output(output: &crate::drm::modeset::Output, kernel_vrr: u64) -> Self {
        let mut crtc_rows = vec![
            ("ACTIVE".to_owned(), handle(91), 1),
            ("MODE_ID".to_owned(), handle(92), 1),
        ];
        if let Some(vrr) = output.crtc_vrr_enabled_prop {
            crtc_rows.push(("VRR_ENABLED".to_owned(), vrr, kernel_vrr));
        }
        Self {
            planes: vec![(
                output.plane,
                ObjectPropertyRows(vec![
                    ("FB_ID".to_owned(), output.plane_fb_id_prop, 1),
                    (
                        "CRTC_ID".to_owned(),
                        output.plane_crtc_id_prop,
                        u64::from(u32::from(output.crtc)),
                    ),
                ]),
            )],
            crtcs: vec![(output.crtc, ObjectPropertyRows(crtc_rows))],
            connectors: vec![(
                output.connector,
                ObjectPropertyRows(vec![(
                    "CRTC_ID".to_owned(),
                    handle(93),
                    u64::from(u32::from(output.crtc)),
                )]),
            )],
        }
    }

    fn maps(&self) -> DevicePropertyMaps {
        DevicePropertyMaps {
            planes: self
                .planes
                .iter()
                .map(|(handle, properties)| (*handle, properties.map()))
                .collect(),
            crtcs: self
                .crtcs
                .iter()
                .map(|(handle, properties)| (*handle, properties.map()))
                .collect(),
            connectors: self
                .connectors
                .iter()
                .map(|(handle, properties)| (*handle, properties.map()))
                .collect(),
        }
    }
}

struct RecordingOutputPower {
    sink: Rc<AtomicCommitSink>,
    device_properties: DevicePropertyRows,
    events: Rc<RefCell<Vec<&'static str>>>,
}

impl RecordingOutputPower {
    fn submit(
        &self,
        device: &crate::drm::Device,
        request: crate::drm::atomic::AtomicCommitRequest,
    ) -> io::Result<()> {
        let result = self.sink.submit(
            device,
            ::drm::control::AtomicCommitFlags::ALLOW_MODESET,
            AtomicCommitKind::Scene,
            request,
            &[],
        );
        self.events.borrow_mut().push("atomic_submit");
        result
    }
}

impl OutputPowerExecutor for RecordingOutputPower {
    fn disable(
        &self,
        device: &crate::drm::Device,
        output: &crate::drm::modeset::Output,
    ) -> io::Result<()> {
        let properties = DevicePropertyRows::for_output(output, 0);
        let connector = properties.connectors[0].1.map();
        let crtc = properties.crtcs[0].1.map();
        crate::drm::modeset::disable_output_with_properties(output, &connector, &crtc, |request| {
            self.submit(device, request)
        })
    }

    fn enable(
        &self,
        _device: &crate::drm::Device,
        _output: &crate::drm::modeset::Output,
        _framebuffer: ::drm::control::framebuffer::Handle,
    ) -> io::Result<()> {
        self.events.borrow_mut().push("enable");
        Ok(())
    }

    fn disable_device(&self, device: &crate::drm::Device) -> io::Result<()> {
        let maps = self.device_properties.maps();
        crate::drm::modeset::disable_all_outputs_with_properties(
            maps.planes,
            maps.crtcs,
            maps.connectors,
            |request| self.submit(device, request),
        )
    }
}

fn install_recording_power(
    backend: &mut KmsBackend,
    recorder: Rc<RecordingAtomicCommitExecutor>,
    kernel_vrr: u64,
    events: Rc<RefCell<Vec<&'static str>>>,
) {
    let sink = Rc::new(AtomicCommitSink::new(recorder));
    backend.platform.devices[0].atomic_commit_sink = Rc::clone(&sink);
    let properties =
        DevicePropertyRows::for_output(&backend.platform.outputs[0].output, kernel_vrr);
    backend
        .platform
        .for_tests_set_output_power_executor(Rc::new(RecordingOutputPower {
            sink,
            device_properties: properties,
            events,
        }));
}

fn new_power_recorder() -> Rc<RecordingAtomicCommitExecutor> {
    Rc::new(RecordingAtomicCommitExecutor::default())
}

fn backend_snapshot(backend: &KmsBackend) -> Vec<ConnectorSnapshot> {
    backend
        .platform
        .outputs
        .iter()
        .map(|layout| ConnectorSnapshot {
            key: layout.key.clone(),
            modes: vec![layout.output.picked.clone()],
            mm_width: layout.output.mm_width,
            mm_height: layout.output.mm_height,
            edid: layout.output.edid.clone(),
            connector_type: layout.output.connector_type.clone(),
            vrr_capable: layout.output.connector_vrr_capable,
        })
        .collect()
}

struct KernelVrrVtResume {
    snapshots: Vec<ConnectorSnapshot>,
    kernel_vrr: u64,
    events: Rc<RefCell<Vec<&'static str>>>,
}

impl crate::kms::render::backend::kms::session::VtResumeExecutor for KernelVrrVtResume {
    fn acquire_master(&self, _device: &KmsDevice) -> io::Result<()> {
        Ok(())
    }

    fn release_master(&self, _device: &KmsDevice) -> io::Result<()> {
        self.events.borrow_mut().push("release_master");
        Ok(())
    }

    fn crtc_vrr_enabled_value(
        &self,
        _device: &KmsDevice,
        _crtc: ::drm::control::crtc::Handle,
        property: ::drm::control::property::Handle,
    ) -> io::Result<Option<u64>> {
        assert_eq!(property, handle(VRR_PROP_ID));
        self.events.borrow_mut().push("read_vrr_enabled");
        Ok(Some(self.kernel_vrr))
    }

    fn probe_connector_snapshot(
        &self,
        _platform: &PlatformBackend,
    ) -> io::Result<Vec<ConnectorSnapshot>> {
        Ok(self.snapshots.clone())
    }
}

#[test]
// Mutation: omit VRR_ENABLED = 0 from disable_output; the recorded VT off commit must fail this assertion.
fn vrr_vt_release_clears_before_drop_master() {
    let mut backend = vrr_backend();
    let crtc = backend.platform.outputs[0].output.crtc;
    backend.platform.outputs[0].output.vrr_committed.set(true);
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorder = new_power_recorder();
    install_recording_power(&mut backend, Rc::clone(&recorder), 1, Rc::clone(&events));
    backend.for_tests_set_vt_resume_executor(Rc::new(KernelVrrVtResume {
        snapshots: backend_snapshot(&backend),
        kernel_vrr: 1,
        events: Rc::clone(&events),
    }));
    let mut state = yserver_core::server::ServerState::new();

    Backend::begin_vt_release(&mut backend);
    Backend::finish_vt_release(
        &mut backend,
        &mut state,
        &yserver_core::core_loop::input_inventory::InputInventory::default(),
    );

    let off = recorder
        .records()
        .into_iter()
        .find(|record| has_vrr_property(record, crtc))
        .expect("VT release sends the production-built output disable");
    assert!(contains_vrr_property(&off, crtc, 0));
    assert_eq!(off.result, ScriptedResult::Ok);
    assert!(
        !backend.platform.outputs[0].output.vrr_committed.get(),
        "accepted off commit updates the cached kernel-held value"
    );
    let events = events.borrow();
    assert!(
        events.iter().position(|event| *event == "atomic_submit")
            < events.iter().position(|event| *event == "release_master"),
        "the all-off commit is submitted before DRM master is released"
    );
    assert_eq!(
        backend.pstate.vt_state,
        crate::vt::state::VtState::Suspended,
        "the VT release reaches its suspended end state"
    );
}

#[test]
// Mutation: omit VRR_ENABLED = 0 from disable_all_outputs_on_device; shutdown's device commit must fail this assertion.
fn vrr_shutdown_clears() {
    let mut backend = vrr_backend();
    let crtc = backend.platform.outputs[0].output.crtc;
    backend.platform.outputs[0].output.vrr_committed.set(true);
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorder = new_power_recorder();
    install_recording_power(&mut backend, Rc::clone(&recorder), 1, events);

    backend
        .disable_output()
        .expect("shutdown disables the device");

    let clear = recorder
        .records()
        .into_iter()
        .find(|record| has_vrr_property(record, crtc))
        .expect("device-wide shutdown commit carries the inherited VRR property");
    assert!(contains_vrr_property(&clear, crtc, 0));
    assert_eq!(clear.result, ScriptedResult::Ok);
    assert!(
        backend.platform.shutting_down,
        "Backend completed shutdown teardown"
    );
}

#[test]
// Mutation: skip the one stripped retry after a rejected VRR clear; release must still reach its suspended end state.
fn vrr_clear_rejected_release_completes() {
    super::recording_scene::init_tearing_test_logger();
    super::recording_scene::clear_vrr_test_logs();
    let mut backend = vrr_backend();
    let crtc = backend.platform.outputs[0].output.crtc;
    backend.platform.outputs[0].output.vrr_committed.set(true);
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorder = new_power_recorder();
    install_recording_power(&mut backend, Rc::clone(&recorder), 1, Rc::clone(&events));
    recorder.script_result(Err(io::Error::from_raw_os_error(libc::EIO)));
    backend.for_tests_set_vt_resume_executor(Rc::new(KernelVrrVtResume {
        snapshots: backend_snapshot(&backend),
        kernel_vrr: 1,
        events: Rc::clone(&events),
    }));
    let mut state = yserver_core::server::ServerState::new();

    Backend::begin_vt_release(&mut backend);
    Backend::finish_vt_release(
        &mut backend,
        &mut state,
        &yserver_core::core_loop::input_inventory::InputInventory::default(),
    );

    let records = recorder.records();
    let attempts: Vec<_> = records
        .iter()
        .filter(|record| record.kind == AtomicCommitKind::Scene)
        .collect();
    assert_eq!(attempts.len(), 2, "the rejected clear gets one retry");
    assert!(contains_vrr_property(attempts[0], crtc, 0));
    assert!(matches!(
        attempts[0].result,
        ScriptedResult::Error {
            raw_os_error: Some(error),
            ..
        } if error == libc::EIO
    ));
    assert!(!has_vrr_property(attempts[1], crtc));
    assert_eq!(attempts[1].result, ScriptedResult::Ok);
    assert!(
        backend.platform.outputs[0].output.vrr_committed.get(),
        "a successful retry without VRR leaves the cached value unchanged"
    );
    assert_eq!(
        backend.pstate.vt_state,
        crate::vt::state::VtState::Suspended,
        "the failed clear and stripped retry do not block VT release"
    );
    let events = events.borrow();
    let release = events
        .iter()
        .position(|event| *event == "release_master")
        .expect("VT release drops DRM master");
    assert!(
        events
            .iter()
            .enumerate()
            .filter(|(_, event)| **event == "atomic_submit")
            .all(|(index, _)| index < release),
        "both clear attempts precede release_master"
    );
    assert!(
        super::recording_scene::vrr_test_logs().contains(&format!(
            "kms: vrr clear rejected on CRTC {}:",
            u32::from(crtc)
        )),
        "the rejected release clear is logged"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: leave vrr_committed true after an accepted DPMS clear; the output end-state assertion must fail.
fn vrr_dpms_off_clears_and_direct_frame_reenables() {
    let Some((fixture, direct)) = direct_fixture(18_010, 0) else {
        eprintln!(
            "SKIP vrr_dpms_off_clears_and_direct_frame_reenables: no Vulkan scene fixture available"
        );
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    set_vrr_capable(&mut fixture);
    let fd = super::direct_unflip::fd(&fixture);
    let crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    let recorder = fixture.events.recorders[&fd].clone();
    let events = Rc::new(RefCell::new(Vec::new()));
    install_recording_power(&mut fixture.events.backend, Rc::clone(&recorder), 0, events);

    let initial = accepted_direct_record(&mut fixture, &direct, fd);
    let initial_id = super::support::accepted_event_id(&initial).expect("initial direct event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &initial, initial_id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[],
    );

    let dpms_offset = recorder.records().len();
    Backend::set_dpms_power(&mut fixture.events.backend, 3).expect("DPMS off through Backend");
    let dpms_clear = recorder.records()[dpms_offset..]
        .iter()
        .find(|record| contains_vrr_property(record, crtc, 0))
        .cloned()
        .expect("DPMS off commit clears VRR_ENABLED");
    assert_eq!(dpms_clear.result, ScriptedResult::Ok);
    assert!(
        !fixture.events.backend.platform.outputs[0]
            .output
            .vrr_committed
            .get(),
        "accepted DPMS off commit records VRR as disabled"
    );
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[],
        &[direct.candidate.present_id],
    );

    Backend::set_dpms_power(&mut fixture.events.backend, 0).expect("DPMS on through Backend");
    let resumed_direct = fixture.install_direct_present(18_011, 0);
    let direct_frame = accepted_direct_record(&mut fixture, &resumed_direct, fd);
    assert!(contains_vrr_property(&direct_frame, crtc, 1));
    assert!(
        fixture.events.backend.platform.outputs[0]
            .output
            .vrr_committed
            .get(),
        "the next direct frame restores the committed VRR state"
    );
    let resumed_id = super::support::accepted_event_id(&direct_frame)
        .expect("post-DPMS direct commit has an event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &direct_frame, resumed_id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[resumed_direct.candidate.present_id],
        &[],
    );
    finish_direct_scanout(&mut fixture, &resumed_direct, fd);
    fixture.finish_after_normal_drain();
}

#[test]
// Mutation: skip refresh_vrr_committed_from_kernel after device-wide disable; the inherited value must be observed.
fn vrr_vt_acquire_reads_value_left_by_other_master() {
    let mut backend = vrr_backend();
    let events = Rc::new(RefCell::new(Vec::new()));
    let recorder = new_power_recorder();
    install_recording_power(&mut backend, Rc::clone(&recorder), 1, Rc::clone(&events));
    backend.for_tests_set_vt_resume_executor(Rc::new(KernelVrrVtResume {
        snapshots: backend_snapshot(&backend),
        kernel_vrr: 1,
        events: Rc::clone(&events),
    }));
    let mut state = yserver_core::server::ServerState::new();

    // Enter the suspended state through Backend before modeling a value left
    // by the other master; on_vt_acquire is a no-op while already Active.
    Backend::begin_vt_release(&mut backend);
    Backend::finish_vt_release(
        &mut backend,
        &mut state,
        &yserver_core::core_loop::input_inventory::InputInventory::default(),
    );
    backend.platform.outputs[0].output.vrr_committed.set(false);
    recorder.clear_records();
    events.borrow_mut().clear();
    recorder.script_result(Err(io::Error::from_raw_os_error(libc::EIO)));
    recorder.script_result(Err(io::Error::from_raw_os_error(libc::EIO)));

    Backend::on_vt_acquire(&mut backend, &mut state);

    let crtc = backend.platform.outputs[0].output.crtc;
    let attempts: Vec<_> = recorder
        .records()
        .into_iter()
        .filter(|record| record.kind == AtomicCommitKind::Scene)
        .collect();
    assert_eq!(attempts.len(), 2, "the refused device clear gets one retry");
    assert!(contains_vrr_property(&attempts[0], crtc, 0));
    assert!(matches!(
        attempts[0].result,
        ScriptedResult::Error {
            raw_os_error: Some(error),
            ..
        } if error == libc::EIO
    ));
    assert!(!has_vrr_property(&attempts[1], crtc));
    assert!(matches!(
        attempts[1].result,
        ScriptedResult::Error {
            raw_os_error: Some(error),
            ..
        } if error == libc::EIO
    ));
    assert!(
        backend.platform.outputs[0].output.vrr_committed.get(),
        "VT acquire refreshes the value left by the previous DRM master"
    );
    let events = events.borrow();
    let read = events
        .iter()
        .position(|event| *event == "read_vrr_enabled")
        .expect("VT acquire reads VRR_ENABLED from the CRTC");
    assert!(
        events
            .iter()
            .enumerate()
            .filter(|(_, event)| **event == "atomic_submit")
            .all(|(index, _)| index < read),
        "the refresh follows the device-wide clear attempt"
    );
}

fn accepted_direct_record(
    fixture: &mut super::recording_scene::RecordingSceneFixture,
    direct: &super::recording_scene::DirectPresentSetup,
    fd: std::os::fd::RawFd,
) -> AtomicCommitRecord {
    super::direct_unflip::submit_direct(fixture, direct, fd)
        .into_iter()
        .find(|record| super::support::accepted_event_id(record).is_some())
        .expect("Backend accepts the direct frame")
}

fn submit_direct_successor(
    fixture: &mut super::recording_scene::RecordingSceneFixture,
    direct: &mut super::recording_scene::DirectPresentSetup,
    fd: std::os::fd::RawFd,
) -> AtomicCommitRecord {
    let mut candidate = direct.candidate;
    candidate.present_id = candidate.present_id.wrapping_add(1);
    let mut event = direct.event.clone();
    event.present_id = candidate.present_id;
    event.serial = event.serial.wrapping_add(1);
    let offset = super::direct_unflip::record_count(fixture, fd);
    let mut accepted = false;
    for _ in 0..8 {
        accepted =
            Backend::try_present_direct(&mut fixture.events.backend, candidate, event.clone())
                .expect("successor Present through Backend");
        if accepted {
            break;
        }
    }
    assert!(accepted, "eligible direct successor reaches scanout");
    direct.candidate = candidate;
    direct.event = event;
    super::direct_unflip::records_since(fixture, fd, offset)
        .into_iter()
        .find(|record| super::support::accepted_event_id(record).is_some())
        .expect("Backend accepts the direct successor")
}

fn finish_direct_scanout(
    fixture: &mut super::recording_scene::RecordingSceneFixtureGuard,
    direct: &super::recording_scene::DirectPresentSetup,
    fd: std::os::fd::RawFd,
) {
    super::direct_unflip::request_unflip(fixture, direct);
    let offset = super::direct_unflip::record_count(fixture, fd);
    let unflip = super::direct_unflip::grouped_unflip_record(fixture, fd, offset, 1);
    let id = super::support::accepted_event_id(&unflip).expect("composed replacement ID");
    super::atomic_events::complete_recorded_attempt(fixture, fd, &unflip, id, 80);
    super::direct_unflip::drain_present_notifications(fixture, &[], &[direct.candidate.present_id]);
}

fn direct_fixture(
    present_id: u64,
    options: u32,
) -> Option<(
    super::recording_scene::RecordingSceneFixture,
    super::recording_scene::DirectPresentSetup,
)> {
    super::direct_unflip::direct_fixture(
        1,
        super::recording_scene::P1SceneRoute::Shared,
        present_id,
        options,
    )
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: never append VRR_ENABLED to an eligible direct frame.
fn vrr_direct_unredirected_enables() {
    let Some((fixture, direct)) = direct_fixture(18_001, 0) else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    set_vrr_capable(&mut fixture);
    let fd = super::direct_unflip::fd(&fixture);
    let crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    let record = accepted_direct_record(&mut fixture, &direct, fd);

    assert!(contains_vrr_property(&record, crtc, 1));
    assert!(
        fixture.events.backend.platform.outputs[0]
            .output
            .vrr_committed
            .get()
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .pending
            .as_ref()
            .and_then(|frame| frame.accepted_event_id),
        super::support::accepted_event_id(&record),
        "the frame carrying VRR is retained for retirement"
    );
    let id = super::support::accepted_event_id(&record).expect("accepted direct commit ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &record, id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[],
    );
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_some());
    finish_direct_scanout(&mut fixture, &direct, fd);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit VRR_ENABLED = 0 from the composed replacement after unflip.
fn vrr_unflip_clears() {
    let Some((fixture, direct)) = direct_fixture(18_002, 0) else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    set_vrr_capable(&mut fixture);
    let fd = super::direct_unflip::fd(&fixture);
    let crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    let direct_record = accepted_direct_record(&mut fixture, &direct, fd);
    let id = super::support::accepted_event_id(&direct_record).expect("direct event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &direct_record, id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[],
    );

    super::direct_unflip::request_unflip(&mut fixture, &direct);
    let offset = super::direct_unflip::record_count(&fixture, fd);
    let unflip = super::direct_unflip::grouped_unflip_record(&mut fixture, fd, offset, 1);
    assert!(contains_vrr_property(&unflip, crtc, 0));
    assert!(
        !fixture.events.backend.platform.outputs[0]
            .output
            .vrr_committed
            .get()
    );
    let unflip_id = super::support::accepted_event_id(&unflip).expect("unflip event ID");
    super::atomic_events::complete_recorded_attempt(&mut fixture, fd, &unflip, unflip_id, 51);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[],
        &[direct.candidate.present_id],
    );

    let later = super::atomic_events::submit_damaged_frame(&mut fixture);
    let later = later
        .iter()
        .find(|(_, record)| super::support::accepted_event_id(record).is_some())
        .expect("later scene frame accepted")
        .1
        .clone();
    assert!(!contains_vrr_property(&later, crtc, 0));
    assert!(!contains_vrr_property(&later, crtc, 1));
    super::atomic_events::complete_recorded_attempt(
        &mut fixture,
        fd,
        &later,
        super::support::accepted_event_id(&later).unwrap(),
        52,
    );
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: accept every direct owner as VRR-worthy, including Cow and CowDescendant.
fn vrr_cow_owner_never_enables() {
    for owner in ["cow", "cow_descendant"] {
        let Some((fixture, mut direct)) = direct_fixture(18_003, 0) else {
            eprintln!("skip: no Vulkan scene fixture available");
            return;
        };
        let mut fixture = fixture.cleanup_guard();
        set_vrr_capable(&mut fixture);
        Backend::get_overlay_window(&mut fixture.events.backend, None)
            .expect("materialize compositor overlay window");
        let cow_xid = fixture
            .events
            .backend
            .cow_host_xid()
            .expect("overlay window host XID");
        let root_width = fixture.events.backend.platform.fb_w;
        let root_height = fixture.events.backend.platform.fb_h;
        if owner == "cow" {
            direct.candidate.paint_dst_host_xid = cow_xid;
            direct.candidate.dst_window_xid = cow_xid;
            direct.candidate.completion_dst_host_xid = cow_xid;
            direct.event.dst_host_xid = cow_xid;
        } else {
            let target = fixture
                .events
                .backend
                .windows
                .get_mut(&direct.target_xid)
                .expect("direct target geometry");
            target.parent = Some(cow_xid);
            target.width = root_width;
            target.height = root_height;
        }
        let fd = super::direct_unflip::fd(&fixture);
        let crtc = fixture.events.backend.platform.outputs[0].output.crtc;
        let record = accepted_direct_record(&mut fixture, &direct, fd);
        assert!(!has_vrr_property(&record, crtc), "owner={owner}");
        assert!(
            !fixture.events.backend.platform.outputs[0]
                .output
                .vrr_committed
                .get(),
            "owner={owner} must leave VRR off"
        );
        let id = super::support::accepted_event_id(&record).expect("direct event ID");
        super::direct_unflip::complete_direct_initial(&mut fixture, &record, id, fd);
        super::direct_unflip::drain_present_notifications(
            &mut fixture,
            &[direct.candidate.present_id],
            &[],
        );
        finish_direct_scanout(&mut fixture, &direct, fd);
        Backend::release_overlay_window(&mut fixture.events.backend, None)
            .expect("release compositor overlay window");
        fixture.finish_after_normal_drain();
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: write zero unconditionally, add a frame, alter property values, or change flags.
fn vrr_not_capable_stream_unchanged() {
    let cases = [
        (
            "connector vrr_capable absent",
            false,
            Some(handle(VRR_PROP_ID)),
        ),
        (
            "connector vrr_capable is zero",
            false,
            Some(handle(VRR_PROP_ID)),
        ),
        ("CRTC has no VRR_ENABLED", true, None),
    ];
    let flags = ::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT
        | ::drm::control::AtomicCommitFlags::NONBLOCK;
    let prop = |id, value| ("1".to_owned(), format!("property::Handle({id})"), value);
    let direct_properties = vec![
        prop(71, 51028),
        prop(72, 1),
        prop(73, 0),
        prop(74, 0),
        prop(75, 52428800),
        prop(76, 39321600),
        prop(77, 0),
        prop(78, 0),
        prop(79, 800),
        prop(80, 600),
    ];
    let expected: Vec<CommitSignature> = vec![
        (AtomicCommitKind::Scene, flags, direct_properties.clone()),
        (AtomicCommitKind::Scene, flags, direct_properties),
        (
            AtomicCommitKind::Scene,
            flags,
            vec![
                prop(71, 20993),
                prop(72, 1),
                prop(73, 0),
                prop(74, 0),
                prop(75, 0),
                prop(76, 0),
                prop(77, 0),
                prop(78, 0),
                prop(79, 0),
                prop(80, 0),
                prop(81, 0),
            ],
        ),
        (
            AtomicCommitKind::Scene,
            flags,
            vec![prop(71, 20992), prop(72, 1), prop(81, 0), prop(82, 0)],
        ),
    ];
    for (label, connector_capable, property) in cases {
        let Some((fixture, mut direct)) = direct_fixture(18_004, 0) else {
            eprintln!("skip: no Vulkan scene fixture available");
            return;
        };
        let mut fixture = fixture.cleanup_guard();
        let output = &mut fixture.events.backend.platform.outputs[0].output;
        output.crtc_vrr_enabled_prop = property;
        output.connector_vrr_capable = connector_capable;
        output.vrr_committed.set(false);
        output.vrr_disabled.set(false);
        output.vrr_clear_retry_used.set(false);
        let fd = super::direct_unflip::fd(&fixture);
        let recorder = fixture.events.recorders[&fd].clone();
        recorder.clear_records();
        let crtc = fixture.events.backend.platform.outputs[0].output.crtc;
        let first = accepted_direct_record(&mut fixture, &direct, fd);
        let first_id = super::support::accepted_event_id(&first).expect("direct event ID");
        let first_present_id = direct.candidate.present_id;
        super::direct_unflip::complete_direct_initial(&mut fixture, &first, first_id, fd);
        super::direct_unflip::drain_present_notifications(
            &mut fixture,
            &[direct.candidate.present_id],
            &[],
        );
        {
            let events = &mut fixture.events;
            Backend::warp_pointer_root(&mut events.backend, &mut events.state, 111, 77);
        }
        let second = submit_direct_successor(&mut fixture, &mut direct, fd);
        let second_id = super::support::accepted_event_id(&second).expect("second event ID");
        assert_eq!(
            (
                fixture.events.backend.core.cursor_x,
                fixture.events.backend.core.cursor_y
            ),
            (111.0, 77.0),
            "cursor motion is applied while direct scanout continues"
        );
        super::direct_unflip::complete_direct_initial(&mut fixture, &second, second_id, fd);
        super::direct_unflip::drain_present_notifications(
            &mut fixture,
            &[direct.candidate.present_id],
            &[first_present_id],
        );
        super::direct_unflip::request_unflip(&mut fixture, &direct);
        let offset = super::direct_unflip::record_count(&fixture, fd);
        let unflip = super::direct_unflip::grouped_unflip_record(&mut fixture, fd, offset, 1);
        let unflip_id = super::support::accepted_event_id(&unflip).expect("unflip event ID");
        super::atomic_events::complete_recorded_attempt(&mut fixture, fd, &unflip, unflip_id, 61);
        super::direct_unflip::drain_present_notifications(
            &mut fixture,
            &[],
            &[direct.candidate.present_id],
        );
        let scene = super::atomic_events::submit_damaged_frame(&mut fixture);
        let scene = scene
            .iter()
            .find(|(_, record)| super::support::accepted_event_id(record).is_some())
            .expect("scene frame accepted")
            .1
            .clone();
        let records = recorder.records();
        let signature: Vec<_> = records
            .iter()
            .map(|record| {
                (
                    record.kind,
                    record.flags,
                    record
                        .properties
                        .iter()
                        .map(|update| {
                            (
                                format!("{:?}", update.object),
                                format!("{:?}", update.property),
                                if matches!(u32::from(update.property), 81 | 82) {
                                    0
                                } else {
                                    update.value
                                },
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        super::atomic_events::complete_recorded_attempt(
            &mut fixture,
            fd,
            &scene,
            super::support::accepted_event_id(&scene).unwrap(),
            62,
        );
        fixture.finish_after_normal_drain();
        assert!(
            records.iter().all(|record| {
                !record.properties.iter().any(|update| {
                    update.object == crtc.into() && update.property == handle(VRR_PROP_ID)
                })
            }),
            "{label} emits no VRR property"
        );
        assert_eq!(
            signature, expected,
            "{label} changes the pre-VRR commit stream"
        );
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: append VRR_ENABLED to the async direct request instead of deferring it.
fn vrr_async_flip_defers_to_next_vsync_commit() {
    let options = yserver_core::present_scheduler::PRESENT_OPTION_ASYNC;
    let Some((fixture, mut direct)) = direct_fixture(18_005, options) else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    set_vrr_capable(&mut fixture);
    let fd = super::direct_unflip::fd(&fixture);
    super::direct_unflip::enter_async_tearing(&mut fixture, &direct);
    let crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    let async_record = accepted_direct_record(&mut fixture, &direct, fd);
    assert_eq!(async_record.kind, AtomicCommitKind::Async);
    assert!(!has_vrr_property(&async_record, crtc));
    let async_id = super::support::accepted_event_id(&async_record).expect("async event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &async_record, async_id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[],
    );

    direct.candidate.options = 0;
    let vsync = submit_direct_successor(&mut fixture, &mut direct, fd);
    assert_ne!(vsync.kind, AtomicCommitKind::Async);
    assert!(contains_vrr_property(&vsync, crtc, 1));
    assert!(
        fixture.events.backend.platform.outputs[0]
            .output
            .vrr_committed
            .get()
    );
    let vsync_id = super::support::accepted_event_id(&vsync).expect("vsync event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &vsync, vsync_id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[direct.candidate.present_id - 1],
    );
    finish_direct_scanout(&mut fixture, &direct, fd);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: fail to latch VRR off after EINVAL and an accepted no-property retry.
fn vrr_einval_latches_and_frame_goes_out() {
    let Some((fixture, mut direct)) = direct_fixture(18_006, 0) else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    let fd = super::direct_unflip::fd(&fixture);
    let recorder = fixture.events.recorders[&fd].clone();
    let first = accepted_direct_record(&mut fixture, &direct, fd);
    let first_id = super::support::accepted_event_id(&first).expect("initial frame event ID");
    let first_present_id = direct.candidate.present_id;
    super::direct_unflip::complete_direct_initial(&mut fixture, &first, first_id, fd);
    super::direct_unflip::drain_present_notifications(&mut fixture, &[first_present_id], &[]);
    set_vrr_capable(&mut fixture);
    let crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    recorder.clear_records();
    let offset = super::direct_unflip::record_count(&fixture, fd);
    recorder.script_result(Err(io::Error::from_raw_os_error(libc::EINVAL)));
    let retry_record = submit_direct_successor(&mut fixture, &mut direct, fd);
    assert!(!has_vrr_property(&retry_record, crtc));
    let records = super::direct_unflip::records_since(&fixture, fd, offset);
    let rejected = records
        .iter()
        .find(|record| contains_vrr_property(record, crtc, 1))
        .expect("first direct request includes VRR_ENABLED = 1");
    assert!(matches!(
        &rejected.result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            raw_os_error: Some(error),
            ..
        } if *error == libc::EINVAL
    ));
    let accepted = records
        .iter()
        .find(|record| super::support::accepted_event_id(record).is_some())
        .expect("same direct frame is accepted by retry");
    assert!(!has_vrr_property(accepted, crtc));
    assert_eq!(
        records.len(),
        2,
        "one rejected request and its accepted retry"
    );
    assert!(
        fixture.events.backend.platform.outputs[0]
            .output
            .vrr_disabled
            .get()
    );
    assert!(
        !fixture.events.backend.platform.outputs[0]
            .output
            .vrr_committed
            .get(),
        "the rejected value was not committed"
    );
    let id = super::support::accepted_event_id(accepted).expect("retry event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, accepted, id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[first_present_id],
    );

    let second_present_id = direct.candidate.present_id;
    let later = submit_direct_successor(&mut fixture, &mut direct, fd);
    assert!(!has_vrr_property(&later, crtc));
    assert!(fixture.events.backend.pstate.scanout_m2.pending.is_some());
    let later_id = super::support::accepted_event_id(&later).expect("later frame event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &later, later_id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[second_present_id],
    );
    finish_direct_scanout(&mut fixture, &direct, fd);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep appending VRR_ENABLED = 0 after the bounded retry also gets EINVAL.
fn vrr_rejected_clear_is_bounded() {
    let Some((fixture, mut direct)) = direct_fixture(18_008, 0) else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    set_vrr_capable(&mut fixture);
    let fd = super::direct_unflip::fd(&fixture);
    let recorder = fixture.events.recorders[&fd].clone();
    let initial = accepted_direct_record(&mut fixture, &direct, fd);
    let initial_id = super::support::accepted_event_id(&initial).expect("direct event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &initial, initial_id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[],
    );
    let crtc = fixture.events.backend.platform.outputs[0].output.crtc;

    recorder.script_result(Err(io::Error::from_raw_os_error(libc::EINVAL)));
    super::direct_unflip::request_unflip(&mut fixture, &direct);
    let unflip_offset = super::direct_unflip::record_count(&fixture, fd);
    let unflip = super::direct_unflip::grouped_unflip_record(&mut fixture, fd, unflip_offset, 1);
    let unflip_attempts = super::direct_unflip::records_since(&fixture, fd, unflip_offset);
    let unflip_id = super::support::accepted_event_id(&unflip).expect("unflip retry event ID");
    let committed_after_unflip = fixture.events.backend.platform.outputs[0]
        .output
        .vrr_committed
        .get();
    super::atomic_events::complete_recorded_attempt(&mut fixture, fd, &unflip, unflip_id, 81);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[],
        &[direct.candidate.present_id],
    );

    recorder.script_result(Err(io::Error::from_raw_os_error(libc::EINVAL)));
    let scene_offset = super::direct_unflip::record_count(&fixture, fd);
    let scene_attempts = super::atomic_events::submit_damaged_frame(&mut fixture);
    let scene = scene_attempts
        .iter()
        .find(|(_, record)| super::support::accepted_event_id(record).is_some())
        .expect("scene frame accepted by the no-property retry")
        .1
        .clone();
    let scene_records = super::direct_unflip::records_since(&fixture, fd, scene_offset);
    let committed_after_scene = fixture.events.backend.platform.outputs[0]
        .output
        .vrr_committed
        .get();
    let disabled_after_scene = fixture.events.backend.platform.outputs[0]
        .output
        .vrr_disabled
        .get();
    let scene_id = super::support::accepted_event_id(&scene).expect("scene retry event ID");
    super::atomic_events::complete_recorded_attempt(&mut fixture, fd, &scene, scene_id, 82);

    let mut later_candidate = direct.candidate;
    later_candidate.present_id = later_candidate.present_id.wrapping_add(2);
    let mut later_event = direct.event.clone();
    later_event.present_id = later_candidate.present_id;
    later_event.serial = later_event.serial.wrapping_add(2);
    let later_offset = super::direct_unflip::record_count(&fixture, fd);
    let mut later_admitted = false;
    for _ in 0..8 {
        later_admitted = Backend::try_present_direct(
            &mut fixture.events.backend,
            later_candidate,
            later_event.clone(),
        )
        .expect("later direct Present through Backend");
        if later_admitted {
            break;
        }
    }
    assert!(later_admitted, "later direct Present passes admission");
    let later = super::direct_unflip::records_since(&fixture, fd, later_offset)
        .into_iter()
        .find(|record| super::support::accepted_event_id(record).is_some())
        .expect("later direct frame accepted");
    let later_has_vrr = has_vrr_property(&later, crtc);
    let later_id = super::support::accepted_event_id(&later).expect("later scene event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &later, later_id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[later_candidate.present_id],
        &[],
    );
    direct.candidate = later_candidate;
    direct.event = later_event;
    finish_direct_scanout(&mut fixture, &direct, fd);
    fixture.finish_after_normal_drain();

    assert_eq!(unflip_attempts.len(), 2, "one clear request and its retry");
    assert!(contains_vrr_property(&unflip_attempts[0], crtc, 0));
    assert!(!has_vrr_property(&unflip_attempts[1], crtc));
    assert!(
        committed_after_unflip,
        "rejected zero does not change kernel state"
    );
    assert_eq!(
        scene_records.len(),
        2,
        "one bounded clear request and its retry"
    );
    assert!(contains_vrr_property(&scene_records[0], crtc, 0));
    assert!(!has_vrr_property(&scene_records[1], crtc));
    assert!(
        committed_after_scene,
        "rejected zero does not falsify committed state"
    );
    assert!(disabled_after_scene, "EINVAL latches VRR off for this CRTC");
    assert!(
        !later_has_vrr,
        "the failed bounded clear is not repeated forever"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: hard-code `reason=inherited` for a scene clear deferred by gamma.
fn vrr_deferred_scene_clear_logs_the_direct_exit_reason() {
    let Some((fixture, direct)) = direct_fixture(18_009, 0) else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    set_vrr_capable(&mut fixture);
    let fd = super::direct_unflip::fd(&fixture);
    let recorder = fixture.events.recorders[&fd].clone();
    recorder.set_gamma_size_for_tests(4);
    let initial = accepted_direct_record(&mut fixture, &direct, fd);
    let initial_id = super::support::accepted_event_id(&initial).expect("direct event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &initial, initial_id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[],
    );
    super::recording_scene::clear_vrr_test_logs();
    let output_name = "vrr-deferred-exit-test";
    fixture.events.backend.platform.outputs[0]
        .key
        .connector_name = output_name.to_owned();
    fixture.events.backend.platform.outputs[0]
        .output
        .connector_name = output_name.to_owned();

    {
        let output = &mut fixture.events.backend.platform.outputs[0].output;
        output.crtc_gamma_lut_prop = Some(handle(70));
        output.crtc_degamma_lut_prop = Some(handle(71));
        output.crtc_ctm_prop = Some(handle(72));
        output.crtc_gamma_lut_size = Some(4);
    }
    let output_key = fixture.events.backend.platform.outputs[0].key.clone();
    let crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    fixture
        .events
        .backend
        .pstate
        .crtc_key_by_id
        .insert(u32::from(crtc), output_key);
    Backend::set_crtc_gamma(
        &mut fixture.events.backend,
        u32::from(crtc),
        &[0x1111; 4],
        &[0x2222; 4],
        &[0x3333; 4],
    )
    .expect("queue gamma before unflip");

    super::direct_unflip::request_unflip(&mut fixture, &direct);
    let unflip_offset = super::direct_unflip::record_count(&fixture, fd);
    let unflip = super::direct_unflip::grouped_unflip_record(&mut fixture, fd, unflip_offset, 1);
    let unflip_has_gamma = unflip
        .properties
        .iter()
        .any(|update| update.object == crtc.into() && update.property == handle(70));
    let unflip_has_vrr = has_vrr_property(&unflip, crtc);
    let unflip_id = super::support::accepted_event_id(&unflip).expect("unflip event ID");
    super::atomic_events::complete_recorded_attempt(&mut fixture, fd, &unflip, unflip_id, 84);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[],
        &[direct.candidate.present_id],
    );
    let reason_retained = fixture.events.backend.pstate.scanout_m2.unflip_last_reason
        == Some("ineligible_authoritative_root_present");
    let committed_after_gamma_unflip = fixture.events.backend.platform.outputs[0]
        .output
        .vrr_committed
        .get();

    let scene_attempts = super::atomic_events::submit_damaged_frame(&mut fixture);
    let scene = scene_attempts
        .iter()
        .find(|(_, record)| super::support::accepted_event_id(record).is_some())
        .expect("scene frame accepted after gamma")
        .1
        .clone();
    let scene_has_vrr_clear = contains_vrr_property(&scene, crtc, 0);
    let scene_id = super::support::accepted_event_id(&scene).expect("scene event ID");
    super::atomic_events::complete_recorded_attempt(&mut fixture, fd, &scene, scene_id, 85);
    let logs = super::recording_scene::vrr_test_logs();
    fixture.finish_after_normal_drain();

    assert!(unflip_has_gamma, "the unflip carries the pending gamma LUT");
    assert!(!unflip_has_vrr, "gamma has priority over the VRR clear");
    assert!(
        committed_after_gamma_unflip,
        "the deferred clear remains committed"
    );
    assert!(
        reason_retained,
        "the direct exit reason survives until the clear"
    );
    assert!(
        scene_has_vrr_clear,
        "the next scene carries VRR_ENABLED = 0"
    );
    assert!(
        logs.lines().any(|line| {
            line == format!(
                "kms: vrr off output={output_name} crtc={} reason=ineligible_authoritative_root_present",
                u32::from(crtc)
            )
        }),
        "scene transition log must use the retained direct exit reason; logs={logs:?}"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: append VRR_ENABLED in the same frame that carries a pending GAMMA_LUT.
fn vrr_waits_behind_pending_gamma() {
    let Some((fixture, mut direct)) = direct_fixture(18_007, 0) else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    let fd = super::direct_unflip::fd(&fixture);
    let recorder = fixture.events.recorders[&fd].clone();
    {
        let output = &mut fixture.events.backend.platform.outputs[0].output;
        output.crtc_gamma_lut_prop = Some(handle(70));
        output.crtc_degamma_lut_prop = Some(handle(71));
        output.crtc_ctm_prop = Some(handle(72));
        output.crtc_gamma_lut_size = Some(4);
    }
    let output_key = fixture.events.backend.platform.outputs[0].key.clone();
    let crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    fixture
        .events
        .backend
        .pstate
        .crtc_key_by_id
        .insert(u32::from(crtc), output_key);
    recorder.set_gamma_size_for_tests(4);
    let first = accepted_direct_record(&mut fixture, &direct, fd);
    let first_id = super::support::accepted_event_id(&first).expect("direct event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &first, first_id, fd);
    let first_present_id = direct.candidate.present_id;
    super::direct_unflip::drain_present_notifications(&mut fixture, &[first_present_id], &[]);

    let vrr_output = &mut fixture.events.backend.platform.outputs[0].output;
    vrr_output.crtc_vrr_enabled_prop = Some(handle(VRR_PROP_ID));
    vrr_output.connector_vrr_capable = true;
    vrr_output.vrr_committed.set(false);
    let crtc_xid = u32::from(vrr_output.crtc);
    Backend::set_crtc_gamma(
        &mut fixture.events.backend,
        crtc_xid,
        &[0x1111; 4],
        &[0x2222; 4],
        &[0x3333; 4],
    )
    .expect("queue gamma through RandR while direct scanout is active");
    assert!(
        fixture.events.backend.platform.outputs[0]
            .output
            .pending_gamma_lut
            .borrow()
            .is_some()
    );
    let gamma_and_vrr_due = submit_direct_successor(&mut fixture, &mut direct, fd);
    assert!(
        gamma_and_vrr_due
            .properties
            .iter()
            .any(|update| { update.object == crtc.into() && update.property == handle(70) }),
        "the due LUT rides the first eligible direct vsync frame"
    );
    assert!(!has_vrr_property(&gamma_and_vrr_due, crtc));
    assert!(
        !fixture.events.backend.platform.outputs[0]
            .output
            .vrr_committed
            .get()
    );
    let gamma_frame_id =
        super::support::accepted_event_id(&gamma_and_vrr_due).expect("gamma frame event ID");
    super::direct_unflip::complete_direct_initial(
        &mut fixture,
        &gamma_and_vrr_due,
        gamma_frame_id,
        fd,
    );
    let gamma_present_id = direct.candidate.present_id;
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[gamma_present_id],
        &[first_present_id],
    );

    let vrr_frame = submit_direct_successor(&mut fixture, &mut direct, fd);
    assert!(contains_vrr_property(&vrr_frame, crtc, 1));
    assert!(
        !vrr_frame
            .properties
            .iter()
            .any(|update| { update.object == crtc.into() && update.property == handle(70) }),
        "VRR follows the gamma frame"
    );
    assert!(
        fixture.events.backend.platform.outputs[0]
            .output
            .vrr_committed
            .get()
    );
    let vrr_id = super::support::accepted_event_id(&vrr_frame).expect("VRR frame event ID");
    super::direct_unflip::complete_direct_initial(&mut fixture, &vrr_frame, vrr_id, fd);
    super::direct_unflip::drain_present_notifications(
        &mut fixture,
        &[direct.candidate.present_id],
        &[gamma_present_id],
    );
    finish_direct_scanout(&mut fixture, &direct, fd);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit the inherited VRR_ENABLED = 0 clear from the next scene commit.
fn vrr_inherited_on_is_cleared() {
    let Some(mut fixture) = super::recording_scene::RecordingSceneFixture::new(1)
        .expect("recording scene fixture construction")
    else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let output = &mut fixture.events.backend.platform.outputs[0].output;
    output.crtc_vrr_enabled_prop = Some(handle(VRR_PROP_ID));
    output.connector_vrr_capable = false;
    output.vrr_committed.set(true);
    let crtc = output.crtc;
    let frame = super::atomic_events::submit_damaged_frame(&mut fixture);
    let frame = frame
        .iter()
        .find(|(_, record)| super::support::accepted_event_id(record).is_some())
        .expect("scene commit accepted")
        .1
        .clone();
    assert!(contains_vrr_property(&frame, crtc, 0));
    assert!(
        !fixture.events.backend.platform.outputs[0]
            .output
            .vrr_committed
            .get()
    );
    let id = super::support::accepted_event_id(&frame).expect("scene event ID");
    super::atomic_events::complete_recorded_attempt(&mut fixture, frame.device_fd, &frame, id, 71);
    fixture.finish_after_normal_drain();
}
