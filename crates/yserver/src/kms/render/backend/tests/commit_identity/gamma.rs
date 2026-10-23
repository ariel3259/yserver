use super::support::{AtomicEventFixture, accepted_event_id};
use crate::{
    drm::atomic::{
        AtomicCommitKind,
        tests_support::{AtomicCommitRecord, RecordingAtomicCommitExecutor, ScriptedResult},
    },
    kms::render::backend::KmsBackend,
};
use std::{
    io,
    os::fd::{AsFd as _, AsRawFd as _, RawFd},
    rc::Rc,
    time::Duration,
};
use yserver_core::backend::{Backend, ModeSpec};

fn handle<H: ::drm::control::ResourceHandle>(value: u32) -> H {
    ::drm::control::from_u32(value).expect("nonzero gamma test handle")
}

fn enable_gamma(
    backend: &mut KmsBackend,
    recorder: &RecordingAtomicCommitExecutor,
    output_idx: usize,
    gamma_size: u16,
) -> (u32, ::drm::control::crtc::Handle) {
    let output = &mut backend.platform.outputs[output_idx].output;
    output.crtc_gamma_lut_prop = Some(handle(70));
    output.crtc_degamma_lut_prop = Some(handle(71));
    output.crtc_ctm_prop = Some(handle(72));
    if output.plane_in_fence_fd_prop.is_none() {
        output.plane_in_fence_fd_prop = Some(handle(73));
    }
    if output.crtc_out_fence_ptr_prop.is_none() {
        output.crtc_out_fence_ptr_prop = Some(handle(74));
    }
    let crtc = output.crtc;
    let output_key = backend.platform.outputs[output_idx].key.clone();
    let crtc_xid = u32::from(crtc);
    backend.pstate.crtc_key_by_id.insert(crtc_xid, output_key);
    recorder.set_gamma_size_for_tests(gamma_size);
    (crtc_xid, crtc)
}

fn submit_scene_flip(
    backend: &KmsBackend,
    timing: crate::drm::async_flip::FlipTiming,
) -> io::Result<u64> {
    let layout = &backend.platform.outputs[0];
    let device = backend
        .platform
        .device_for_output(&layout.key)
        .expect("test output has a KMS device");
    let mut out_fence = -1;
    crate::drm::page_flip::submit_scene_flip_with_fences(
        &device.device,
        &device.atomic_commit_sink,
        &layout.output,
        handle(0x900),
        -1,
        &mut out_fence,
        &[],
        timing,
    )
}

fn record_for_id(records: &[AtomicCommitRecord], id: u64) -> AtomicCommitRecord {
    records
        .iter()
        .find(|record| record.user_data == id)
        .expect("commit identity belongs to a recorded request")
        .clone()
}

fn contains_crtc_property(
    record: &AtomicCommitRecord,
    crtc: ::drm::control::crtc::Handle,
    property: ::drm::control::property::Handle,
) -> bool {
    record
        .properties
        .iter()
        .any(|update| update.object == crtc.into() && update.property == property)
}

fn complete_recorded_attempt(
    fixture: &mut AtomicEventFixture,
    fd: RawFd,
    record: &AtomicCommitRecord,
    frame: u32,
) {
    let id = accepted_event_id(record).expect("accepted request carries a commit identity");
    for event in &record.events {
        fixture.queue_commit_completion(
            fd,
            event.crtc,
            id,
            frame,
            Duration::from_millis(u64::from(frame) * 10),
        );
    }
    Backend::on_page_flip_ready(&mut fixture.backend, &mut fixture.state, fd);
}

fn configure_atomic_event_gamma(
    fixture: &mut AtomicEventFixture,
) -> (
    u32,
    ::drm::control::crtc::Handle,
    RawFd,
    Rc<RecordingAtomicCommitExecutor>,
) {
    let fd = fixture.backend.platform.devices[0]
        .device
        .as_fd()
        .as_raw_fd();
    let recorder = fixture.recorders[&fd].clone();
    let (crtc_xid, crtc) = enable_gamma(&mut fixture.backend, &recorder, 0, 4);
    (crtc_xid, crtc, fd, recorder)
}

