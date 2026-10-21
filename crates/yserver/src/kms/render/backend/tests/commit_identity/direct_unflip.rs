//! Direct-frame and unflip retirement by accepted atomic commit identity.

use super::{
    atomic_events::{complete_recorded_attempt, pending_ids},
    recording_scene::{DirectPresentSetup, P1SceneRoute, RecordingSceneFixture},
    support::accepted_event_id,
};
use crate::{
    drm::atomic::tests_support::{AtomicCommitRecord, ScriptedResult},
    kms::vk::scanout::BoPhase,
};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    os::fd::RawFd,
    rc::Rc,
    time::{Duration, Instant},
};
use yserver_core::backend::Backend;

const DIRECT_PROBATION: usize = 8;
const WAIT_DEADLINE: Duration = Duration::from_secs(5);

fn direct_fixture(
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

fn fd(fixture: &RecordingSceneFixture) -> RawFd {
    fixture.kms_fd(0)
}

fn record_count(fixture: &RecordingSceneFixture, device_fd: RawFd) -> usize {
    fixture.events.recorders[&device_fd].records().len()
}

fn records_since(
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

fn accepted_records(records: &[AtomicCommitRecord]) -> Vec<AtomicCommitRecord> {
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

fn enter_async_tearing(fixture: &mut RecordingSceneFixture, direct: &DirectPresentSetup) {
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

fn submit_direct(
    fixture: &mut RecordingSceneFixture,
    direct: &DirectPresentSetup,
    device_fd: RawFd,
) -> Vec<AtomicCommitRecord> {
    let start = record_count(fixture, device_fd);
    admit_direct(fixture, direct);
    records_since(fixture, device_fd, start)
}

fn inject_output(
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

fn request_unflip(fixture: &mut RecordingSceneFixture, direct: &DirectPresentSetup) {
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
    assert!(fixture.events.backend.pstate.scanout_m2.unflip_requested);
}

fn drain_present_notifications(
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
    for event in &completed {
        fixture
            .present_notification_counts
            .entry(event.present_id)
            .or_default()
            .0 += 1;
    }
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
    for event in &idled {
        fixture
            .present_notification_counts
            .entry(event.present_id)
            .or_default()
            .1 += 1;
    }
}

fn assert_issue281_unflip_end_state(fixture: &RecordingSceneFixture, present_id: u64) {
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .composed_unflip
            .is_none(),
        "no composed-unflip batch remains"
    );
    assert!(
        fixture.pending_render_job_ids().is_empty(),
        "no copied source render job remains"
    );
    assert!(
        fixture
            .events
            .backend
            .platform
            .for_tests_pending_scanout_render_job_ids()
            .is_empty(),
        "no render-completion or safe-dependency notification remains"
    );
    fixture.assert_quiescent();
    assert_eq!(
        fixture.present_notification_counts.get(&present_id),
        Some(&(1, 1)),
        "the direct Present has exactly one Complete and one Idle"
    );
    assert!(
        fixture
            .events
            .backend
            .platform
            .scanout_pools
            .iter()
            .filter_map(Option::as_ref)
            .flat_map(|scanout| scanout.display_pool().bos.iter())
            .all(|bo| {
                matches!(bo.state.phase, BoPhase::Free | BoPhase::OnScreen)
                    && bo.state.in_fence_fd.is_none()
            }),
        "every scanout BO is Free or OnScreen with no owned in-fence fd"
    );
}

fn complete_direct_initial(
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
    assert!(fixture.events.backend.pstate.scanout_m2.pending.is_none());
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_some());
}

fn grouped_unflip_record(
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

fn primary_fb_value(record: &AtomicCommitRecord, output: &crate::drm::modeset::Output) -> u32 {
    let value = record
        .properties
        .iter()
        .find(|update| {
            update.object == output.plane.into() && update.property == output.plane_fb_id_prop
        })
        .expect("recorded primary plane FB_ID")
        .value;
    u32::try_from(value).expect("primary FB_ID fits u32")
}

fn primary_in_fence_value(
    record: &AtomicCommitRecord,
    output: &crate::drm::modeset::Output,
) -> u64 {
    record
        .properties
        .iter()
        .find(|update| {
            update.object == output.plane.into()
                && Some(update.property) == output.plane_in_fence_fd_prop
        })
        .expect("recorded primary-plane IN_FENCE_FD")
        .value
}

fn paint_solid_pixmap(fixture: &mut RecordingSceneFixture, xid: u32, pixel: [u8; 4]) {
    let backend = &mut fixture.events.backend;
    let width = usize::from(backend.platform.fb_w);
    let height = usize::from(backend.platform.fb_h);
    let width_u16 = backend.platform.fb_w;
    let height_u16 = backend.platform.fb_h;
    let mut bytes = Vec::with_capacity(width * height * pixel.len());
    for _ in 0..width * height {
        bytes.extend_from_slice(&pixel);
    }
    Backend::put_image(backend, None, xid, 24, width_u16, height_u16, 0, 0, &bytes)
        .expect("paint a solid scene pixmap through Backend");
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: select the retained OnScreen BO for the replacement; the fresh-target assertion must fail.
fn issue281_fullscreen_exit_flips_fresh_scene() {
    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::Shared, 281, 0) else {
        println!("SKIP issue281_fullscreen_exit_flips_fresh_scene: no Vulkan test device");
        return;
    };
    let device_fd = fd(&fixture);
    let scene_a = [0x11, 0x37, 0x5b, 0xff];
    let scene_b = [0x72, 0x24, 0x19, 0xff];
    assert_ne!(scene_a, scene_b);
    paint_solid_pixmap(&mut fixture, direct.target_xid, scene_a);
    let scene_a_offset = record_count(&fixture, device_fd);
    let scene_a_frame = scene_completion_for_output(&mut fixture, 0, device_fd, scene_a_offset);
    let scene_a_id = accepted_event_id(&scene_a_frame).expect("scene A commit accepted");
    complete_recorded_attempt(&mut fixture, device_fd, &scene_a_frame, scene_a_id, 280);
    let old_on_screen_idx = fixture.events.backend.platform.scanout_pools[0]
        .as_ref()
        .expect("scanout pool")
        .display_pool()
        .bos
        .iter()
        .position(|bo| bo.state.phase == BoPhase::OnScreen)
        .expect("scene A is on screen");
    let old_scene_backing = fixture
        .events
        .backend
        .store
        .lookup(0x6a00 + u32::try_from(old_on_screen_idx).expect("BO index fits"))
        .expect("scene A scanout backing");
    let old_scene_pixels = fixture.backing_pixels_for_id(old_scene_backing);
    paint_solid_pixmap(&mut fixture, direct.candidate.src_pixmap_xid, scene_b);
    let initial_attempts = submit_direct(&mut fixture, &direct, device_fd);
    let initial = accepted_records(&initial_attempts).remove(0);
    let direct_id = accepted_event_id(&initial).expect("direct Present accepted");
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);

    let retained_fb = fixture.events.backend.platform.scanout_pools[0]
        .as_ref()
        .expect("scanout pool")
        .display_pool()
        .bos
        .iter()
        .find(|bo| bo.state.phase == BoPhase::OnScreen)
        .and_then(|bo| bo.fb_handle)
        .map(u32::from)
        .expect("retained OnScreen framebuffer");
    let offset = record_count(&fixture, device_fd);
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 1);
    let replacement_fb = primary_fb_value(
        &replacement,
        &fixture.events.backend.platform.outputs[0].output,
    );

    assert_ne!(
        replacement_fb, retained_fb,
        "fresh unflip must render into a Free BO while preserving the old OnScreen BO"
    );
    let target_bo_idx = fixture.events.backend.platform.scanout_pools[0]
        .as_ref()
        .expect("scanout pool")
        .display_pool()
        .bos
        .iter()
        .position(|bo| {
            bo.fb_handle
                .is_some_and(|fb| u32::from(fb) == replacement_fb)
        })
        .expect("grouped FB_ID names a pool BO");
    let target = fixture.events.backend.platform.scanout_pools[0]
        .as_ref()
        .expect("scanout pool")
        .display_pool()
        .bos
        .iter()
        .find(|bo| {
            bo.fb_handle
                .is_some_and(|fb| u32::from(fb) == replacement_fb)
        })
        .expect("grouped FB_ID names a pool BO");
    assert_eq!(target.state.phase, BoPhase::Pending);
    let fresh_scene_pixels = fixture.backing_pixels_for_id(
        fixture
            .events
            .backend
            .store
            .lookup(0x6a00 + u32::try_from(target_bo_idx).expect("BO index fits"))
            .expect("fresh scanout backing"),
    );
    assert_ne!(
        old_scene_pixels, fresh_scene_pixels,
        "the replacement contains scene B instead of the retained scene A pixels"
    );
    let fallback_pixels = fixture.backing_pixels_for_id(
        fixture
            .events
            .backend
            .store
            .lookup(direct.target_xid)
            .expect("materialized fallback backing"),
    );
    let source_pixels = fixture.backing_pixels_for_id(direct.source_id);
    assert_eq!(
        fresh_scene_pixels, fallback_pixels,
        "the fresh primary framebuffer contains the materialized full scene B"
    );
    assert_eq!(
        fallback_pixels, source_pixels,
        "scene B was fully materialized"
    );
    assert_ne!(
        fallback_pixels, old_scene_pixels,
        "scene B differs from composed scene A"
    );
    assert!(
        fixture.events.backend.platform.scanout_pools[0]
            .as_ref()
            .unwrap()
            .display_pool()
            .bos
            .iter()
            .any(
                |bo| bo.fb_handle.is_some_and(|fb| u32::from(fb) == retained_fb)
                    && bo.state.phase == BoPhase::OnScreen
            )
    );

    let replacement_id = accepted_event_id(&replacement).expect("grouped replacement accepted");
    complete_recorded_attempt(&mut fixture, device_fd, &replacement, replacement_id, 281);
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: remove the direct-exit watchdog deadline; this test must fail to wake without scene or Present input.
fn issue281_direct_exit_watchdog_prevents_96s_freeze() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::Shared, 282, 0) else {
        println!("SKIP issue281_direct_exit_watchdog_prevents_96s_freeze: no Vulkan test device");
        return;
    };
    let device_fd = fd(&fixture);
    let clock = Rc::new(Cell::new(Instant::now()));
    let test_clock = Rc::clone(&clock);
    fixture
        .events
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || test_clock.get()));

    let scene_offset = record_count(&fixture, device_fd);
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
    .expect("damage root before entering direct mode");
    Backend::maybe_composite(&mut fixture.events.backend).expect("compose the initial scene");
    let initial_scene = accepted_records(&records_since(&fixture, device_fd, scene_offset));
    assert_eq!(
        initial_scene.len(),
        2,
        "both outputs start on a composed scene"
    );
    for (index, record) in initial_scene.iter().enumerate() {
        complete_recorded_attempt(
            &mut fixture,
            device_fd,
            record,
            accepted_event_id(record).expect("initial scene commit accepted"),
            40 + u32::try_from(index).expect("output index fits"),
        );
    }

    let direct_attempts = submit_direct(&mut fixture, &direct, device_fd);
    let direct_record = accepted_records(&direct_attempts).remove(0);
    let direct_id = accepted_event_id(&direct_record).expect("direct Present accepted");
    complete_direct_initial(&mut fixture, &direct_record, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);

    let watchdog = clock.get() + Duration::from_secs(2);
    assert_eq!(
        Backend::next_wakeup(&fixture.events.backend),
        Some(watchdog),
        "the exit request installs one two-second deadline"
    );
    clock.set(watchdog);
    assert_eq!(
        Backend::next_wakeup(&fixture.events.backend),
        Some(watchdog),
        "the expired watchdog is due immediately"
    );
    let offset = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("watchdog starts the fresh-frame unflip without unrelated input");
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 2);
    let replacement_id = accepted_event_id(&replacement).expect("grouped replacement accepted");
    complete_recorded_attempt(&mut fixture, device_fd, &replacement, replacement_id, 282);
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.events.wait_for_submitted_gpu_work();
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore the Composite Overlay Window identity guard for CowDescendant fallbacks.
fn issue281_cow_descendant_unflip_does_not_exit_server() {
    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::Shared, 284, 0) else {
        println!("SKIP issue281_cow_descendant_unflip_does_not_exit_server: no Vulkan test device");
        return;
    };
    let device_fd = fd(&fixture);
    let overlay_xid = yserver_core::resources::COMPOSITE_OVERLAY_WINDOW.0;
    fixture.track_extra_drawable(overlay_xid);
    Backend::get_overlay_window(&mut fixture.events.backend, None)
        .expect("create the Composite Overlay Window through Backend");
    fixture
        .events
        .backend
        .windows
        .get_mut(&direct.target_xid)
        .expect("direct fallback target")
        .parent = Some(overlay_xid);
    let fallback_id = fixture
        .events
        .backend
        .store
        .lookup(direct.target_xid)
        .expect("pinned direct fallback backing");
    assert_ne!(
        fallback_id,
        fixture
            .events
            .backend
            .cow_id
            .expect("overlay backing is live"),
        "CowDescendant fallback is a distinct drawable from the overlay backing"
    );
    paint_solid_pixmap(
        &mut fixture,
        direct.candidate.src_pixmap_xid,
        [0x4a, 0x7c, 0x13, 0xff],
    );

    let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd))
        .into_iter()
        .next()
        .expect("direct root frame accepted");
    let direct_id = accepted_event_id(&initial).expect("accepted direct frame has an ID");
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);

    let offset = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("materialize and compose the CowDescendant fallback");
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 1);
    let replacement_fb = primary_fb_value(
        &replacement,
        &fixture.events.backend.platform.outputs[0].output,
    );
    let replacement_idx = fixture.events.backend.platform.scanout_pools[0]
        .as_ref()
        .expect("scanout pool")
        .display_pool()
        .bos
        .iter()
        .position(|bo| {
            bo.fb_handle
                .is_some_and(|fb| u32::from(fb) == replacement_fb)
        })
        .expect("grouped FB_ID names its prepared scanout BO");
    let replacement_id = fixture
        .events
        .backend
        .store
        .lookup(0x6a00 + u32::try_from(replacement_idx).expect("BO index fits"))
        .expect("fresh scanout backing");
    let source_pixels = fixture.backing_pixels_for_id(direct.source_id);
    assert_eq!(
        fixture.backing_pixels_for_id(fallback_id),
        source_pixels,
        "materialization writes the exact pinned non-overlay fallback"
    );
    assert_eq!(
        fixture.backing_pixels_for_id(replacement_id),
        source_pixels,
        "the grouped primary FB contains the fresh CowDescendant scene"
    );

    let replacement_commit = accepted_event_id(&replacement).expect("group accepted");
    complete_recorded_attempt(
        &mut fixture,
        device_fd,
        &replacement,
        replacement_commit,
        284,
    );
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.events.wait_for_submitted_gpu_work();
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: treat a signalled None dependency as missing and leave UnflipPrepared blocking fallback.
fn issue281_cancellation_with_none_dependency_recycles_inline() {
    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::Shared, 285, 0) else {
        println!(
            "SKIP issue281_cancellation_with_none_dependency_recycles_inline: no Vulkan test device"
        );
        return;
    };
    let device_fd = fd(&fixture);
    let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd))
        .into_iter()
        .next()
        .expect("direct root frame accepted");
    let direct_id = accepted_event_id(&initial).expect("accepted direct frame has an ID");
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);
    let offset = record_count(&fixture, device_fd);
    fixture.events.recorders[&device_fd]
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    fixture.events.recorders[&device_fd].script_result(Ok(()));
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("reject the group and submit the safe per-output fallback");
    let attempts = records_since(&fixture, device_fd, offset);
    let rejected = attempts
        .iter()
        .find(|record| record.events.len() == 1 && record.result != ScriptedResult::Ok)
        .expect("grouped replacement rejected with EIO");
    assert!(accepted_event_id(rejected).is_none());
    assert_eq!(
        primary_in_fence_value(rejected, &fixture.events.backend.platform.outputs[0].output,),
        u64::MAX,
        "a None dependency is submitted as the sign-extended -1 sentinel"
    );
    let fallback = attempts
        .iter()
        .find(|record| record.events.len() == 1 && accepted_event_id(record).is_some())
        .expect("safe None cancellation lets the ordinary scene fallback proceed inline");
    assert!(
        fixture
            .events
            .backend
            .scene
            .pending_unflip_frame_identities()
            .is_empty(),
        "the deferred acknowledgement was removed before fallback submission"
    );
    assert!(
        fixture
            .events
            .backend
            .platform
            .for_tests_pending_scanout_render_job_ids()
            .is_empty(),
        "an already-signalled None dependency does not arm a notification wait"
    );
    let fallback_id = accepted_event_id(fallback).expect("accepted fallback has an ID");
    complete_recorded_attempt(&mut fixture, device_fd, fallback, fallback_id, 285);
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.events.wait_for_submitted_gpu_work();
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: treat EBUSY as persistent; retire direct ownership on the first CRTC; invalidate the fresh scene after final retirement.
fn issue281_group_waits_for_every_render_and_retires_once() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::Shared, 283, 0) else {
        println!(
            "SKIP issue281_group_waits_for_every_render_and_retires_once: no Vulkan test device"
        );
        return;
    };
    let device_fd = fd(&fixture);
    let clock = Rc::new(Cell::new(Instant::now()));
    let test_clock = Rc::clone(&clock);
    fixture
        .events
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || test_clock.get()));
    let initial_attempts = submit_direct(&mut fixture, &direct, device_fd);
    let initial = accepted_records(&initial_attempts).remove(0);
    let direct_id = accepted_event_id(&initial).expect("direct Present accepted");
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);

    let offset = record_count(&fixture, device_fd);
    fixture.events.recorders[&device_fd]
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    fixture.events.recorders[&device_fd].script_result(Ok(()));
    Backend::maybe_composite(&mut fixture.events.backend).expect("prepare EBUSY retry");
    let rejected = records_since(&fixture, device_fd, offset)
        .into_iter()
        .find(|record| record.events.len() == 2 && record.result != ScriptedResult::Ok)
        .expect("first grouped attempt is transiently rejected");
    assert!(accepted_event_id(&rejected).is_none());
    let mut retained_fences = Vec::new();
    for (output_idx, layout) in fixture.events.backend.platform.outputs.iter().enumerate() {
        let fb = primary_fb_value(&rejected, &layout.output);
        let bo = fixture.events.backend.platform.scanout_pools[output_idx]
            .as_ref()
            .expect("scanout pool")
            .display_pool()
            .bos
            .iter()
            .find(|bo| {
                bo.fb_handle
                    .is_some_and(|fb_handle| u32::from(fb_handle) == fb)
            })
            .expect("transient attempt keeps its fresh target");
        assert_eq!(bo.state.phase, BoPhase::Submitted);
        let raw_fd = bo.state.in_fence_fd.map_or(u64::MAX, |fd| {
            u64::try_from(fd).expect("in-fence fd is nonnegative")
        });
        assert_eq!(
            primary_in_fence_value(&rejected, &layout.output),
            raw_fd,
            "the ioctl's per-plane fence is the BO-owned dependency"
        );
        retained_fences.push((fb, raw_fd));
    }
    assert!(matches!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .composed_unflip
            .as_ref()
            .unwrap()
            .phase,
        crate::kms::render::backend::ComposedUnflipPhase::RetryWaiting
    ));
    clock.set(clock.get() + Duration::from_millis(100));
    Backend::maybe_composite(&mut fixture.events.backend).expect("retry the same prepared batch");
    let replacement = records_since(&fixture, device_fd, offset)
        .into_iter()
        .find(|record| record.events.len() == 2 && accepted_event_id(record).is_some())
        .expect("bounded retry accepts one grouped replacement");
    for layout in &fixture.events.backend.platform.outputs {
        let fb = primary_fb_value(&replacement, &layout.output);
        let fence = primary_in_fence_value(&replacement, &layout.output);
        assert!(
            retained_fences.contains(&(fb, fence)),
            "retry reuses the same prepared FB and final fence"
        );
    }
    let replacement_id = accepted_event_id(&replacement).expect("grouped replacement accepted");
    assert!(
        replacement
            .flags
            .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT)
    );
    assert!(
        replacement
            .flags
            .contains(::drm::control::AtomicCommitFlags::NONBLOCK)
    );
    assert!(
        !replacement
            .flags
            .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC)
    );
    assert_eq!(
        replacement.events.len(),
        2,
        "both CRTCs use one atomic replacement"
    );

    inject_output(
        &mut fixture,
        device_fd,
        &replacement,
        0,
        replacement_id.wrapping_add(1),
        60,
        Duration::from_millis(600),
    );
    inject_output(
        &mut fixture,
        device_fd,
        &replacement,
        0,
        direct_id,
        61,
        Duration::from_millis(610),
    );
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_some());
    assert_eq!(direct_source_pins(&fixture, direct.source_id), 1);

    inject_output(
        &mut fixture,
        device_fd,
        &replacement,
        0,
        replacement_id,
        62,
        Duration::from_millis(620),
    );
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_some());
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs,
        HashMap::from([(1, replacement_id)]),
        "one correct event retires only its own output"
    );
    inject_output(
        &mut fixture,
        device_fd,
        &replacement,
        0,
        replacement_id,
        63,
        Duration::from_millis(630),
    );
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_some());
    inject_output(
        &mut fixture,
        device_fd,
        &replacement,
        1,
        direct_id,
        64,
        Duration::from_millis(640),
    );
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_some());

    inject_output(
        &mut fixture,
        device_fd,
        &replacement,
        1,
        replacement_id,
        65,
        Duration::from_millis(650),
    );
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_none());
    assert!(fixture.events.backend.present_source_pins.is_empty());
    assert!(!fixture.events.backend.scene.owes_repaint());
    let accepted_count = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("a clean scene does not need another replacement");
    assert_eq!(
        record_count(&fixture, device_fd),
        accepted_count,
        "normal retirement does not cause unsolicited compose or a second replacement"
    );
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.events.wait_for_submitted_gpu_work();
    fixture.finish();
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

