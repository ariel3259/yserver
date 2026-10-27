//! Direct-frame and unflip retirement by accepted atomic commit identity.

use super::{
    atomic_events::{complete_recorded_attempt, pending_ids},
    recording_scene::{DirectPresentSetup, P1SceneRoute, RecordingSceneFixture},
    support::accepted_event_id,
};
use crate::drm::atomic::tests_support::{AtomicCommitRecord, ScriptedResult};
use std::{
    collections::{HashMap, HashSet},
    os::fd::RawFd,
    time::{Duration, Instant},
};
use yserver_core::backend::Backend;

const DIRECT_PROBATION: usize = 8;
const WAIT_DEADLINE: Duration = Duration::from_secs(5);

pub(crate) fn direct_fixture(
    output_count: usize,
    route: P1SceneRoute,
    present_id: u64,
    options: u32,
) -> Option<(RecordingSceneFixture, DirectPresentSetup)> {
    let mut fixture = RecordingSceneFixture::with_route(output_count, route)
        .expect("direct recording scene fixture construction")?;
    let direct = fixture.install_direct_present(present_id, options);
    Some((fixture, direct))
}

pub(crate) fn fd(fixture: &RecordingSceneFixture) -> RawFd {
    fixture.kms_fd(0)
}

pub(crate) fn record_count(fixture: &RecordingSceneFixture, device_fd: RawFd) -> usize {
    fixture.events.recorders[&device_fd].records().len()
}

pub(crate) fn records_since(
    fixture: &RecordingSceneFixture,
    device_fd: RawFd,
    offset: usize,
) -> Vec<AtomicCommitRecord> {
    fixture.events.recorders[&device_fd]
        .records()
        .into_iter()
        .skip(offset)
        .filter(|record| record.device_fd == device_fd)
        .collect()
}

pub(super) fn accepted_records(records: &[AtomicCommitRecord]) -> Vec<AtomicCommitRecord> {
    records
        .iter()
        .filter(|record| accepted_event_id(record).is_some())
        .cloned()
        .collect()
}

fn admit_direct(fixture: &mut RecordingSceneFixture, direct: &DirectPresentSetup) {
    for attempt in 0..DIRECT_PROBATION {
        let accepted = Backend::try_present_direct(
            &mut fixture.events.backend,
            direct.candidate,
            direct.event.clone(),
        )
        .expect("direct candidate through Backend");
        assert_eq!(
            accepted,
            attempt + 1 == DIRECT_PROBATION,
            "the existing direct probation must be driven by Backend entries"
        );
    }
}

pub(super) fn enter_async_tearing(
    fixture: &mut RecordingSceneFixture,
    direct: &DirectPresentSetup,
) {
    let device = &mut fixture.events.backend.platform.devices[0];
    device.async_page_flip = true;
    let start = record_count(fixture, fd(fixture));
    super::super::tearing::submit_initial_tearing_frame(&mut fixture.events.backend);
    let initial = records_since(fixture, fd(fixture), start)
        .into_iter()
        .find(|record| accepted_event_id(record).is_some())
        .expect("initial tearing scene frame accepted");
    let initial_id = accepted_event_id(&initial).expect("accepted initial frame has an ID");
    complete_recorded_attempt(fixture, fd(fixture), &initial, initial_id, 41);
    super::super::tearing::finish_entering_tearing(
        &mut fixture.events.backend,
        direct.target_xid,
        direct.target_xid,
        direct.target_xid,
        direct.candidate.present_id + 100,
    );
    Backend::set_cursor_hidden(&mut fixture.events.backend, true);
}

pub(super) fn submit_direct(
    fixture: &mut RecordingSceneFixture,
    direct: &DirectPresentSetup,
    device_fd: RawFd,
) -> Vec<AtomicCommitRecord> {
    let start = record_count(fixture, device_fd);
    admit_direct(fixture, direct);
    records_since(fixture, device_fd, start)
}