#[test]
// Mutation: drop the pending LUT when the standalone GammaOnly commit returns EBUSY.
fn gamma_standalone_ebusy_retries_after_the_next_crtc_event() {
    let mut fixture = AtomicEventFixture::new(1);
    let (crtc_xid, crtc, fd, recorder) = configure_atomic_event_gamma(&mut fixture);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    let red = vec![0x1111; 4];
    let green = vec![0x2222; 4];
    let blue = vec![0x3333; 4];
    Backend::set_crtc_gamma(&mut fixture.backend, crtc_xid, &red, &green, &blue)
        .expect("EBUSY leaves the cached gamma pending");
    assert_eq!(
        recorder.records()[0].kind,
        AtomicCommitKind::GammaOnly,
        "idle gamma uses the event-producing standalone stream"
    );
    assert!(matches!(
        recorder.records()[0].result,
        ScriptedResult::Error {
            raw_os_error: Some(error),
            ..
        } if error == libc::EBUSY
    ));
    assert!(
        fixture.backend.platform.outputs[0]
            .output
            .pending_gamma_lut
            .borrow()
            .is_some()
    );

    // An async frame cannot carry the pending CRTC property; its event frees
    // the stream and triggers the retained standalone gamma retry.
    let async_id = submit_scene_flip(&fixture.backend, crate::drm::async_flip::FlipTiming::Async)
        .expect("submit async frame after rejected standalone attempt");
    let async_record = record_for_id(&recorder.records(), async_id);
    assert_eq!(async_record.kind, AtomicCommitKind::Async);
    assert!(!contains_crtc_property(&async_record, crtc, handle(70)));
    complete_recorded_attempt(&mut fixture, fd, &async_record, 1);

    let records = recorder.records();
    assert_eq!(
        records.len(),
        3,
        "gamma retries once after the next CRTC event"
    );
    let retry = &records[2];
    assert_eq!(retry.kind, AtomicCommitKind::GammaOnly);
    assert!(accepted_event_id(retry).is_some());
    assert!(contains_crtc_property(retry, crtc, handle(70)));
    assert!(!contains_crtc_property(&records[1], crtc, handle(70)));
    assert!(
        fixture.backend.platform.outputs[0]
            .output
            .pending_gamma_lut
            .borrow()
            .is_none()
    );
    assert_eq!(fixture.backend.get_crtc_gamma(crtc_xid), (red, green, blue));
}

#[test]
// Mutation: drop the per-CRTC GAMMA_LUT disable latch after EINVAL.
fn gamma_einval_latches_crtc_off_and_resubmits_the_frame() {
    let mut fixture = AtomicEventFixture::new(1);
    let (crtc_xid, crtc, fd, recorder) = configure_atomic_event_gamma(&mut fixture);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    let red = vec![0x1010; 4];
    let green = vec![0x2020; 4];
    let blue = vec![0x3030; 4];
    Backend::set_crtc_gamma(&mut fixture.backend, crtc_xid, &red, &green, &blue)
        .expect("script the gamma ramp as pending behind EBUSY");
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Ok(()));

    let frame_id = submit_scene_flip(&fixture.backend, crate::drm::async_flip::FlipTiming::Vsync)
        .expect("the frame is accepted after retry without GAMMA_LUT");
    let records = recorder.records();
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].kind, AtomicCommitKind::Scene);
    assert_ne!(records[1].result, ScriptedResult::Ok);
    assert!(contains_crtc_property(&records[1], crtc, handle(70)));
    let accepted = record_for_id(&records, frame_id);
    assert_eq!(accepted.kind, AtomicCommitKind::Scene);
    assert_eq!(accepted.result, ScriptedResult::Ok);
    assert!(!contains_crtc_property(&accepted, crtc, handle(70)));
    assert!(accepted.properties.iter().any(|update| {
        update.object == fixture.backend.platform.outputs[0].output.plane.into()
            && update.property == fixture.backend.platform.outputs[0].output.plane_fb_id_prop
    }));
    assert_eq!(accepted_event_id(&accepted), Some(frame_id));
    assert_eq!(
        fixture.backend.platform.devices[0]
            .atomic_commit_sink
            .for_tests_pending_page_flip_commit_ids(
                &fixture.backend.platform.devices[0].device,
                crtc,
            ),
        [frame_id],
        "the retried frame owns its normal CRTC event association"
    );
    assert!(
        fixture.backend.platform.outputs[0]
            .output
            .gamma_lut_disabled
            .get()
    );
    assert!(
        fixture.backend.platform.outputs[0]
            .output
            .pending_gamma_lut
            .borrow()
            .is_none()
    );
    assert_eq!(
        fixture.backend.get_crtc_gamma(crtc_xid),
        (red.clone(), green.clone(), blue.clone())
    );

    let before = recorder.records().len();
    Backend::set_crtc_gamma(&mut fixture.backend, crtc_xid, &red, &green, &blue)
        .expect("RandR cache stays writable after the per-CRTC capability latch");
    assert_eq!(recorder.records().len(), before);

    complete_recorded_attempt(&mut fixture, fd, &accepted, 2);
}