fn direct_source_pins(
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

fn configure_migration_fixture(
    fixture: &mut RecordingSceneFixture,
    direct: &DirectPresentSetup,
    cow_descendant: bool,
) -> (crate::kms::render::store::DrawableId, Vec<u8>) {
    use yserver_core::backend::Backend;

    if cow_descendant {
        fixture.track_extra_drawable(yserver_core::resources::COMPOSITE_OVERLAY_WINDOW.0);
    }
    let backend = &mut fixture.events.backend;
    let target_id = backend
        .store
        .lookup(direct.target_xid)
        .expect("direct fallback window has a backing");
    if cow_descendant {
        Backend::get_overlay_window(backend, None).expect("materialize Composite Overlay Window");
        let overlay_xid = yserver_core::resources::COMPOSITE_OVERLAY_WINDOW.0;
        backend
            .windows
            .get_mut(&direct.target_xid)
            .expect("direct target geometry")
            .parent = Some(overlay_xid);
        let target = backend
            .resolve_paint_target(direct.target_xid)
            .expect("CowDescendant resolves a direct fallback target");
        assert_eq!(target.backing_id(), target_id);
        assert_ne!(
            target.backing_id(),
            backend.cow_id.expect("overlay backing is live"),
            "the pinned fallback is not the Composite Overlay Window"
        );
    }

    let width = backend.platform.fb_w;
    let height = backend.platform.fb_h;
    Backend::fill_rectangle(
        backend,
        None,
        direct.target_xid,
        0x0000_11aa,
        0,
        0,
        width,
        height,
    )
    .expect("seed old backing with pixels A through Backend");
    Backend::fill_rectangle(
        backend,
        None,
        direct.candidate.src_host_xid,
        0x0000_33cc,
        0,
        0,
        width,
        height,
    )
    .expect("seed direct source with pixels B through Backend");
    let source_pixels = backend
        .backing_pixels_for_tests(direct.candidate.src_host_xid)
        .expect("read direct source backing")
        .2;
    (target_id, source_pixels)
}

fn finish_configure_migration(
    mut fixture: RecordingSceneFixture,
    direct: &DirectPresentSetup,
    device_fd: RawFd,
) {
    let offset = record_count(&fixture, device_fd);
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 1);
    let replacement_id = accepted_event_id(&replacement).expect("accepted unflip replacement");
    complete_recorded_attempt(&mut fixture, device_fd, &replacement, replacement_id, 95);
    assert!(
        fixture.events.backend.present_source_pins.is_empty(),
        "direct source and fallback pins release after the replacement retires"
    );
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit or move the materialization hook after migration; both saved pixel comparisons must fail.
fn issue281_configure_no_background_resize_migrates_materialized_direct_pixels() {
    use yserver_core::{backend::Backend, host_x11::HostSubwindowConfig};

    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::Shared, 301, 0) else {
        return;
    };
    let (old_id, source_pixels) = configure_migration_fixture(&mut fixture, &direct, false);
    let device_fd = fd(&fixture);
    let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd));
    assert_eq!(initial.len(), 1, "one direct frame is accepted");
    let direct_id = accepted_event_id(&initial[0]).expect("direct frame accepted");
    complete_direct_initial(&mut fixture, &initial[0], direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .current
            .as_ref()
            .expect("direct frame is on screen")
            .fallback_target
            .backing_id(),
        old_id
    );

    let width = fixture.events.backend.platform.fb_w;
    let height = fixture.events.backend.platform.fb_h;
    Backend::configure_subwindow(
        &mut fixture.events.backend,
        None,
        direct.target_xid,
        HostSubwindowConfig {
            width: Some(width + 1),
            height: Some(height + 1),
            ..HostSubwindowConfig::default()
        },
    )
    .expect("real ConfigureWindow resize through Backend");
    assert!(fixture.events.backend.pstate.scanout_m2.unflip_requested);

    let pinned_fallback = fixture.backing_pixels_for_id(old_id);
    let replacement_id = fixture
        .events
        .backend
        .store
        .lookup(direct.target_xid)
        .expect("replacement leaf backing");
    assert_ne!(
        replacement_id, old_id,
        "resize reallocates the leaf backing"
    );
    let (replacement_width, replacement_height, replacement_pixels) = fixture
        .events
        .backend
        .backing_pixels_for_tests(direct.target_xid)
        .expect("replacement backing pixels");
    let expected_size = usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("pixel byte count fits usize");
    let expected_source = source_pixels[..expected_size].to_vec();
    let migrated_region = super::support::bgra_region(
        &replacement_pixels,
        replacement_width,
        0,
        0,
        u32::from(width),
        u32::from(height),
    );
    assert_eq!(
        (replacement_width, replacement_height),
        (u32::from(width + 1), u32::from(height + 1))
    );

    let pixel_assertions = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert!(
            pinned_fallback == expected_source,
            "the exact pinned fallback receives direct pixels B; first actual/expected: {:?}/{:?}",
            &pinned_fallback[..4],
            &expected_source[..4]
        );
        assert!(
            migrated_region == expected_source,
            "the replacement backing is seeded from direct pixels B; first actual/expected: {:?}/{:?}",
            &migrated_region[..4],
            &expected_source[..4]
        );
    }));

    finish_configure_migration(fixture, &direct, device_fd);
    if let Err(payload) = pixel_assertions {
        std::panic::resume_unwind(payload);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore the overlay/COW identity guard, defer materialization, or abort on a non-overlay pinned fallback.
fn issue281_configure_border_change_migrates_materialized_direct_pixels() {
    use yserver_core::{backend::Backend, host_x11::HostSubwindowConfig};

    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::Shared, 302, 0) else {
        return;
    };
    let (old_id, source_pixels) = configure_migration_fixture(&mut fixture, &direct, true);
    let device_fd = fd(&fixture);
    let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd));
    assert_eq!(
        initial.len(),
        1,
        "one CowDescendant direct frame is accepted"
    );
    let direct_id = accepted_event_id(&initial[0]).expect("direct frame accepted");
    complete_direct_initial(&mut fixture, &initial[0], direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    let frame = fixture
        .events
        .backend
        .pstate
        .scanout_m2
        .current
        .as_ref()
        .expect("direct frame is on screen");
    assert_eq!(frame.fallback_target.backing_id(), old_id);
    assert_ne!(
        frame.fallback_target.backing_id(),
        fixture.events.backend.cow_id.unwrap()
    );

    let configure_error = Backend::configure_subwindow(
        &mut fixture.events.backend,
        None,
        direct.target_xid,
        HostSubwindowConfig {
            border_width: Some(1),
            ..HostSubwindowConfig::default()
        },
    )
    .err()
    .map(|error| error.to_string());
    assert!(fixture.events.backend.pstate.scanout_m2.unflip_requested);

    let pinned_fallback = fixture.backing_pixels_for_id(old_id);
    let replacement_id = fixture
        .events
        .backend
        .store
        .lookup(direct.target_xid)
        .expect("replacement leaf backing");
    let (replacement_width, replacement_height, replacement_pixels) = fixture
        .events
        .backend
        .backing_pixels_for_tests(direct.target_xid)
        .expect("replacement backing pixels");
    let expected_size = usize::from(fixture.events.backend.platform.fb_w)
        .checked_mul(usize::from(fixture.events.backend.platform.fb_h))
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("pixel byte count fits usize");
    let expected_source = source_pixels[..expected_size].to_vec();
    let migrated_region = (replacement_id != old_id).then(|| {
        super::support::bgra_region(
            &replacement_pixels,
            replacement_width,
            1,
            1,
            u32::from(fixture.events.backend.platform.fb_w),
            u32::from(fixture.events.backend.platform.fb_h),
        )
    });
    let replacement_shape_matches = (replacement_width, replacement_height)
        == (
            u32::from(fixture.events.backend.platform.fb_w) + 2,
            u32::from(fixture.events.backend.platform.fb_h) + 2,
        );

    let migrated_sample = migrated_region.as_deref().unwrap_or_default();
    let migration_assertions = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert!(
            configure_error.is_none(),
            "ConfigureWindow must migrate a non-overlay fallback target; error: {configure_error:?}"
        );
        assert!(
            replacement_id != old_id,
            "border change reallocates leaf backing"
        );
        assert!(
            replacement_shape_matches,
            "border replacement storage has the new extent"
        );
        assert!(
            pinned_fallback == expected_source,
            "the exact pinned non-overlay fallback receives direct pixels B; first actual/expected: {:?}/{:?}",
            &pinned_fallback[..4],
            &expected_source[..4]
        );
        assert!(
            migrated_region.as_deref() == Some(expected_source.as_slice()),
            "the replacement backing is seeded from direct pixels B; first actual/expected: {:?}/{:?}",
            &migrated_sample[..migrated_sample.len().min(4)],
            &expected_source[..4]
        );
    }));

    finish_configure_migration(fixture, &direct, device_fd);
    if let Err(payload) = migration_assertions {
        std::panic::resume_unwind(payload);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: let any ready fallback satisfy every target; the pending fallback's source-pixel assertion must fail.
fn issue281_configure_migrations_materialize_current_and_pending_fallbacks_independently() {
    use yserver_core::{backend::Backend, host_x11::HostSubwindowConfig};

    let Some((mut fixture, current)) = direct_fixture(1, P1SceneRoute::Shared, 303, 0) else {
        return;
    };
    let (current_old_id, current_pixels) =
        configure_migration_fixture(&mut fixture, &current, false);
    let device_fd = fd(&fixture);
    let current_record = accepted_records(&submit_direct(&mut fixture, &current, device_fd));
    assert_eq!(current_record.len(), 1, "current direct frame is accepted");
    let current_id = accepted_event_id(&current_record[0]).expect("current direct id");
    complete_direct_initial(&mut fixture, &current_record[0], current_id, device_fd);
    drain_present_notifications(&mut fixture, &[current.candidate.present_id], &[]);

    let pending = fixture.install_direct_present(304, 0);
    let (pending_old_id, _) = configure_migration_fixture(&mut fixture, &pending, true);
    let pending_source_color = 0x0000_55ee;
    let width = fixture.events.backend.platform.fb_w;
    let height = fixture.events.backend.platform.fb_h;
    Backend::fill_rectangle(
        &mut fixture.events.backend,
        None,
        pending.candidate.src_host_xid,
        pending_source_color,
        0,
        0,
        width,
        height,
    )
    .expect("seed distinct pending direct source through Backend");
    let pending_pixels = fixture
        .events
        .backend
        .backing_pixels_for_tests(pending.candidate.src_host_xid)
        .expect("read pending direct source")
        .2;
    let pending_offset = record_count(&fixture, device_fd);
    assert!(
        Backend::try_present_direct(
            &mut fixture.events.backend,
            pending.candidate,
            pending.event.clone(),
        )
        .expect("pending direct successor through Backend"),
        "active direct ownership admits the pending frame immediately"
    );
    let pending_record = accepted_records(&records_since(&fixture, device_fd, pending_offset));
    assert_eq!(pending_record.len(), 1, "pending direct frame is accepted");
    let pending_id = accepted_event_id(&pending_record[0]).expect("pending direct id");

    assert_ne!(current_old_id, pending_old_id);
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .current
            .as_ref()
            .expect("current direct frame remains held")
            .fallback_target
            .backing_id(),
        current_old_id
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .pending
            .as_ref()
            .expect("pending direct frame remains held")
            .fallback_target
            .backing_id(),
        pending_old_id
    );

    Backend::configure_subwindow(
        &mut fixture.events.backend,
        None,
        current.target_xid,
        HostSubwindowConfig {
            width: Some(width + 1),
            height: Some(height + 1),
            ..HostSubwindowConfig::default()
        },
    )
    .expect("migrate current fallback through a real no-background resize");
    Backend::configure_subwindow(
        &mut fixture.events.backend,
        None,
        pending.target_xid,
        HostSubwindowConfig {
            border_width: Some(1),
            ..HostSubwindowConfig::default()
        },
    )
    .expect("migrate pending non-overlay fallback through border relayout");

    let current_fallback = fixture.backing_pixels_for_id(current_old_id);
    let pending_fallback = fixture.backing_pixels_for_id(pending_old_id);
    let current_replacement = fixture
        .events
        .backend
        .backing_pixels_for_tests(current.target_xid)
        .expect("read resized current backing");
    let pending_replacement = fixture
        .events
        .backend
        .backing_pixels_for_tests(pending.target_xid)
        .expect("read bordered pending backing");
    let pixel_count = usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .expect("pixel byte count fits usize");
    let expected_current = current_pixels[..pixel_count].to_vec();
    let expected_pending = pending_pixels[..pixel_count].to_vec();
    let current_migrated = super::support::bgra_region(
        &current_replacement.2,
        current_replacement.0,
        0,
        0,
        u32::from(width),
        u32::from(height),
    );
    let pending_migrated = super::support::bgra_region(
        &pending_replacement.2,
        pending_replacement.0,
        1,
        1,
        u32::from(width),
        u32::from(height),
    );
    let pixel_assertions = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert!(
            current_fallback == expected_current,
            "the current frame's exact fallback is materialized before resize migration; first actual/expected: {:?}/{:?}",
            &current_fallback[..4],
            &expected_current[..4]
        );
        assert!(
            current_migrated == expected_current,
            "the current replacement backing preserves its direct pixels; first actual/expected: {:?}/{:?}",
            &current_migrated[..4],
            &expected_current[..4]
        );
        assert!(
            pending_fallback == expected_pending,
            "the pending frame's distinct fallback is materialized before border migration; first actual/expected: {:?}/{:?}",
            &pending_fallback[..4],
            &expected_pending[..4]
        );
        assert!(
            pending_migrated == expected_pending,
            "the pending replacement backing preserves its own direct pixels; first actual/expected: {:?}/{:?}",
            &pending_migrated[..4],
            &expected_pending[..4]
        );
    }));

    complete_direct_initial(&mut fixture, &pending_record[0], pending_id, device_fd);
    drain_present_notifications(
        &mut fixture,
        &[pending.candidate.present_id],
        &[current.candidate.present_id],
    );
    let offset = record_count(&fixture, device_fd);
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 1);
    let replacement_id = accepted_event_id(&replacement).expect("accepted unflip replacement");
    complete_recorded_attempt(&mut fixture, device_fd, &replacement, replacement_id, 96);
    assert!(
        fixture.events.backend.present_source_pins.is_empty(),
        "both direct source and fallback pins release after normal unflip retirement"
    );
    drain_present_notifications(&mut fixture, &[], &[pending.candidate.present_id]);
    fixture.finish_after_normal_drain();

    if let Err(payload) = pixel_assertions {
        std::panic::resume_unwind(payload);
    }
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
            .scanout_m2
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
            .scanout_m2
            .completed
            .is_empty()
    );
    assert!(fixture.events.backend.pstate.scanout_m2.idled.is_empty());
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
            .scanout_m2
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
            .scanout_m2
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
    assert!(fixture.events.backend.pstate.scanout_m2.pending.is_none());
    assert_eq!(fixture.events.backend.pstate.scanout_m2.completed.len(), 1);
    assert_eq!(
        fixture.events.backend.pstate.scanout_m2.completed[0].completion_clock,
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
            .scanout_m2
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
            .scanout_m2
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
    assert!(fixture.events.backend.pstate.scanout_m2.pending.is_some());
    assert_eq!(pending_ids(&fixture, device_fd, crtc), [vsync_id]);
    complete_recorded_attempt(&mut fixture, device_fd, &attempts[1], vsync_id, 71);
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_some());
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
    assert!(fixture.events.backend.pstate.scanout_m2.pending.is_none());
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_none());
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .queued_successor
            .is_none()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
    assert!(fixture.events.backend.pstate.scanout_m2.pending.is_some());
    assert_eq!(
        pending_ids(&fixture, device_fd, accepted[0].events[0].crtc),
        [accepted_id]
    );
    complete_recorded_attempt(&mut fixture, device_fd, &accepted[0], accepted_id, 81);
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_some());
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
            .scanout_m2
            .unflip_awaiting_outputs,
        HashMap::from([(0, replacement_id), (1, replacement_id)])
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
            .scanout_m2
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
            .scanout_m2
            .unflip_awaiting_outputs,
        HashMap::from([(1, replacement_id)])
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_outputs_to_replace,
        HashSet::from([1])
    );
    assert!(fixture.events.backend.pstate.scanout_m2.current.is_some());
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
            .scanout_m2
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
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    recorder.script_result(Ok(()));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    Backend::maybe_composite(&mut fixture.events.backend).expect("degrade grouped unflip");
    let rejected_group = records_since(&fixture, device_fd, offset);
    let failed_group = rejected_group
        .iter()
        .find(|record| record.events.len() == 2 && record.result != ScriptedResult::Ok)
        .expect("grouped unflip rejected");
    assert!(
        accepted_event_id(failed_group).is_none(),
        "a rejected group publishes no accepted identity"
    );
    fixture.events.wait_for_submitted_gpu_work();
    Backend::on_scanout_render_completion(&mut fixture.events.backend, &mut fixture.events.state);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("submit degraded replacements after safe notifications");
    let attempts = records_since(&fixture, device_fd, offset);
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
            .scanout_m2
            .unflip_outputs_to_replace,
        HashSet::from([0, 1])
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
            .scanout_m2
            .unflip_outputs_to_replace
            .len(),
        2
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
            .scanout_m2
            .unflip_outputs_to_replace,
        HashSet::from([waiting])
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty(),
        "no accepted replacement ID remains while one output still requires replacement"
    );
    assert!(
        fixture.events.backend.pstate.scanout_m2.current.is_some(),
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
    assert!(fixture.events.backend.pstate.scanout_m2.unflip_requested);
    assert!(fixture.events.backend.pstate.scanout_m2.pending.is_none());

    let later_offset = record_count(&fixture, device_fd);
    recorder.script_result(Ok(()));
    let later = scene_completion_for_output(&mut fixture, waiting, device_fd, later_offset);
    let later_id = accepted_event_id(&later).unwrap();
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
            .scanout_m2
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);

    let pre_drain_offset = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("repair full scene after degraded unflip");
    let repair_frames = accepted_records(&records_since(&fixture, device_fd, pre_drain_offset));
    for (index, record) in repair_frames.iter().enumerate() {
        let repair_id = accepted_event_id(record).expect("full-scene repair accepted");
        complete_recorded_attempt(
            &mut fixture,
            device_fd,
            record,
            repair_id,
            104 + u32::try_from(index).expect("repair frame index fits"),
        );
    }
    fixture.drain_deferred_pool_releases(Duration::from_secs(5));
    assert!(
        !fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .reentry_blocked_until_composed,
        "normal composed fallback reopens direct admission after every replacement retires"
    );
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
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
    fixture.readiness.as_ref().unwrap().hold_ready_jobs();
    let device_fd = fd(&fixture);
    let initial_attempts = submit_direct(&mut fixture, &direct, device_fd);
    let initial = accepted_records(&initial_attempts).remove(0);
    let direct_id = accepted_event_id(&initial).unwrap();
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);
    fixture.events.recorders[&device_fd]
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    let unflip_offset = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend).expect("prepare copied unflip");
    let source_job = fixture
        .pending_render_job_ids()
        .last()
        .copied()
        .unwrap_or_else(|| compose_copied_job(&mut fixture));
    deliver_copied_job(&mut fixture, source_job);
    let rejected_group = records_since(&fixture, device_fd, unflip_offset)
        .into_iter()
        .find(|record| record.result != ScriptedResult::Ok)
        .expect("the grouped copied replacement is rejected");
    assert!(accepted_event_id(&rejected_group).is_none());
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert_eq!(direct_source_pins(&fixture, direct.source_id), 1);

    fixture.events.wait_for_submitted_gpu_work();
    Backend::on_scanout_render_completion(&mut fixture.events.backend, &mut fixture.events.state);
    let fallback_job = compose_copied_job(&mut fixture);
    let fallback_start = record_count(&fixture, device_fd);
    deliver_copied_job(&mut fixture, fallback_job);
    let replacement = records_since(&fixture, device_fd, fallback_start)
        .into_iter()
        .find(|record| {
            record.events.len() == 1
                && record.device_fd == device_fd
                && accepted_event_id(record).is_some()
                && record.kind != crate::drm::atomic::AtomicCommitKind::Async
        })
        .expect("later degraded copied sink commit is accepted");
    let replacement_id = accepted_event_id(&replacement).unwrap();
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
            .scanout_m2
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
            .scanout_m2
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
    fixture.drain_deferred_pool_releases(Duration::from_secs(5));
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
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
    Backend::maybe_composite(&mut fixture.events.backend).expect("prepare copied unflip");
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
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty(),
        "source render readiness must not create an accepted sink identity"
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_outputs_to_replace
            .is_empty()
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

    deliver_copied_job(&mut fixture, job_id);
    let sink_attempts = records_since(&fixture, device_fd, start);
    let rejected_group = sink_attempts
        .iter()
        .find(|record| record.result != ScriptedResult::Ok)
        .expect("the ready grouped copied replacement is rejected with EIO");
    assert_eq!(
        rejected_group.events.len(),
        1,
        "the grouped rejection covers this output"
    );
    assert_eq!(rejected_group.events.len(), 1);
    assert!(accepted_records(&sink_attempts).is_empty());
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_outputs_to_replace,
        HashSet::from([0]),
        "EIO begins degraded handling only after the source is ready and sink copy is submitted"
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert_eq!(direct_source_pins(&fixture, direct.source_id), 1);
    assert_eq!(fixture.events.backend.present_source_pins.len(), 2);

    fixture.events.wait_for_submitted_gpu_work();
    Backend::on_scanout_render_completion(&mut fixture.events.backend, &mut fixture.events.state);
    let fallback_job = compose_copied_job(&mut fixture);
    let fallback_start = record_count(&fixture, device_fd);
    deliver_copied_job(&mut fixture, fallback_job);
    let accepted = records_since(&fixture, device_fd, fallback_start)
        .into_iter()
        .find(|record| record.events.len() == 1 && accepted_event_id(record).is_some())
        .expect("safe cancellation permits the per-output copied replacement");
    let accepted_id = accepted_event_id(&accepted).unwrap();
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs,
        HashMap::from([(0, accepted_id)])
    );
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(accepted_id)
    );

    complete_recorded_attempt(&mut fixture, device_fd, &accepted, accepted_id, 120);
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_outputs_to_replace
            .is_empty()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
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
    fixture.drain_deferred_pool_releases(Duration::from_secs(5));
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.finish_after_normal_drain();
}