pub(super) fn inject_output(
    fixture: &mut RecordingSceneFixture,
    device_fd: RawFd,
    record: &AtomicCommitRecord,
    output_idx: usize,
    event_id: u64,
    frame: u32,
    ust: Duration,
) {
    fixture.events.queue_commit_completion(
        device_fd,
        record.events[output_idx].crtc,
        event_id,
        frame,
        ust,
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
}

pub(super) fn request_unflip(fixture: &mut RecordingSceneFixture, direct: &DirectPresentSetup) {
    let mut successor = direct.candidate;
    successor.present_id += 1;
    successor.valid_region_xid = 0x55;
    let mut event = direct.event.clone();
    event.present_id = successor.present_id;
    event.serial = event.serial.wrapping_add(1);
    assert!(
        !Backend::try_present_direct(&mut fixture.events.backend, successor, event)
            .expect("ineligible authoritative-root successor"),
        "the successor must request a composed unflip"
    );
    assert!(fixture.events.backend.pstate.direct.root().unflip_requested);
}

pub(super) fn drain_present_notifications(
    fixture: &mut RecordingSceneFixture,
    expected_completed: &[u64],
    expected_idled: &[u64],
) {
    let completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(
        completed.len(),
        expected_completed.len(),
        "unexpected number of direct Complete events"
    );
    assert_eq!(
        completed
            .iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        expected_completed,
        "direct Complete events must belong to the expected Presents"
    );
    let idled = Backend::drain_retired_present_idle_events(&mut fixture.events.backend);
    assert_eq!(
        idled.len(),
        expected_idled.len(),
        "unexpected number of direct Idle events"
    );
    assert_eq!(
        idled
            .iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        expected_idled,
        "direct Idle events must belong to the expected Presents"
    );
}

pub(crate) fn output_owner_frame(
    fixture: &mut RecordingSceneFixture,
    direct: &DirectPresentSetup,
    output_idx: usize,
    target: crate::kms::render::backend::ScanoutM0Target,
    fallback_override: Option<crate::kms::render::store::DrawableId>,
    paint_host_override: Option<u32>,
) -> crate::kms::render::backend::DirectPresentFrame {
    use crate::kms::render::{backend::DirectPresentFrame, target::PaintTarget};
    use std::collections::HashSet;

    let backend = &mut fixture.events.backend;
    let output = &backend.platform.outputs[output_idx].output;
    let mut candidate = direct.candidate;
    candidate.crtc_id = u32::from(output.crtc);
    candidate.crtc_epoch = backend.present_crtc_clock_epoch(candidate.crtc_id);
    if let Some(paint_host_xid) = paint_host_override {
        candidate.paint_dst_host_xid = paint_host_xid;
    }
    let fallback_id = fallback_override.unwrap_or_else(|| {
        backend
            .store
            .lookup(direct.target_xid)
            .expect("direct setup has a fallback drawable")
    });
    let source_pin = backend.pin_direct_source(direct.source_id);
    let fallback_target_pin = backend.pin_direct_source(fallback_id);
    let mut event = direct.event.clone();
    event.crtc_id = candidate.crtc_id;
    event.crtc_epoch = candidate.crtc_epoch;
    DirectPresentFrame {
        source_pin,
        fallback_target_pin,
        source_id: direct.source_id,
        candidate,
        target,
        present_received_at: Instant::now(),
        present_received_monotonic: None,
        fallback_target: PaintTarget::new(fallback_id, (0, 0), None, 24),
        event,
        completion_output_idx: output_idx,
        completion_clock: None,
        awaiting_outputs: HashSet::new(),
        accepted_event_id: None,
        async_outcome: crate::drm::async_flip::AsyncOutcome::NotRequested,
    }
}

pub(crate) fn seed_output_owner_successor(
    fixture: &mut RecordingSceneFixture,
    direct: &DirectPresentSetup,
    output_idx: usize,
    target: crate::kms::render::backend::ScanoutM0Target,
    fallback_override: Option<crate::kms::render::store::DrawableId>,
    paint_host_override: Option<u32>,
) {
    let frame = output_owner_frame(
        fixture,
        direct,
        output_idx,
        target,
        fallback_override,
        paint_host_override,
    );
    let output = &fixture.events.backend.platform.outputs[output_idx].output;
    let candidate = frame.candidate;
    fixture.events.backend.pstate.scanout_m1.entries.insert(
        (
            direct.source_id,
            crate::kms::render::backend::DirectScope::Output(output_idx),
        ),
        crate::kms::render::backend::ScanoutM1ProbeEntry::accepted_with_shape(
            crate::drm::modeset::DirectScanoutProbeFramebuffer::for_tests(
                super::recording_scene::raw(
                    0x8a00
                        + u32::try_from(candidate.present_id).expect("test Present id fits")
                        + u32::try_from(output_idx).expect("test output index fits"),
                ),
            ),
            crate::drm::async_flip::AsyncShape {
                fourcc: 0x3432_5258,
                modifier: 0,
                pitch: u32::from(candidate.src_width) * 4,
                width: u32::from(candidate.src_width),
                height: u32::from(candidate.src_height),
                crtcs: vec![output.crtc],
                direct: true,
            },
        ),
    );
    let owner = &mut fixture.events.backend.pstate.direct.outputs[output_idx];
    assert!(!owner.active(), "output owner slot starts empty");
    owner.queued_successor = Some(frame);
    owner.hold_direct = true;
}

fn submit_output_owner_successor(
    fixture: &mut RecordingSceneFixture,
    direct: &DirectPresentSetup,
    output_idx: usize,
) -> AtomicCommitRecord {
    let device_fd = fd(fixture);
    let start = record_count(fixture, device_fd);
    seed_output_owner_successor(
        fixture,
        direct,
        output_idx,
        crate::kms::render::backend::ScanoutM0Target::Unredirected,
        None,
        None,
    );
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("submit output owner through the scheduling pass");
    let crtc = fixture.events.backend.platform.outputs[output_idx]
        .output
        .crtc;
    records_since(fixture, device_fd, start)
        .into_iter()
        .find(|record| {
            accepted_event_id(record).is_some()
                && record.events.len() == 1
                && record.events[0].crtc == crtc
        })
        .expect("output owner direct commit was accepted")
}

fn submit_two_output_owner_successors(
    fixture: &mut RecordingSceneFixture,
    first: &DirectPresentSetup,
    second: &DirectPresentSetup,
) -> (AtomicCommitRecord, AtomicCommitRecord) {
    fixture.events.backend.platform.outputs[0]
        .output
        .picked
        .vrefresh = 75;
    fixture.events.backend.platform.outputs[1]
        .output
        .picked
        .vrefresh = 240;
    assert_ne!(
        fixture.events.backend.platform.outputs[1].x, 0,
        "two-output fixtures keep a nonzero second-output origin"
    );
    let device_fd = fd(fixture);
    let start = record_count(fixture, device_fd);
    seed_output_owner_successor(
        fixture,
        first,
        0,
        crate::kms::render::backend::ScanoutM0Target::Unredirected,
        None,
        None,
    );
    seed_output_owner_successor(
        fixture,
        second,
        1,
        crate::kms::render::backend::ScanoutM0Target::Unredirected,
        None,
        None,
    );
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("submit both output owners through the scheduling pass");
    let records = records_since(fixture, device_fd, start);
    let first_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    let second_crtc = fixture.events.backend.platform.outputs[1].output.crtc;
    let first_record = records
        .iter()
        .find(|record| record.events.iter().any(|event| event.crtc == first_crtc))
        .expect("first output direct commit")
        .clone();
    let second_record = records
        .iter()
        .find(|record| record.events.iter().any(|event| event.crtc == second_crtc))
        .expect("second output direct commit")
        .clone();
    (first_record, second_record)
}

fn assert_plane_source_origin(
    fixture: &RecordingSceneFixture,
    record: &AtomicCommitRecord,
    output_idx: usize,
) {
    let output = &fixture.events.backend.platform.outputs[output_idx].output;
    for (property, name) in [
        (output.plane_src_x_prop, "SRC_X"),
        (output.plane_src_y_prop, "SRC_Y"),
    ] {
        let value = record
            .properties
            .iter()
            .find(|update| update.object == output.plane.into() && update.property == property)
            .unwrap_or_else(|| panic!("output {output_idx} commit is missing {name}"))
            .value;
        assert_eq!(
            value, 0,
            "output {output_idx} {name} must start at the pixmap origin"
        );
    }
}

fn assert_atomic_record_touches_output_only(
    fixture: &RecordingSceneFixture,
    record: &AtomicCommitRecord,
    output_idx: usize,
) {
    let output = &fixture.events.backend.platform.outputs[output_idx].output;
    let other = &fixture.events.backend.platform.outputs[1 - output_idx].output;
    assert_eq!(record.events.len(), 1);
    assert_eq!(record.events[0].crtc, output.crtc);
    assert!(record.properties.iter().any(|update| {
        update.property == output.plane_fb_id_prop && update.object == output.plane.into()
    }));
    assert!(record.properties.iter().any(|update| {
        update.property == output.plane_crtc_id_prop && update.object == output.plane.into()
    }));
    let other_plane_properties = [
        other.plane_fb_id_prop,
        other.plane_crtc_id_prop,
        other.plane_src_x_prop,
        other.plane_src_y_prop,
        other.plane_src_w_prop,
        other.plane_src_h_prop,
        other.plane_crtc_x_prop,
        other.plane_crtc_y_prop,
        other.plane_crtc_w_prop,
        other.plane_crtc_h_prop,
    ];
    for property in other_plane_properties {
        assert!(
            !record
                .properties
                .iter()
                .any(|update| update.object == other.plane.into() && update.property == property)
        );
    }
    for property in [
        other.crtc_out_fence_ptr_prop,
        other.crtc_vrr_enabled_prop,
        other.crtc_gamma_lut_prop,
        other.crtc_degamma_lut_prop,
        other.crtc_ctm_prop,
    ]
    .into_iter()
    .flatten()
    {
        assert!(
            !record
                .properties
                .iter()
                .any(|update| update.object == other.crtc.into() && update.property == property)
        );
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: use layout.x/y as an output owner's source origin.
fn output_owner_submits_and_unflips_only_its_crtc() {
    let Some((mut fixture, mut direct)) = direct_fixture(2, P1SceneRoute::Shared, 203, 0) else {
        return;
    };
    // Give output 1 its own-sized source: using its desktop x coordinate as
    // SRC_X would address beyond this 800-pixel framebuffer.
    let output = &fixture.events.backend.platform.outputs[1];
    let width = output.width;
    let height = output.height;
    let source_xid = direct.candidate.src_pixmap_xid + 0x10;
    let source_storage = fixture
        .events
        .backend
        .platform
        .gpu
        .allocate_drawable_storage(width, height, 24)
        .expect("allocate output-sized direct source");
    direct.source_id = fixture
        .events
        .backend
        .store
        .allocate(
            source_xid,
            crate::kms::render::store::DrawableKind::Pixmap,
            24,
            false,
            source_storage,
        )
        .expect("register output-sized direct source");
    Backend::put_image(
        &mut fixture.events.backend,
        None,
        source_xid,
        24,
        1,
        1,
        0,
        0,
        &[0, 0, 0, 0],
    )
    .expect("initialize output-sized direct source through Backend");
    direct.candidate.src_pixmap_xid = source_xid;
    direct.candidate.src_host_xid = source_xid;
    direct.candidate.src_width = width;
    direct.candidate.src_height = height;
    direct.event.host_xid = source_xid;
    fixture.extra_drawables.push(source_xid);
    let output_crtc = fixture.events.backend.platform.outputs[1].output.crtc;
    fixture.events.backend.pstate.scanout_m1.entries.insert(
        (
            direct.source_id,
            crate::kms::render::backend::DirectScope::Output(1),
        ),
        crate::kms::render::backend::ScanoutM1ProbeEntry::accepted_with_shape(
            crate::drm::modeset::DirectScanoutProbeFramebuffer::for_tests(
                super::recording_scene::raw(0x8203),
            ),
            crate::drm::async_flip::AsyncShape {
                fourcc: 0x3432_5258,
                modifier: 0,
                pitch: u32::from(width) * 4,
                width: u32::from(width),
                height: u32::from(height),
                crtcs: vec![output_crtc],
                direct: true,
            },
        ),
    );
    let device_fd = fd(&fixture);
    Backend::set_cursor_hidden(&mut fixture.events.backend, true);
    let direct_record = submit_output_owner_successor(&mut fixture, &direct, 1);
    let direct_id = accepted_event_id(&direct_record).expect("direct submit accepted");
    assert_atomic_record_touches_output_only(&fixture, &direct_record, 1);
    assert_plane_source_origin(&fixture, &direct_record, 1);
    complete_recorded_attempt(&mut fixture, device_fd, &direct_record, direct_id, 53);
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_some()
    );
    assert_eq!(
        fixture.events.backend.pstate.direct.active_scopes(),
        [crate::kms::render::backend::DirectScope::Output(1)]
    );

    Backend::unmap_subwindow(&mut fixture.events.backend, None, direct.target_xid)
        .expect("unmap direct window through Backend");
    let start = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("submit output owner composed unflip through scheduler");
    let crtc = fixture.events.backend.platform.outputs[1].output.crtc;
    let unflip = records_since(&fixture, device_fd, start)
        .into_iter()
        .find(|record| {
            accepted_event_id(record).is_some()
                && record.events.len() == 1
                && record.events[0].crtc == crtc
        })
        .expect("output owner composed-unflip commit was accepted");
    assert_atomic_record_touches_output_only(&fixture, &unflip, 1);
    assert_eq!(
        fixture.events.backend.pstate.direct.outputs[1].unflip_awaiting_outputs,
        HashMap::from([(1, accepted_event_id(&unflip).unwrap())])
    );
}

fn damage_scene_root(fixture: &mut RecordingSceneFixture) {
    let backend = &fixture.events.backend;
    let root = backend.core.window_id;
    let width = backend.platform.fb_w;
    let height = backend.platform.fb_h;
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
    .expect("damage scene root through Backend");
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: restore the root-wide early return; use the normal mask in the fresh-unflip pass.
fn composed_output_flips_while_other_output_owner_pending_and_unflipping() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::Shared, 208, 0) else {
        return;
    };
    let device_fd = fd(&fixture);
    Backend::set_cursor_hidden(&mut fixture.events.backend, true);
    damage_scene_root(&mut fixture);
    let before_pending_pass = record_count(&fixture, device_fd);
    let direct_record = submit_output_owner_successor(&mut fixture, &direct, 1);
    let direct_id = accepted_event_id(&direct_record).expect("direct output commit accepted");

    let pending_records = records_since(&fixture, device_fd, before_pending_pass);
    let output0_scene = pending_records
        .iter()
        .find(|record| {
            record.kind == crate::drm::atomic::AtomicCommitKind::Scene
                && record.events.len() == 1
                && record.events[0].crtc == fixture.events.backend.platform.outputs[0].output.crtc
        })
        .unwrap_or_else(|| panic!("output 0 composed while output 1's direct commit is pending; records={pending_records:?}"));
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .pending
            .is_some()
    );
    complete_recorded_attempt(
        &mut fixture,
        device_fd,
        output0_scene,
        accepted_event_id(output0_scene).unwrap(),
        58,
    );
    complete_recorded_attempt(&mut fixture, device_fd, &direct_record, direct_id, 59);
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_some()
    );

    Backend::unmap_subwindow(&mut fixture.events.backend, None, direct.target_xid)
        .expect("request output 1 unflip through Backend");
    damage_scene_root(&mut fixture);
    let before_fresh_unflip = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("fresh-unflip output 1 while output 0 damage waits");
    let fresh_unflip = records_since(&fixture, device_fd, before_fresh_unflip)
        .into_iter()
        .find(|record| accepted_event_id(record).is_some())
        .expect("fresh composed replacement accepted for output 1");
    let output1_crtc = fixture.events.backend.platform.outputs[1].output.crtc;
    assert_eq!(fresh_unflip.events.len(), 1);
    assert_eq!(fresh_unflip.events[0].crtc, output1_crtc);
    let fresh_id = accepted_event_id(&fresh_unflip).expect("fresh unflip accepted ID");
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(1),
        Some(fresh_id),
        "output 1's deferred scene frame uses the accepted replacement identity"
    );
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        None,
        "output 0 receives no pending acknowledgement from output 1's replacement"
    );
    assert_eq!(
        fixture.events.backend.pstate.direct.outputs[1].unflip_awaiting_outputs,
        HashMap::from([(1, fresh_id)])
    );

    damage_scene_root(&mut fixture);
    let before_next_pass = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("compose output 0 on the pass after output 1's replacement submission");
    let output0_after_unflip = records_since(&fixture, device_fd, before_next_pass)
        .into_iter()
        .find(|record| {
            record.kind == crate::drm::atomic::AtomicCommitKind::Scene
                && record.events.len() == 1
                && record.events[0].crtc == fixture.events.backend.platform.outputs[0].output.crtc
        })
        .expect("output 0's deferred damage composes on the next scheduler pass");
    assert_eq!(output0_after_unflip.events.len(), 1);
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        accepted_event_id(&output0_after_unflip)
    );
    assert_eq!(
        fixture.events.backend.pstate.direct.outputs[1].unflip_awaiting_outputs,
        HashMap::from([(1, fresh_id)]),
        "output 0's scene flip does not retire output 1's direct owner"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep scanout_commit_in_flight_for_outputs over every output in queued dispatch.
fn queued_successor_not_blocked_by_scene_flip_on_other_output() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::Shared, 209, 0) else {
        return;
    };
    let device_fd = fd(&fixture);
    let initial = record_count(&fixture, device_fd);
    damage_scene_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("submit composed scene frames before the queued direct successor");
    let scene_records = records_since(&fixture, device_fd, initial);
    let output0_scene = scene_records
        .iter()
        .find(|record| {
            record.kind == crate::drm::atomic::AtomicCommitKind::Scene
                && record.events.len() == 1
                && record.events[0].crtc == fixture.events.backend.platform.outputs[0].output.crtc
        })
        .expect("output 0 scene flip is in flight");
    let output1_scene = scene_records
        .iter()
        .find(|record| {
            record.kind == crate::drm::atomic::AtomicCommitKind::Scene
                && record.events.len() == 1
                && record.events[0].crtc == fixture.events.backend.platform.outputs[1].output.crtc
        })
        .expect("output 1 scene flip is available for retirement");
    complete_recorded_attempt(
        &mut fixture,
        device_fd,
        output1_scene,
        accepted_event_id(output1_scene).expect("output 1 scene ID"),
        60,
    );
    assert!(fixture.events.backend.scene.has_pending_page_flip(0));
    assert!(!fixture.events.backend.scene.has_pending_page_flip(1));

    seed_output_owner_successor(
        &mut fixture,
        &direct,
        1,
        crate::kms::render::backend::ScanoutM0Target::Unredirected,
        None,
        None,
    );
    let before_successor = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("dispatch output 1's queued direct successor");
    let successor = records_since(&fixture, device_fd, before_successor)
        .into_iter()
        .find(|record| accepted_event_id(record).is_some())
        .expect("queued successor submitted while output 0 scene flip is in flight");
    assert_eq!(successor.events.len(), 1);
    assert_eq!(
        successor.events[0].crtc,
        fixture.events.backend.platform.outputs[1].output.crtc
    );
    assert!(fixture.events.backend.scene.has_pending_page_flip(0));
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .pending
            .is_some()
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .queued_successor
            .is_none()
    );
    assert!(
        output0_scene
            .events
            .iter()
            .all(|event| event.crtc == fixture.events.backend.platform.outputs[0].output.crtc)
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: route retirement to the first active owner instead of scope_for_output.
fn two_output_owners_retire_independently() {
    let Some((mut fixture, first)) = direct_fixture(2, P1SceneRoute::Shared, 205, 0) else {
        return;
    };
    let device_fd = fd(&fixture);
    let second = fixture.install_direct_present(206, 0);
    Backend::set_cursor_hidden(&mut fixture.events.backend, true);
    let (first_record, second_record) =
        submit_two_output_owner_successors(&mut fixture, &first, &second);
    assert_atomic_record_touches_output_only(&fixture, &first_record, 0);
    assert_atomic_record_touches_output_only(&fixture, &second_record, 1);
    let first_id = accepted_event_id(&first_record).expect("first output accepted ID");
    let second_id = accepted_event_id(&second_record).expect("second output accepted ID");
    assert_ne!(first_id, second_id);

    complete_recorded_attempt(&mut fixture, device_fd, &second_record, second_id, 61);
    let second_completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(
        second_completed
            .iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        [second.candidate.present_id]
    );
    assert_eq!(
        second_completed[0].completion_clock,
        Some(yserver_core::backend::PresentClockSample {
            msc: 61,
            ust: 610_000,
            source: yserver_core::backend::PresentClockSource::PageFlip,
        })
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[0]
            .pending
            .is_some()
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_some()
    );
    complete_recorded_attempt(&mut fixture, device_fd, &second_record, second_id, 62);
    assert!(Backend::drain_completed_present_events(&mut fixture.events.backend).is_empty());

    complete_recorded_attempt(&mut fixture, device_fd, &first_record, first_id, 71);
    let first_completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(
        first_completed
            .iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        [first.candidate.present_id]
    );
    assert_eq!(
        first_completed[0].completion_clock,
        Some(yserver_core::backend::PresentClockSample {
            msc: 71,
            ust: 710_000,
            source: yserver_core::backend::PresentClockSource::PageFlip,
        })
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[0]
            .current
            .is_some()
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_some()
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: drain the root slot only.
fn stopped_output_owner_notifications_are_drained() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::Shared, 207, 0) else {
        return;
    };
    let device_fd = fd(&fixture);
    Backend::set_cursor_hidden(&mut fixture.events.backend, true);
    let direct_record = submit_output_owner_successor(&mut fixture, &direct, 1);
    let direct_id = accepted_event_id(&direct_record).expect("direct submit accepted");
    complete_recorded_attempt(&mut fixture, device_fd, &direct_record, direct_id, 73);
    Backend::unmap_subwindow(&mut fixture.events.backend, None, direct.target_xid)
        .expect("unmap direct window through Backend");
    let start = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("submit composed unflip through Backend");
    let crtc = fixture.events.backend.platform.outputs[1].output.crtc;
    let unflip = records_since(&fixture, device_fd, start)
        .into_iter()
        .find(|record| {
            accepted_event_id(record).is_some()
                && record.events.len() == 1
                && record.events[0].crtc == crtc
        })
        .expect("output unflip accepted");
    complete_recorded_attempt(
        &mut fixture,
        device_fd,
        &unflip,
        accepted_event_id(&unflip).unwrap(),
        74,
    );

    let completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(
        completed
            .iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        [direct.candidate.present_id]
    );
    let idled = Backend::drain_retired_present_idle_events(&mut fixture.events.backend);
    assert_eq!(
        idled
            .iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        [direct.candidate.present_id]
    );
    assert!(Backend::drain_completed_present_events(&mut fixture.events.backend).is_empty());
    assert!(Backend::drain_retired_present_idle_events(&mut fixture.events.backend).is_empty());
    assert!(!fixture.events.backend.pstate.direct.outputs[1].active());
    assert!(fixture.events.backend.present_source_pins.is_empty());
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: the VT release stop-all path stops only the root slot.
fn global_teardown_unflips_every_output_owner() {
    use yserver_core::core_loop::input_inventory::InputInventory;

    let Some((mut fixture, first)) = direct_fixture(2, P1SceneRoute::Shared, 209, 0) else {
        return;
    };
    let second = fixture.install_direct_present(210, 0);
    let device_fd = fd(&fixture);
    Backend::set_cursor_hidden(&mut fixture.events.backend, true);
    let (first_record, second_record) =
        submit_two_output_owner_successors(&mut fixture, &first, &second);
    let first_id = accepted_event_id(&first_record).expect("first output accepted ID");
    let second_id = accepted_event_id(&second_record).expect("second output accepted ID");
    complete_recorded_attempt(&mut fixture, device_fd, &first_record, first_id, 81);
    complete_recorded_attempt(&mut fixture, device_fd, &second_record, second_id, 82);
    let completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(
        completed
            .iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        [first.candidate.present_id, second.candidate.present_id]
    );
    assert_eq!(
        fixture.events.backend.pstate.direct.active_scopes(),
        [
            crate::kms::render::backend::DirectScope::Output(0),
            crate::kms::render::backend::DirectScope::Output(1),
        ]
    );

    super::install_recording_vt_io(&mut fixture);
    Backend::begin_vt_release(&mut fixture.events.backend);
    Backend::finish_vt_release(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        &InputInventory::default(),
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .active_scopes()
            .is_empty()
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    assert!(Backend::drain_completed_present_events(&mut fixture.events.backend).is_empty());
    let idled = Backend::drain_retired_present_idle_events(&mut fixture.events.backend);
    assert_eq!(
        idled
            .iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        [first.candidate.present_id, second.candidate.present_id]
    );
    assert!(Backend::drain_retired_present_idle_events(&mut fixture.events.backend).is_empty());
}

pub(super) fn complete_direct_initial(
    fixture: &mut RecordingSceneFixture,
    direct_record: &AtomicCommitRecord,
    direct_id: u64,
    device_fd: RawFd,
) {
    for (output_idx, event) in direct_record.events.iter().enumerate() {
        fixture.events.queue_commit_completion(
            device_fd,
            event.crtc,
            direct_id,
            50 + u32::try_from(output_idx).expect("output index fits"),
            Duration::from_millis(500 + output_idx as u64 * 10),
        );
    }
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .is_none()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .current
            .is_some()
    );
}

pub(super) fn grouped_unflip_record(
    fixture: &mut RecordingSceneFixture,
    device_fd: RawFd,
    offset: usize,
    output_count: usize,
) -> AtomicCommitRecord {
    Backend::maybe_composite(&mut fixture.events.backend).expect("submit grouped unflip");
    records_since(fixture, device_fd, offset)
        .into_iter()
        .find(|record| record.events.len() == output_count && accepted_event_id(record).is_some())
        .expect("grouped composed replacement accepted")
}

fn scene_completion_for_output(
    fixture: &mut RecordingSceneFixture,
    output_idx: usize,
    device_fd: RawFd,
    offset: usize,
) -> AtomicCommitRecord {
    let deadline = Instant::now() + WAIT_DEADLINE;
    loop {
        Backend::before_block(&mut fixture.events.backend);
        let root = fixture.events.backend.core.window_id;
        let width = fixture.events.backend.platform.fb_w;
        let height = fixture.events.backend.platform.fb_h;
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
        .expect("damage root through Backend");
        Backend::maybe_composite(&mut fixture.events.backend)
            .expect("compose degraded replacement through Backend");
        if let Some(record) = records_since(fixture, device_fd, offset)
            .into_iter()
            .find(|record| {
                accepted_event_id(record).is_some()
                    && record.events.len() == 1
                    && record.events[0].crtc
                        == fixture.events.backend.platform.outputs[output_idx]
                            .output
                            .crtc
            })
        {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "output {output_idx} did not accept a degraded replacement"
        );
    }
}

fn compose_copied_job(fixture: &mut RecordingSceneFixture) -> u64 {
    let deadline = Instant::now() + WAIT_DEADLINE;
    loop {
        Backend::before_block(&mut fixture.events.backend);
        let root = fixture.events.backend.core.window_id;
        let width = fixture.events.backend.platform.fb_w;
        let height = fixture.events.backend.platform.fb_h;
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
        .expect("damage copied scene through Backend");
        Backend::maybe_composite(&mut fixture.events.backend)
            .expect("tick copied scene through Backend");
        if let Some(&job_id) = fixture.pending_render_job_ids().last() {
            return job_id;
        }
        assert!(
            Instant::now() < deadline,
            "copied scene did not submit a render job"
        );
    }
}

fn deliver_copied_job(fixture: &mut RecordingSceneFixture, job_id: u64) {
    fixture
        .readiness
        .as_ref()
        .expect("copied fixture has a readiness observer")
        .release_ready_job(job_id);
    let deadline = Instant::now() + WAIT_DEADLINE;
    loop {
        Backend::on_scanout_render_completion(
            &mut fixture.events.backend,
            &mut fixture.events.state,
        );
        if !fixture.pending_render_job_ids().contains(&job_id) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "copied completion was not delivered"
        );
        Backend::before_block(&mut fixture.events.backend);
    }
}

pub(super) fn direct_source_pins(
    fixture: &RecordingSceneFixture,
    source_id: crate::kms::render::store::DrawableId,
) -> usize {
    fixture
        .events
        .backend
        .present_source_pins
        .values()
        .filter(|&&id| id == source_id)
        .count()
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: overwrite the selected reference-output clock when a later output retires.
fn p1_grouped_direct_awaits_accepted_id_on_every_output() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::Shared, 201, 0) else {
        return;
    };
    let device_fd = fd(&fixture);
    let attempts = submit_direct(&mut fixture, &direct, device_fd);
    let accepted = accepted_records(&attempts);
    assert_eq!(accepted.len(), 1);
    let direct_record = &accepted[0];
    let direct_id = accepted_event_id(direct_record).expect("direct request accepted");
    assert_eq!(direct_record.events.len(), 2);
    assert_ne!(direct_id, 0);
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .as_ref()
            .and_then(|frame| frame.accepted_event_id),
        Some(direct_id),
        "the live direct frame retains its accepted grouped ID"
    );
    for event in &direct_record.events {
        assert_eq!(pending_ids(&fixture, device_fd, event.crtc), [direct_id]);
    }

    let reference_clock = yserver_core::backend::PresentClockSample {
        msc: 61,
        ust: 6_100_000,
        source: yserver_core::backend::PresentClockSource::PageFlip,
    };
    fixture.events.queue_commit_completion(
        device_fd,
        direct_record.events[0].crtc,
        u64::MAX,
        60,
        Duration::from_millis(6_000),
    );
    fixture.events.queue_commit_completion(
        device_fd,
        direct_record.events[0].crtc,
        direct_id,
        61,
        Duration::from_millis(6_100),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .completed
            .is_empty()
    );
    assert!(fixture.events.backend.pstate.direct.root().idled.is_empty());
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .as_ref()
            .unwrap()
            .awaiting_outputs,
        HashSet::from([1])
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .as_ref()
            .unwrap()
            .completion_clock,
        Some(reference_clock)
    );
    fixture.events.queue_commit_completion(
        device_fd,
        direct_record.events[0].crtc,
        direct_id,
        62,
        Duration::from_millis(6_200),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .completed
            .is_empty()
    );
    assert_eq!(
        pending_ids(&fixture, device_fd, direct_record.events[1].crtc),
        [direct_id]
    );
    inject_output(
        &mut fixture,
        device_fd,
        direct_record,
        1,
        direct_id,
        63,
        Duration::from_millis(6_300),
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .is_none()
    );
    assert_eq!(
        fixture.events.backend.pstate.direct.root().completed.len(),
        1
    );
    assert_eq!(
        fixture.events.backend.pstate.direct.root().completed[0].completion_clock,
        Some(reference_clock),
        "the selected reference output supplies the completion clock"
    );
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);

    request_unflip(&mut fixture, &direct);
    let offset = record_count(&fixture, device_fd);
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 2);
    let replacement_id = accepted_event_id(&replacement).unwrap();
    complete_recorded_attempt(&mut fixture, device_fd, &replacement, replacement_id, 64);
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: discard the accepted Vsync ID after async EINVAL.
fn p1_direct_async_fallback_uses_only_vsync_acceptance() {
    let Some((mut fixture, direct)) = direct_fixture(
        1,
        P1SceneRoute::Shared,
        202,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    ) else {
        return;
    };
    enter_async_tearing(&mut fixture, &direct);
    let device_fd = fd(&fixture);
    let recorder = fixture.events.recorders[&device_fd].clone();
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    let attempts = submit_direct(&mut fixture, &direct, device_fd);
    assert_eq!(
        attempts.len(),
        2,
        "async request followed by Vsync fallback"
    );
    assert_eq!(
        attempts[0].kind,
        crate::drm::atomic::AtomicCommitKind::Async
    );
    assert_ne!(attempts[0].result, ScriptedResult::Ok);
    assert_eq!(attempts[1].result, ScriptedResult::Ok);
    let async_id = attempts[0].user_data;
    let vsync_id = accepted_event_id(&attempts[1]).expect("Vsync fallback accepted");
    assert_ne!(async_id, 0);
    assert_ne!(vsync_id, async_id);
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .as_ref()
            .and_then(|frame| frame.accepted_event_id),
        Some(vsync_id),
        "the frame awaits only the accepted Vsync request"
    );
    let crtc = attempts[1].events[0].crtc;
    fixture.events.queue_commit_completion(
        device_fd,
        crtc,
        async_id,
        70,
        Duration::from_millis(7_000),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .is_some()
    );
    assert_eq!(pending_ids(&fixture, device_fd, crtc), [vsync_id]);
    complete_recorded_attempt(&mut fixture, device_fd, &attempts[1], vsync_id, 71);
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .current
            .is_some()
    );
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);
    let offset = record_count(&fixture, device_fd);
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 1);
    let replacement_id = accepted_event_id(&replacement).unwrap();
    complete_recorded_attempt(&mut fixture, device_fd, &replacement, replacement_id, 72);
    assert!(fixture.events.backend.present_source_pins.is_empty());
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: convert the two failed atomic attempts into a successful direct submit with cookie zero.
fn p1_direct_both_attempts_fail_without_obligation_or_pin_leak() {
    let Some((mut fixture, async_direct)) = direct_fixture(
        1,
        P1SceneRoute::Shared,
        203,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    ) else {
        return;
    };
    enter_async_tearing(&mut fixture, &async_direct);
    let device_fd = fd(&fixture);
    let recorder = fixture.events.recorders[&device_fd].clone();
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    let start = record_count(&fixture, device_fd);
    for _ in 0..DIRECT_PROBATION - 1 {
        assert!(
            !Backend::try_present_direct(
                &mut fixture.events.backend,
                async_direct.candidate,
                async_direct.event.clone(),
            )
            .expect("direct candidate probation through Backend")
        );
    }
    assert!(
        Backend::try_present_direct(
            &mut fixture.events.backend,
            async_direct.candidate,
            async_direct.event.clone(),
        )
        .is_err(),
        "both direct attempts fail through Backend"
    );
    let failed = records_since(&fixture, device_fd, start);
    assert_eq!(failed.len(), 2);
    assert!(
        failed
            .iter()
            .all(|attempt| attempt.result != ScriptedResult::Ok)
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .is_none()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .current
            .is_none()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .queued_successor
            .is_none()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    for event in &failed[0].events {
        assert!(pending_ids(&fixture, device_fd, event.crtc).is_empty());
    }

    let mut later = async_direct;
    later.candidate.options = 0;
    let mut later_event = later.event.clone();
    later_event.options = 0;
    later.event = later_event;
    let accepted_attempts = submit_direct(&mut fixture, &later, device_fd);
    let accepted = accepted_records(&accepted_attempts);
    assert_eq!(accepted.len(), 1);
    let accepted_id = accepted_event_id(&accepted[0]).unwrap();
    assert_ne!(accepted_id, failed[0].user_data);
    assert_ne!(accepted_id, failed[1].user_data);
    for attempt in &failed {
        for event in &attempt.events {
            fixture.events.queue_commit_completion(
                device_fd,
                event.crtc,
                attempt.user_data,
                80,
                Duration::from_millis(8_000),
            );
        }
    }
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .is_some()
    );
    assert_eq!(
        pending_ids(&fixture, device_fd, accepted[0].events[0].crtc),
        [accepted_id]
    );
    complete_recorded_attempt(&mut fixture, device_fd, &accepted[0], accepted_id, 81);
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .current
            .is_some()
    );
    drain_present_notifications(&mut fixture, &[later.candidate.present_id], &[]);
    request_unflip(&mut fixture, &later);
    let offset = record_count(&fixture, device_fd);
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 1);
    let replacement_id = accepted_event_id(&replacement).unwrap();
    complete_recorded_attempt(&mut fixture, device_fd, &replacement, replacement_id, 82);
    assert!(fixture.events.backend.present_source_pins.is_empty());
    drain_present_notifications(&mut fixture, &[], &[later.candidate.present_id]);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: attribute an explicit ID miss to the queue front so a late direct cookie steals the replacement event.
