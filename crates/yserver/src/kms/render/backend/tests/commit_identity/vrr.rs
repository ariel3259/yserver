use crate::{
    drm::atomic::{
        AtomicCommitKind, AtomicCommitSink,
        tests_support::{AtomicCommitRecord, RecordingAtomicCommitExecutor},
    },
    kms::{
        render::{
            backend::KmsBackend,
            platform::{ConnectorSnapshot, KmsDevice, PlatformBackend, RenderDeviceId},
        },
        scanout_route::{RenderKmsRelationship, ScanoutRoute},
    },
};
use std::{io, rc::Rc};
use yserver_core::backend::{Backend, ModeSpec};

const VRR_PROP_ID: u32 = 90;

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
// Mutation: drop the same-CRTC VRR state carry-over in connector replacement.
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

    apply_modeset_replacement(backend, 59, crtc);

    let output = &backend.platform.outputs[0].output;
    assert_eq!(
        u32::from(output.crtc),
        crtc,
        "the replacement keeps its CRTC"
    );
    assert!(
        output.vrr_committed.get(),
        "the kernel-held value carries over"
    );
    assert!(output.vrr_disabled.get(), "the EINVAL latch carries over");

    output.vrr_committed.set(true);
    output.vrr_disabled.set(true);
    apply_modeset_replacement(backend, 58, crtc + 1);
    let output = &backend.platform.outputs[0].output;
    assert_eq!(
        u32::from(output.crtc),
        crtc + 1,
        "the replacement uses a new CRTC"
    );
    assert!(
        !output.vrr_committed.get(),
        "a new CRTC starts with kernel state 0"
    );
    assert!(
        !output.vrr_disabled.get(),
        "a new CRTC starts without an EINVAL latch"
    );
    fixture.finish_after_normal_drain();
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
// Mutations: write VRR_ENABLED = 0 unconditionally, or add one extra frame commit.
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
    let expected: Vec<(AtomicCommitKind, Vec<(String, String)>)> = vec![
        (
            AtomicCommitKind::Scene,
            vec![
                ("1".to_owned(), "property::Handle(71)".to_owned()),
                ("1".to_owned(), "property::Handle(72)".to_owned()),
                ("1".to_owned(), "property::Handle(73)".to_owned()),
                ("1".to_owned(), "property::Handle(74)".to_owned()),
                ("1".to_owned(), "property::Handle(75)".to_owned()),
                ("1".to_owned(), "property::Handle(76)".to_owned()),
                ("1".to_owned(), "property::Handle(77)".to_owned()),
                ("1".to_owned(), "property::Handle(78)".to_owned()),
                ("1".to_owned(), "property::Handle(79)".to_owned()),
                ("1".to_owned(), "property::Handle(80)".to_owned()),
            ],
        ),
        (
            AtomicCommitKind::Scene,
            vec![
                ("1".to_owned(), "property::Handle(71)".to_owned()),
                ("1".to_owned(), "property::Handle(72)".to_owned()),
                ("1".to_owned(), "property::Handle(73)".to_owned()),
                ("1".to_owned(), "property::Handle(74)".to_owned()),
                ("1".to_owned(), "property::Handle(75)".to_owned()),
                ("1".to_owned(), "property::Handle(76)".to_owned()),
                ("1".to_owned(), "property::Handle(77)".to_owned()),
                ("1".to_owned(), "property::Handle(78)".to_owned()),
                ("1".to_owned(), "property::Handle(79)".to_owned()),
                ("1".to_owned(), "property::Handle(80)".to_owned()),
            ],
        ),
        (
            AtomicCommitKind::Scene,
            vec![
                ("1".to_owned(), "property::Handle(71)".to_owned()),
                ("1".to_owned(), "property::Handle(72)".to_owned()),
                ("1".to_owned(), "property::Handle(73)".to_owned()),
                ("1".to_owned(), "property::Handle(74)".to_owned()),
                ("1".to_owned(), "property::Handle(75)".to_owned()),
                ("1".to_owned(), "property::Handle(76)".to_owned()),
                ("1".to_owned(), "property::Handle(77)".to_owned()),
                ("1".to_owned(), "property::Handle(78)".to_owned()),
                ("1".to_owned(), "property::Handle(79)".to_owned()),
                ("1".to_owned(), "property::Handle(80)".to_owned()),
                ("1".to_owned(), "property::Handle(81)".to_owned()),
            ],
        ),
        (
            AtomicCommitKind::Scene,
            vec![
                ("1".to_owned(), "property::Handle(71)".to_owned()),
                ("1".to_owned(), "property::Handle(72)".to_owned()),
                ("1".to_owned(), "property::Handle(81)".to_owned()),
                ("1".to_owned(), "property::Handle(82)".to_owned()),
            ],
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
        assert!(
            recorder.records().iter().all(|record| {
                !record.properties.iter().any(|update| {
                    update.object == crtc.into() && update.property == handle(VRR_PROP_ID)
                })
            }),
            "{label} emits no VRR property"
        );
        let signature: Vec<_> = recorder
            .records()
            .into_iter()
            .map(|record| {
                (
                    record.kind,
                    record
                        .properties
                        .iter()
                        .map(|update| {
                            (
                                format!("{:?}", update.object),
                                format!("{:?}", update.property),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            signature, expected,
            "{label} changes the pre-VRR commit stream"
        );
        super::atomic_events::complete_recorded_attempt(
            &mut fixture,
            fd,
            &scene,
            super::support::accepted_event_id(&scene).unwrap(),
            62,
        );
        fixture.finish_after_normal_drain();
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