fn enable_recording_scene_gamma(
    fixture: &mut super::recording_scene::RecordingSceneFixture,
) -> (
    u32,
    ::drm::control::crtc::Handle,
    RawFd,
    Rc<RecordingAtomicCommitExecutor>,
) {
    let fd = fixture.kms_fd(0);
    let recorder = fixture.events.recorders[&fd].clone();
    let (crtc_xid, crtc) = enable_gamma(&mut fixture.events.backend, &recorder, 0, 4);
    (crtc_xid, crtc, fd, recorder)
}

struct LogicalVtResume;

impl crate::kms::render::backend::kms::session::VtResumeExecutor for LogicalVtResume {
    fn acquire_master(&self, _device: &crate::kms::render::platform::KmsDevice) -> io::Result<()> {
        Ok(())
    }

    fn release_master(&self, _device: &crate::kms::render::platform::KmsDevice) -> io::Result<()> {
        Ok(())
    }

    fn probe_connector_snapshot(
        &self,
        platform: &crate::kms::render::platform::PlatformBackend,
    ) -> io::Result<Vec<crate::kms::render::platform::ConnectorSnapshot>> {
        Ok(platform
            .outputs
            .iter()
            .map(|output| crate::kms::render::platform::ConnectorSnapshot {
                key: output.key.clone(),
                modes: vec![output.output.picked.clone()],
                mm_width: output.output.mm_width,
                mm_height: output.output.mm_height,
                edid: output.output.edid.clone(),
                connector_type: output.output.connector_type.clone(),
            })
            .collect())
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: let a scene flip through while the standalone GAMMA_LUT event is pending.
fn p1_gamma_standalone_serializes_scene_until_its_event() {
    let Some(mut fixture) = super::recording_scene::RecordingSceneFixture::new(1)
        .expect("recording scene fixture construction")
    else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let (crtc_xid, crtc, fd, recorder) = enable_recording_scene_gamma(&mut fixture);
    let initial = super::atomic_events::submit_damaged_frame(&mut fixture);
    complete_recorded_attempt(&mut fixture.events, fd, &initial[0].1, 0);
    recorder.clear_records();
    Backend::set_crtc_gamma(
        &mut fixture.events.backend,
        crtc_xid,
        &[0x1111; 4],
        &[0x2222; 4],
        &[0x3333; 4],
    )
    .expect("submit standalone gamma through Backend");
    let gamma = recorder.records()[0].clone();
    assert_eq!(gamma.kind, AtomicCommitKind::GammaOnly);
    assert!(gamma.events[0].crtc == crtc);
    let gamma_id = accepted_event_id(&gamma).expect("standalone gamma carries a commit identity");
    assert!(
        !fixture
            .events
            .backend
            .scene
            .output_ready_for_submit(0, &fixture.events.backend.platform)
    );

    let root = fixture.events.backend.core.window_id;
    let (width, height) = (
        fixture.events.backend.platform.fb_w,
        fixture.events.backend.platform.fb_h,
    );
    Backend::clear_area(
        &mut fixture.events.backend,
        None,
        root,
        0,
        None,
        0,
        0,
        width,
        height,
        (0, 0),
    )
    .expect("queue visible scene damage through Backend");
    Backend::maybe_composite(&mut fixture.events.backend).expect("compose while gamma is pending");
    assert_eq!(
        recorder.records().len(),
        1,
        "scene submit waits for gamma event"
    );
    assert!(!fixture.events.backend.scene.has_pending_page_flips());

    fixture
        .events
        .queue_commit_completion(fd, crtc, gamma_id, 1, Duration::from_millis(10));
    Backend::on_page_flip_ready(&mut fixture.events.backend, &mut fixture.events.state, fd);
    let scene_attempts = super::atomic_events::submit_damaged_frame(&mut fixture);
    let scene = scene_attempts
        .iter()
        .find(|(_, record)| {
            record.kind == AtomicCommitKind::Scene && accepted_event_id(record).is_some()
        })
        .expect("scene damage submits after the gamma event");
    assert!(!contains_crtc_property(&scene.1, crtc, handle(70)));
    complete_recorded_attempt(&mut fixture.events, fd, &scene.1, 2);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: append the pending CRTC property to an async frame instead of waiting for vsync.
fn p1_gamma_async_flip_defers_lut_to_the_next_vsync_commit() {
    let Some(mut fixture) = super::recording_scene::RecordingSceneFixture::new(1)
        .expect("recording scene fixture construction")
    else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let (crtc_xid, crtc, fd, recorder) = enable_recording_scene_gamma(&mut fixture);
    let first_async_id = submit_scene_flip(
        &fixture.events.backend,
        crate::drm::async_flip::FlipTiming::Async,
    )
    .expect("submit first async frame");
    let first_async = record_for_id(&recorder.records(), first_async_id);
    assert_eq!(first_async.kind, AtomicCommitKind::Async);

    let red = vec![0x1111; 4];
    let green = vec![0x2222; 4];
    let blue = vec![0x3333; 4];
    Backend::set_crtc_gamma(&mut fixture.events.backend, crtc_xid, &red, &green, &blue)
        .expect("retain gamma while the async frame is in flight");
    assert!(
        fixture.events.backend.platform.outputs[0]
            .output
            .pending_gamma_lut
            .borrow()
            .is_some()
    );
    let root = fixture.events.backend.core.window_id;
    let (width, height) = (
        fixture.events.backend.platform.fb_w,
        fixture.events.backend.platform.fb_h,
    );
    Backend::clear_area(
        &mut fixture.events.backend,
        None,
        root,
        0,
        None,
        0,
        0,
        width,
        height,
        (0, 0),
    )
    .expect("queue scene damage so the LUT can wait for a frame");
    complete_recorded_attempt(&mut fixture.events, fd, &first_async, 1);
    assert_eq!(
        recorder.records().len(),
        1,
        "async retirement does not add a CRTC commit while scene work is queued"
    );

    let async_id = submit_scene_flip(
        &fixture.events.backend,
        crate::drm::async_flip::FlipTiming::Async,
    )
    .expect("submit the next async frame");
    let async_record = record_for_id(&recorder.records(), async_id);
    assert_eq!(async_record.kind, AtomicCommitKind::Async);
    assert!(!contains_crtc_property(&async_record, crtc, handle(70)));
    complete_recorded_attempt(&mut fixture.events, fd, &async_record, 2);
    assert!(
        fixture.events.backend.platform.outputs[0]
            .output
            .pending_gamma_lut
            .borrow()
            .is_some()
    );

    let vsync_id = submit_scene_flip(
        &fixture.events.backend,
        crate::drm::async_flip::FlipTiming::Vsync,
    )
    .expect("submit the next vsync frame with gamma");
    let vsync = record_for_id(&recorder.records(), vsync_id);
    assert_eq!(vsync.kind, AtomicCommitKind::Scene);
    assert!(contains_crtc_property(&vsync, crtc, handle(70)));
    assert!(contains_crtc_property(&vsync, crtc, handle(71)));
    assert!(contains_crtc_property(&vsync, crtc, handle(72)));
    assert!(
        fixture.events.backend.platform.outputs[0]
            .output
            .pending_gamma_lut
            .borrow()
            .is_none()
    );
    complete_recorded_attempt(&mut fixture.events, fd, &vsync, 3);
    let scene_attempts = super::atomic_events::submit_damaged_frame(&mut fixture);
    let scene = scene_attempts
        .iter()
        .find(|(_, record)| {
            record.kind == AtomicCommitKind::Scene && accepted_event_id(record).is_some()
        })
        .expect("queued scene damage submits after the gamma-carrying vsync");
    complete_recorded_attempt(&mut fixture.events, fd, &scene.1, 4);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: make the shared gamma reapply helper a no-op after modeset and VT acquire.
fn p1_gamma_reapplies_after_modeset_and_vt_resume() {
    let Some(mut fixture) = super::recording_scene::RecordingSceneFixture::new(1)
        .expect("recording scene fixture construction")
    else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    fixture
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture
        .events
        .backend
        .for_tests_set_vt_resume_executor(Rc::new(LogicalVtResume));
    let (crtc_xid, crtc, fd, recorder) = enable_recording_scene_gamma(&mut fixture);

    let initial = super::atomic_events::submit_damaged_frame(&mut fixture);
    let initial_scene = initial
        .iter()
        .find(|(_, record)| {
            record.kind == AtomicCommitKind::Scene && accepted_event_id(record).is_some()
        })
        .expect("initial scene frame is accepted");
    complete_recorded_attempt(&mut fixture.events, fd, &initial_scene.1, 1);
    recorder.clear_records();

    let red = vec![0x1111; 4];
    let green = vec![0x2222; 4];
    let blue = vec![0x3333; 4];
    Backend::set_crtc_gamma(&mut fixture.events.backend, crtc_xid, &red, &green, &blue)
        .expect("set cached gamma through Backend");
    let gamma = recorder
        .records()
        .into_iter()
        .find(|record| record.kind == AtomicCommitKind::GammaOnly)
        .expect("idle gamma is applied through the standalone commit");
    complete_recorded_attempt(&mut fixture.events, fd, &gamma, 2);

    let (mode, connector, output_id, output_key, x, y) = {
        let backend = &mut fixture.events.backend;
        let layout = &backend.platform.outputs[0];
        let mode = ModeSpec {
            width: layout.width,
            height: layout.height,
            vrefresh: layout.output.picked.vrefresh.saturating_sub(1),
        };
        let connector = layout.key.connector_name.clone();
        let output_key = layout.key.clone();
        let (x, y) = (layout.x, layout.y);
        let output_id = Backend::randr_outputs_and_modes(backend)
            .0
            .into_iter()
            .find(|output| output.name == connector)
            .expect("output has a RandR id")
            .output_id;
        (mode, connector, output_id, output_key, x, y)
    };
    let mut discovered = super::super::randr::clone_test_drm_output(
        &fixture.events.backend.platform.outputs[0].output,
    );
    let changed_mode =
        super::super::test_advertised_mode(mode.width, mode.height, mode.vrefresh, false);
    // The synthetic DRM device cannot answer get_connector; pre-resolve the
    // overridden output's advertised mode just as discovery would.
    discovered.picked = changed_mode.clone();
    discovered.modes.push(changed_mode);
    fixture.events.backend.pstate.crtc_config_discovery_override = Some(discovered);
    fixture
        .events
        .backend
        .initialize_provider_output_sources()
        .expect("authorize the live test output's KMS provider");
    let mode_offset = recorder.records().len();
    assert!(
        Backend::apply_crtc_config(
            &mut fixture.events.backend,
            output_id,
            &connector,
            Some(mode),
            x,
            y,
        )
        .expect("apply a real mode change through Backend")
    );

    let mode_records = recorder.records();
    let mode_gamma = mode_records
        .iter()
        .skip(mode_offset)
        .find(|record| contains_crtc_property(record, crtc, handle(70)))
        .cloned();
    assert!(
        mode_gamma.is_some(),
        "modeset reapply reaches the atomic stream"
    );
    if let Some(record) = mode_gamma
        .as_ref()
        .filter(|record| accepted_event_id(record).is_some())
    {
        complete_recorded_attempt(&mut fixture.events, fd, record, 3);
    }
    let mode_scene = super::atomic_events::submit_damaged_frame(&mut fixture);
    let accepted_mode_scene = mode_scene
        .iter()
        .find(|(_, record)| {
            record.kind == AtomicCommitKind::Scene && accepted_event_id(record).is_some()
        })
        .expect("modeset damage reaches a scene commit");
    complete_recorded_attempt(&mut fixture.events, fd, &accepted_mode_scene.1, 4);

    let _ = Backend::begin_vt_release(&mut fixture.events.backend);
    Backend::finish_vt_release(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        &yserver_core::core_loop::input_inventory::InputInventory::default(),
    );
    let resume_offset = recorder.records().len();
    Backend::on_vt_acquire(&mut fixture.events.backend, &mut fixture.events.state);
    let resume_records = recorder.records();
    let resume_gamma = resume_records
        .iter()
        .skip(resume_offset)
        .find(|record| contains_crtc_property(record, crtc, handle(70)))
        .cloned();
    if let Some(record) = resume_gamma
        .as_ref()
        .filter(|record| accepted_event_id(record).is_some())
    {
        complete_recorded_attempt(&mut fixture.events, fd, record, 5);
    }
    let resumed_scene = super::atomic_events::submit_damaged_frame(&mut fixture);
    let accepted_resumed_scene = resumed_scene
        .iter()
        .find(|(_, record)| {
            record.kind == AtomicCommitKind::Scene && accepted_event_id(record).is_some()
        })
        .expect("resume damage reaches a scene commit");
    let resumed_scene_has_gamma =
        contains_crtc_property(&accepted_resumed_scene.1, crtc, handle(70));
    assert!(
        resume_gamma.is_some() || resumed_scene_has_gamma,
        "VT reapply reaches either the idle CRTC commit or the next scene vsync"
    );
    let resumed_crtc_xid = Backend::randr_outputs_and_modes(&mut fixture.events.backend)
        .0
        .into_iter()
        .find(|output| output.name == connector)
        .expect("resumed output has a RandR CRTC")
        .crtc_id;
    assert_eq!(
        fixture.events.backend.get_crtc_gamma(resumed_crtc_xid),
        (red, green, blue)
    );
    complete_recorded_attempt(&mut fixture.events, fd, &accepted_resumed_scene.1, 6);
    assert_eq!(fixture.events.backend.platform.outputs[0].key, output_key);
    fixture.finish_after_normal_drain();
}