fn p1_grouped_unflip_ignores_late_direct_or_duplicate_cookie() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::Shared, 204, 0) else {
        return;
    };
    let device_fd = fd(&fixture);
    let attempts = submit_direct(&mut fixture, &direct, device_fd);
    let initial = accepted_records(&attempts).remove(0);
    let direct_id = accepted_event_id(&initial).unwrap();
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);
    let offset = record_count(&fixture, device_fd);
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 2);
    let replacement_id = accepted_event_id(&replacement).unwrap();
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(0, replacement_id), (1, replacement_id)])
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace,
        HashSet::from([0, 1])
    );

    fixture.events.queue_commit_completion(
        device_fd,
        replacement.events[0].crtc,
        direct_id,
        90,
        Duration::from_millis(9_000),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .len(),
        2,
        "a late direct cookie cannot satisfy an accepted replacement"
    );
    inject_output(
        &mut fixture,
        device_fd,
        &replacement,
        0,
        replacement_id,
        91,
        Duration::from_millis(9_100),
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(1, replacement_id)])
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace,
        HashSet::from([1])
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .current
            .is_some()
    );
    assert!(!fixture.events.backend.present_source_pins.is_empty());
    fixture.events.queue_commit_completion(
        device_fd,
        replacement.events[0].crtc,
        replacement_id,
        92,
        Duration::from_millis(9_200),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(1, replacement_id)])
    );
    inject_output(
        &mut fixture,
        device_fd,
        &replacement,
        1,
        replacement_id,
        93,
        Duration::from_millis(9_300),
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: release direct pins while an output still needs replacement; clear unflip_requested before the final replacement retires.
fn p1_degraded_shared_unflip_waits_for_accepted_replacements() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::Shared, 205, 0) else {
        return;
    };
    let device_fd = fd(&fixture);
    let initial_attempts = submit_direct(&mut fixture, &direct, device_fd);
    let initial = accepted_records(&initial_attempts).remove(0);
    let direct_id = accepted_event_id(&initial).unwrap();
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);

    let recorder = fixture.events.recorders[&device_fd].clone();
    let offset = record_count(&fixture, device_fd);
    // Fresh grouped unflip, then the retained-framebuffer grouped unflip.
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    recorder.script_result(Ok(()));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    Backend::maybe_composite(&mut fixture.events.backend).expect("degrade grouped unflip");
    let attempts = records_since(&fixture, device_fd, offset);
    let failed_group = attempts
        .iter()
        .find(|record| record.events.len() == 2 && record.result != ScriptedResult::Ok)
        .expect("grouped unflip rejected");
    let accepted_scene: Vec<_> = accepted_records(&attempts)
        .into_iter()
        .filter(|record| record.events.len() == 1)
        .collect();
    let rejected_scene: Vec<_> = attempts
        .iter()
        .filter(|record| record.events.len() == 1 && record.result != ScriptedResult::Ok)
        .collect();
    assert_eq!(accepted_scene.len(), 1);
    assert_eq!(rejected_scene.len(), 1);
    let replaced = fixture
        .events
        .backend
        .platform
        .outputs
        .iter()
        .position(|output| output.output.crtc == accepted_scene[0].events[0].crtc)
        .unwrap();
    let waiting = 1 - replaced;
    let accepted_id = accepted_event_id(&accepted_scene[0]).unwrap();
    let rejected_id = rejected_scene[0].user_data;
    assert_ne!(failed_group.user_data, accepted_id);
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace,
        HashSet::from([0, 1])
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(replaced, accepted_id)])
    );

    fixture.events.queue_commit_completion(
        device_fd,
        accepted_scene[0].events[0].crtc,
        direct_id,
        100,
        Duration::from_millis(10_000),
    );
    let rejected_crtc = rejected_scene[0].events[0].crtc;
    fixture.events.queue_commit_completion(
        device_fd,
        rejected_crtc,
        rejected_id,
        101,
        Duration::from_millis(10_100),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace
            .len(),
        2
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(replaced, accepted_id)])
    );

    inject_output(
        &mut fixture,
        device_fd,
        &accepted_scene[0],
        0,
        accepted_id,
        102,
        Duration::from_millis(10_200),
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace,
        HashSet::from([waiting])
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .is_empty(),
        "no accepted replacement ID remains while one output still requires replacement"
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .current
            .is_some(),
        "an empty accepted map cannot retire direct scanout while another output still needs replacement"
    );
    assert_eq!(direct_source_pins(&fixture, direct.source_id), 1);
    assert_eq!(fixture.events.backend.present_source_pins.len(), 2);

    let early_reentry_id = direct.candidate.present_id + 10_000;
    let mut early_reentry_candidate = direct.candidate;
    early_reentry_candidate.present_id = early_reentry_id;
    let mut early_reentry_event = direct.event.clone();
    early_reentry_event.present_id = early_reentry_id;
    early_reentry_event.serial = early_reentry_event.serial.wrapping_add(10_000);
    let before_early_reentry = record_count(&fixture, device_fd);
    assert!(
        !Backend::try_present_direct(
            &mut fixture.events.backend,
            early_reentry_candidate,
            early_reentry_event,
        )
        .expect("eligible Present remains behind degraded unflip"),
        "direct admission stays closed while any degraded output needs replacement"
    );
    assert_eq!(
        record_count(&fixture, device_fd),
        before_early_reentry,
        "the eligible Present must not submit a direct commit"
    );
    assert!(fixture.events.backend.pstate.direct.root().unflip_requested);
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .pending
            .is_none()
    );

    let later_offset = record_count(&fixture, device_fd);
    recorder.script_result(Ok(()));
    let later = scene_completion_for_output(&mut fixture, waiting, device_fd, later_offset);
    let later_id = accepted_event_id(&later).unwrap();
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(waiting, later_id)])
    );
    let later_scene_frames = accepted_records(&records_since(&fixture, device_fd, later_offset));
    assert!(later_scene_frames.iter().any(|record| {
        accepted_event_id(record) == Some(later_id) && record.events[0].crtc == later.events[0].crtc
    }));
    for (index, record) in later_scene_frames.iter().enumerate() {
        complete_recorded_attempt(
            &mut fixture,
            device_fd,
            record,
            accepted_event_id(record).unwrap(),
            103 + u32::try_from(index).expect("scene frame index fits"),
        );
    }
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);

    fixture.drain_deferred_pool_releases(Duration::from_secs(5));
    assert!(
        !fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .reentry_blocked_until_composed,
        "normal composed fallback reopens direct admission after every replacement retires"
    );
    let admitted_reentry_id = direct.candidate.present_id + 10_001;
    let admitted_start = record_count(&fixture, device_fd);
    for attempt in 0..DIRECT_PROBATION {
        let present_id = admitted_reentry_id + u64::try_from(attempt).unwrap();
        let mut candidate = direct.candidate;
        candidate.present_id = present_id;
        let mut event = direct.event.clone();
        event.present_id = present_id;
        event.serial = event.serial.wrapping_add(u32::try_from(attempt).unwrap());
        let accepted = Backend::try_present_direct(&mut fixture.events.backend, candidate, event)
            .expect("eligible Present after all unflip requirements retire");
        assert_eq!(
            accepted,
            attempt + 1 == DIRECT_PROBATION,
            "direct admission resumes only after the existing probation"
        );
    }
    let reentered = accepted_records(&records_since(&fixture, device_fd, admitted_start));
    assert_eq!(reentered.len(), 1, "the admitted Present submits once");
    let reentered_id = accepted_event_id(&reentered[0]).unwrap();
    complete_recorded_attempt(&mut fixture, device_fd, &reentered[0], reentered_id, 104);
    drain_present_notifications(
        &mut fixture,
        &[admitted_reentry_id + (DIRECT_PROBATION as u64 - 1)],
        &[],
    );

    request_unflip(&mut fixture, &direct);
    let cleanup_offset = record_count(&fixture, device_fd);
    let cleanup = grouped_unflip_record(&mut fixture, device_fd, cleanup_offset, 2);
    let cleanup_id = accepted_event_id(&cleanup).unwrap();
    complete_recorded_attempt(&mut fixture, device_fd, &cleanup, cleanup_id, 105);
    assert!(fixture.events.backend.present_source_pins.is_empty());
    drain_present_notifications(
        &mut fixture,
        &[],
        &[admitted_reentry_id + (DIRECT_PROBATION as u64 - 1)],
    );
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit post-tick registration of an already-signalled copied replacement accepted inside scene.tick.
fn p1_degraded_copied_unflip_registers_immediately_accepted_replacement() {
    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::CopiedSameDevice, 206, 0)
    else {
        return;
    };
    let device_fd = fd(&fixture);
    let initial_attempts = submit_direct(&mut fixture, &direct, device_fd);
    let initial = accepted_records(&initial_attempts).remove(0);
    let direct_id = accepted_event_id(&initial).unwrap();
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);
    fixture.events.recorders[&device_fd]
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("degrade copied grouped unflip and tick scene");

    let deadline = Instant::now() + WAIT_DEADLINE;
    let replacement = loop {
        let Some(replacement) = records_since(&fixture, device_fd, 0)
            .into_iter()
            .rev()
            .find(|record| {
                record.events.len() == 1
                    && record.device_fd == device_fd
                    && accepted_event_id(record).is_some()
                    && record.kind != crate::drm::atomic::AtomicCommitKind::Async
            })
            .filter(|record| record.user_data != direct_id)
        else {
            assert!(
                Instant::now() < deadline,
                "scene tick did not accept a copied replacement"
            );
            Backend::before_block(&mut fixture.events.backend);
            let root = fixture.events.backend.core.window_id;
            let width = fixture.events.backend.platform.fb_w;
            let height = fixture.events.backend.platform.fb_h;
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
            .expect("damage copied replacement through Backend");
            Backend::maybe_composite(&mut fixture.events.backend)
                .expect("tick copied replacement through Backend");
            continue;
        };
        if replacement.events[0].crtc == fixture.events.backend.platform.outputs[0].output.crtc
            && replacement.user_data != direct_id
        {
            break replacement;
        }
        assert!(
            Instant::now() < deadline,
            "copied replacement event targeted another CRTC"
        );
    };
    let replacement_id = accepted_event_id(&replacement).unwrap();
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(0, replacement_id)]),
        "scene tick registers the accepted sink ID before returning"
    );
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(replacement_id)
    );
    assert!(fixture.pending_render_job_ids().is_empty());

    fixture.events.queue_commit_completion(
        device_fd,
        replacement.events[0].crtc,
        direct_id,
        110,
        Duration::from_millis(11_000),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(0, replacement_id)])
    );
    complete_recorded_attempt(&mut fixture, device_fd, &replacement, replacement_id, 111);
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    assert!(
        fixture
            .copy_ownership_observation()
            .iter()
            .all(|row| { !(row.contains("phase=Free") && row.contains("ForeignPendingKms")) })
    );
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: record source-render readiness as a sink ID; select the grouped rejection instead of the rejected sink submit.
fn p1_degraded_copied_unflip_has_no_id_before_sink_acceptance() {
    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::CopiedSameDevice, 207, 0)
    else {
        return;
    };
    let readiness = fixture.readiness.as_ref().unwrap();
    readiness.hold_ready_jobs();
    let device_fd = fd(&fixture);
    let initial_attempts = submit_direct(&mut fixture, &direct, device_fd);
    let initial = accepted_records(&initial_attempts).remove(0);
    let direct_id = accepted_event_id(&initial).unwrap();
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);
    let recorder = fixture.events.recorders[&device_fd].clone();
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    let start = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend).expect("degrade grouped copied unflip");
    let job_id = fixture
        .pending_render_job_ids()
        .last()
        .copied()
        .unwrap_or_else(|| compose_copied_job(&mut fixture));
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .is_empty(),
        "source render readiness must not create an accepted sink identity"
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace,
        HashSet::from([0])
    );
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        None
    );
    assert_eq!(direct_source_pins(&fixture, direct.source_id), 1);
    assert_eq!(fixture.events.backend.present_source_pins.len(), 2);
    assert!(
        records_since(&fixture, device_fd, start)
            .iter()
            .all(|record| accepted_event_id(record).is_none())
    );

    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    let sink_start = record_count(&fixture, device_fd);
    deliver_copied_job(&mut fixture, job_id);
    let sink_attempts = records_since(&fixture, device_fd, sink_start);
    assert_eq!(
        sink_attempts.len(),
        1,
        "the selector begins immediately before the rejected sink submission"
    );
    let rejected = sink_attempts
        .into_iter()
        .find(|record| record.events.len() == 1 && record.result != ScriptedResult::Ok)
        .expect("sink acceptance rejected");
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace,
        HashSet::from([0])
    );
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        None
    );

    let accepted_start = record_count(&fixture, device_fd);
    let next_job = compose_copied_job(&mut fixture);
    deliver_copied_job(&mut fixture, next_job);
    let accepted = records_since(&fixture, device_fd, accepted_start)
        .into_iter()
        .find(|record| accepted_event_id(record).is_some())
        .expect("later copied sink request accepted");
    let accepted_id = accepted_event_id(&accepted).unwrap();
    assert_ne!(accepted_id, rejected.user_data);
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(0, accepted_id)])
    );
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(accepted_id)
    );

    fixture.events.queue_commit_completion(
        device_fd,
        accepted.events[0].crtc,
        direct_id,
        120,
        Duration::from_millis(12_000),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        device_fd,
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs,
        HashMap::from([(0, accepted_id)])
    );
    complete_recorded_attempt(&mut fixture, device_fd, &accepted, accepted_id, 121);
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    assert!(fixture.pending_render_job_ids().is_empty());
    assert!(
        fixture
            .copy_ownership_observation()
            .iter()
            .all(|row| { !(row.contains("phase=Free") && row.contains("ForeignPendingKms")) })
    );
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    fixture.finish_after_normal_drain();
}
