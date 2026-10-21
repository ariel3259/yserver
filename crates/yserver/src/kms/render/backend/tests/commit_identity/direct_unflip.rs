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
    Backend::copy_area(
        &mut fixture.events.backend,
        None,
        successor.src_host_xid,
        successor.paint_dst_host_xid,
        0,
        0,
        successor.x_off,
        successor.y_off,
        successor.src_width,
        successor.src_height,
    )
    .expect("ordinary Present Copy fallback after direct rejection");
    Backend::note_present_pixmap(
        &mut fixture.events.backend,
        successor.src_pixmap_xid,
        successor.dst_window_xid,
    );
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
    let after_all_off = fixture.events.backend.pstate.vt_state != crate::vt::state::VtState::Active;
    assert_issue281_unflip_end_state_with_lifecycle(fixture, present_id, after_all_off);
}

fn assert_issue281_unflip_end_state_after_all_off(
    fixture: &RecordingSceneFixture,
    present_id: u64,
) {
    assert_issue281_unflip_end_state_with_lifecycle(fixture, present_id, true);
}

fn assert_issue281_unflip_end_state_with_lifecycle(
    fixture: &RecordingSceneFixture,
    present_id: u64,
    after_all_off: bool,
) {
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
    if after_all_off {
        fixture.assert_quiescent_after_all_off();
    } else {
        fixture.assert_quiescent();
    }
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
// Production Present::Pixmap Copy through run_core writes C to the pinned fallback.
// Mutation: ignore unflip_shadow_ready and rematerialize the old direct scene B.
fn issue281_present_copy_preserves_newer_fallback_pixels() {
    use yserver_core::{
        backend::{Backend, WindowHandle},
        resources::{ROOT_VISUAL, ROOT_WINDOW},
        server::{BackendCapabilities, ServerState, XFixesRegion},
    };
    use yserver_protocol::x11::{
        ClientId, CreatePixmapRequest, CreateWindowRequest, ResourceId, xfixes::RegionRect,
    };

    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::Shared, 292, 0) else {
        return;
    };
    fixture.events.use_raw_event_source_for_device(0);
    let device_fd = fd(&fixture);
    let scene_b = [0x23, 0x4b, 0x68, 0xff];
    let scene_c = [0x91, 0x37, 0x4a, 0xff];
    assert_ne!(scene_b, scene_c);
    paint_solid_pixmap(&mut fixture, direct.candidate.src_pixmap_xid, scene_b);
    let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd)).remove(0);
    let direct_id = accepted_event_id(&initial).expect("direct Present accepted");
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);

    let (outputs, modes) = Backend::randr_outputs_and_modes(&mut fixture.events.backend);
    let (screen_width, screen_height) = Backend::fb_dimensions(&fixture.events.backend);
    fixture.events.state = ServerState::with_randr_outputs_and_modes(
        screen_width,
        screen_height,
        outputs,
        modes,
        BackendCapabilities::from_backend(&fixture.events.backend),
    );
    fixture
        .events
        .state
        .resources
        .window_mut(ROOT_WINDOW)
        .expect("core root")
        .host_xid = WindowHandle::from_raw(fixture.events.backend.window_id());
    let target = ResourceId(direct.target_xid);
    let width = fixture.events.backend.platform.fb_w;
    let height = fixture.events.backend.platform.fb_h;
    fixture.events.state.resources.create_window(
        ClientId(1),
        CreateWindowRequest {
            depth: 24,
            window: target,
            parent: ROOT_WINDOW,
            x: 0,
            y: 0,
            width,
            height,
            class: 1,
            visual: ROOT_VISUAL,
            ..Default::default()
        },
    );
    fixture
        .events
        .state
        .resources
        .window_mut(target)
        .expect("authoritative root-sized Present window")
        .host_xid = WindowHandle::from_raw(direct.target_xid);
    let _ = fixture.events.state.resources.map_window(target);
    let root_host_xid = fixture.events.backend.core.window_id;
    fixture
        .events
        .backend
        .windows
        .get_mut(&direct.target_xid)
        .expect("recording scene window for the Present destination")
        .parent = Some(root_host_xid);

    const PRESENT_PIXMAP: u32 = 0x0100_9201;
    let pixmap = Backend::create_pixmap(&mut fixture.events.backend, None, 24, width, height)
        .expect("allocate production Present source pixmap");
    paint_solid_pixmap(&mut fixture, pixmap.as_raw(), scene_c);
    let present_source_id = fixture
        .events
        .backend
        .store
        .lookup(pixmap.as_raw())
        .expect("production Present pixmap backing");
    let present_source_pixels = fixture.backing_pixels_for_id(present_source_id);
    assert!(
        present_source_pixels
            .chunks_exact(4)
            .all(|pixel| pixel == scene_c),
        "the source pixmap starts as distinct scene C; first pixel={:?}",
        present_source_pixels.get(..4)
    );
    fixture.events.state.resources.create_pixmap(
        ClientId(1),
        CreatePixmapRequest {
            depth: 24,
            pixmap: ResourceId(PRESENT_PIXMAP),
            drawable: target,
            width,
            height,
        },
    );
    assert!(
        fixture
            .events
            .state
            .resources
            .set_pixmap_host_xid(ResourceId(PRESENT_PIXMAP), pixmap)
    );
    const VALID_REGION: u32 = 0x0100_9202;
    fixture.events.state.xfixes_regions.insert(
        VALID_REGION,
        XFixesRegion {
            owner: ClientId(1),
            rects: vec![RegionRect {
                x: 0,
                y: 0,
                width,
                height,
            }],
        },
    );

    let fallback_id = fixture
        .events
        .backend
        .resolve_paint_target(direct.target_xid)
        .expect("the direct frame's pinned fallback target")
        .backing_id();
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .composed_unflip
            .is_none()
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_fallback_source
            .is_none()
    );
    let offset = record_count(&fixture, device_fd);
    let copy_calls_before = fixture.events.backend.engine_copy_area_calls;
    let peer = super::super::tearing::install_test_core_client(&mut fixture.events.state);
    let (peer, _, _) = super::super::tearing::run_core_present_pixmap_with_valid_region(
        &mut fixture.events.state,
        &mut fixture.events.backend,
        peer,
        1,
        direct.target_xid,
        PRESENT_PIXMAP,
        0,
        VALID_REGION,
        None,
    );
    drop(peer);
    assert!(
        fixture.events.backend.engine_copy_area_calls > copy_calls_before,
        "the production Present::Pixmap path executed a GPU CopyArea"
    );
    assert_eq!(
        fixture.events.backend.recent_present_pixmaps.back(),
        Some(&(pixmap.as_raw(), direct.target_xid)),
        "the real core Present request records the C source and pinned destination"
    );
    assert!(fixture.events.backend.pstate.scanout_m2.unflip_requested);
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_fallback_source,
        None,
        "the actual Present Copy callback clears its matching fallback source"
    );
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_shadow_ready_targets,
        HashSet::from([fallback_id]),
        "the production note_present_pixmap callback marks the pinned backing ready"
    );
    assert!(
        fixture.events.state.present_pending_exec.is_empty(),
        "the core Present Copy executed instead of remaining source-deferred"
    );
    let replacement = grouped_unflip_record(&mut fixture, device_fd, offset, 1);
    fixture.events.wait_for_submitted_gpu_work();
    let fallback_pixels = fixture.backing_pixels_for_id(fallback_id);
    assert!(
        fallback_pixels
            .chunks_exact(4)
            .all(|pixel| pixel == scene_c),
        "the flushed production Present::Pixmap Copy writes scene C; first pixel={:?}",
        fallback_pixels.get(..4)
    );
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
        .expect("grouped primary FB names its fresh target");
    let replacement_id = fixture
        .events
        .backend
        .store
        .lookup(0x6a00 + u32::try_from(replacement_idx).expect("BO index fits"))
        .expect("fresh scanout backing");
    let replacement_pixels = fixture.backing_pixels_for_id(replacement_id);
    assert!(
        replacement_pixels
            .chunks_exact(4)
            .all(|pixel| pixel == scene_c),
        "the FB named by grouped primary FB_ID also contains newer scene C; first pixel={:?}",
        replacement_pixels.get(..4)
    );
    let replacement_commit = accepted_event_id(&replacement).expect("group accepted");
    complete_recorded_attempt(
        &mut fixture,
        device_fd,
        &replacement,
        replacement_commit,
        75,
    );
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
    let mut exit_candidate = direct.candidate;
    exit_candidate.present_id += 1;
    exit_candidate.src_pixmap_xid = 0x0100_9280;
    exit_candidate.src_host_xid = 0x0100_9280;
    let mut exit_event = direct.event.clone();
    exit_event.present_id = exit_candidate.present_id;
    exit_event.serial = exit_event.serial.wrapping_add(1);
    assert!(
        !Backend::try_present_direct(&mut fixture.events.backend, exit_candidate, exit_event)
            .expect("fullscreen-exit successor without a source requests an unflip"),
        "the exit request is not admitted as another direct frame"
    );
    assert!(fixture.events.backend.pstate.scanout_m2.unflip_requested);
    assert!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .unflip_fallback_source
            .is_none(),
        "fullscreen exit has no pending Present Copy to delay its watchdog"
    );

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

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Boundary test: delay one platform prepared-plane readiness row while all
// scene work still uses the real Backend and recording atomic executor.
// Mutation: submit a group before every output's prepared row is ready.
fn issue281_shared_unflip_waits_for_every_output_readiness_boundary() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::Shared, 286, 0) else {
        return;
    };
    let device_fd = fd(&fixture);
    let gate = Rc::new(crate::kms::render::platform::RecordingPreparedUnflipReadiness::default());
    gate.hold_output(1);
    fixture
        .events
        .backend
        .platform
        .for_tests_set_prepared_unflip_readiness(gate.clone());
    let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd)).remove(0);
    let direct_id = accepted_event_id(&initial).expect("direct Present accepted");
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);

    let offset = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend).expect("prepare both shared outputs");
    assert!(
        records_since(&fixture, device_fd, offset)
            .iter()
            .all(|record| record.events.len() != 2 || accepted_event_id(record).is_none()),
        "one ready output cannot cause an early grouped replacement"
    );
    let submitted: Vec<_> = fixture.events.backend.platform.scanout_pools[..2]
        .iter()
        .enumerate()
        .map(|(output_idx, pool)| {
            let pool = pool.as_ref().expect("live scanout pool").display_pool();
            let (bo_idx, bo) = pool
                .bos
                .iter()
                .enumerate()
                .find(|(_, bo)| bo.state.phase == BoPhase::Submitted)
                .expect("unflip target remains submitted while readiness is held");
            assert_ne!(bo_idx, 0, "the retained OnScreen BO is not a render target");
            (
                output_idx,
                bo.fb_handle.expect("submitted BO has a framebuffer"),
                bo.state.in_fence_fd.map_or(u64::MAX, |raw| {
                    u64::try_from(raw).expect("owned in-fence descriptor is nonnegative")
                }),
            )
        })
        .collect();
    assert_eq!(submitted.len(), 2);

    gate.release_output(1);
    Backend::maybe_composite(&mut fixture.events.backend).expect("publish held readiness row");
    let group = records_since(&fixture, device_fd, offset)
        .into_iter()
        .find(|record| record.events.len() == 2 && accepted_event_id(record).is_some())
        .expect("one all-output group follows both readiness rows");
    for (output_idx, fb, fence) in &submitted {
        let output = &fixture.events.backend.platform.outputs[*output_idx].output;
        assert_eq!(primary_fb_value(&group, output), u32::from(*fb));
        assert_eq!(primary_in_fence_value(&group, output), *fence);
    }
    fixture.events.wait_for_submitted_gpu_work();
    let group_id = accepted_event_id(&group).expect("accepted grouped replacement");
    for output_idx in 0..2 {
        inject_output(
            &mut fixture,
            device_fd,
            &group,
            output_idx,
            group_id,
            70 + u32::try_from(output_idx).expect("output index fits"),
            Duration::from_millis(700 + output_idx as u64 * 10),
        );
    }
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.events.wait_for_submitted_gpu_work();
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Boundary test: hold copied source readiness, then hold the sink's prepared
// row at the platform boundary; mixed route still groups only after both.
// Mutation: use the source-render completion as the copied plane's final fence.
fn issue281_mixed_shared_copied_unflip_waits_for_readiness_and_uses_sink_fence() {
    let Some((mut fixture, direct)) = direct_fixture(2, P1SceneRoute::MixedSharedCopied, 287, 0)
    else {
        return;
    };
    let device_fd = fd(&fixture);
    let source_readiness = Rc::clone(fixture.readiness.as_ref().expect("copied route readiness"));
    source_readiness.hold_ready_jobs();
    let gate = Rc::new(crate::kms::render::platform::RecordingPreparedUnflipReadiness::default());
    gate.hold_output(1);
    fixture
        .events
        .backend
        .platform
        .for_tests_set_prepared_unflip_readiness(gate.clone());
    let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd)).remove(0);
    let direct_id = accepted_event_id(&initial).expect("direct Present accepted");
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);

    let offset = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend).expect("start mixed preparation");
    let job_id = fixture
        .pending_render_job_ids()
        .last()
        .copied()
        .expect("copied output registered source-render readiness");
    assert!(
        records_since(&fixture, device_fd, offset)
            .iter()
            .all(|record| record.events.len() != 2 || accepted_event_id(record).is_none()),
        "mixed group waits for copied source readiness"
    );
    source_readiness.capture_completion_fence(job_id);
    deliver_copied_job(&mut fixture, job_id);
    assert!(
        records_since(&fixture, device_fd, offset)
            .iter()
            .all(|record| record.events.len() != 2 || accepted_event_id(record).is_none()),
        "sink preparation still waits for the copied output's held readiness row"
    );
    let copied_pool = fixture.events.backend.platform.scanout_pools[1]
        .as_ref()
        .expect("copied output")
        .display_pool();
    let (copied_bo_idx, copied_bo) = copied_pool
        .bos
        .iter()
        .enumerate()
        .find(|(_, bo)| bo.state.phase == BoPhase::Submitted)
        .expect("sink copy retains its submitted destination");
    let sink_fence = copied_bo.state.in_fence_fd;
    let source_fence = source_readiness
        .take_captured_completion_fence(job_id)
        .expect("capture source fence while its original fd is live");
    let source_fence_raw = std::os::fd::AsRawFd::as_raw_fd(&source_fence);
    assert_ne!(
        source_fence_raw, -1,
        "source completion has its own live fd"
    );
    let copied_fb = u32::from(copied_bo.fb_handle.expect("sink BO has FB_ID"));

    gate.release_output(1);
    Backend::maybe_composite(&mut fixture.events.backend).expect("publish copied sink readiness");
    let group = records_since(&fixture, device_fd, offset)
        .into_iter()
        .find(|record| record.events.len() == 2 && accepted_event_id(record).is_some())
        .expect("mixed outputs submit in one accepted group");
    let copied_output = &fixture.events.backend.platform.outputs[1].output;
    assert_eq!(primary_fb_value(&group, copied_output), copied_fb);
    let recorded_sink_fence = primary_in_fence_value(&group, copied_output);
    match sink_fence {
        Some(sink_fence) => {
            assert_ne!(
                source_fence_raw, sink_fence,
                "the source completion descriptor remains live while the sink fence is exported"
            );
            assert_eq!(
                recorded_sink_fence,
                u64::try_from(sink_fence).expect("sink fence descriptor is nonnegative"),
                "the grouped copied plane carries the sink-copy completion"
            );
        }
        None => assert_eq!(
            recorded_sink_fence,
            u64::MAX,
            "an already-signalled sink copy is encoded as sign-extended -1, never the source fd"
        ),
    }
    assert_ne!(copied_bo_idx, 0, "the retained destination is not selected");
    fixture.events.wait_for_submitted_gpu_work();
    let group_id = accepted_event_id(&group).expect("accepted grouped replacement");
    for output_idx in 0..2 {
        inject_output(
            &mut fixture,
            device_fd,
            &group,
            output_idx,
            group_id,
            72 + u32::try_from(output_idx).expect("output index fits"),
            Duration::from_millis(720 + output_idx as u64 * 10),
        );
    }
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.events.wait_for_submitted_gpu_work();
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Boundary test: hold the copied source notification and then the sink plane
// readiness row; retain source and sink fence handles only while descriptors
// are live. Mutation: submit the copied destination before sink readiness.
fn issue281_copied_only_unflip_uses_sink_dependency_and_signalled_sentinel() {
    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::CopiedSameDevice, 289, 0)
    else {
        return;
    };
    let device_fd = fd(&fixture);
    let source_readiness = Rc::clone(fixture.readiness.as_ref().expect("copied readiness"));
    source_readiness.hold_ready_jobs();
    let gate = Rc::new(crate::kms::render::platform::RecordingPreparedUnflipReadiness::default());
    gate.hold_output(0);
    fixture
        .events
        .backend
        .platform
        .for_tests_set_prepared_unflip_readiness(gate.clone());
    let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd)).remove(0);
    let direct_id = accepted_event_id(&initial).expect("direct Present accepted");
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);

    let offset = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend).expect("start copied preparation");
    let job_id = fixture
        .pending_render_job_ids()
        .last()
        .copied()
        .expect("copied source render is pending");
    assert!(
        records_since(&fixture, device_fd, offset)
            .iter()
            .all(|record| record.events.is_empty()),
        "a copied source-render handoff has no flip obligation"
    );
    source_readiness.capture_completion_fence(job_id);
    deliver_copied_job(&mut fixture, job_id);
    let source_fence = source_readiness
        .take_captured_completion_fence(job_id)
        .expect("duplicate source fence while its original descriptor is live");
    let source_fence_raw = std::os::fd::AsRawFd::as_raw_fd(&source_fence);
    let mut source_poll = libc::pollfd {
        fd: source_fence_raw,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: the duplicated source sync-file remains owned by `source_fence`.
    assert_eq!(unsafe { libc::poll(&mut source_poll, 1, 0) }, 1);
    assert_ne!(
        source_poll.revents & libc::POLLIN,
        0,
        "source render is ready"
    );
    assert!(
        records_since(&fixture, device_fd, offset)
            .iter()
            .all(|record| accepted_event_id(record).is_none()),
        "source readiness alone does not submit the copied destination"
    );
    let (bo_idx, bo) = fixture.events.backend.platform.scanout_pools[0]
        .as_ref()
        .expect("copied output")
        .display_pool()
        .bos
        .iter()
        .enumerate()
        .find(|(_, bo)| bo.state.phase == BoPhase::Submitted)
        .expect("sink copy destination remains submitted at the readiness boundary");
    let sink_fence = bo.state.in_fence_fd;
    let fb = u32::from(bo.fb_handle.expect("copied destination has FB_ID"));
    let retained_sink_fence = sink_fence.map(|raw| {
        // SAFETY: the Submitted BO owns this final fence until group acceptance.
        unsafe { std::os::fd::BorrowedFd::borrow_raw(raw) }
            .try_clone_to_owned()
            .expect("duplicate sink fence while its original descriptor is live")
    });

    gate.release_output(0);
    Backend::maybe_composite(&mut fixture.events.backend).expect("publish copied sink readiness");
    let group = records_since(&fixture, device_fd, offset)
        .into_iter()
        .find(|record| record.events.len() == 1 && accepted_event_id(record).is_some())
        .expect("copied-only replacement is accepted after sink readiness");
    let output = &fixture.events.backend.platform.outputs[0].output;
    assert_eq!(primary_fb_value(&group, output), fb);
    match (sink_fence, retained_sink_fence.as_ref()) {
        (Some(raw), Some(_duplicate)) => {
            assert_ne!(source_fence_raw, raw);
            assert_eq!(
                primary_in_fence_value(&group, output),
                u64::try_from(raw).expect("sink dependency fd is nonnegative"),
                "the primary plane receives the sink-copy dependency"
            );
        }
        (None, None) => assert_eq!(
            primary_in_fence_value(&group, output),
            u64::MAX,
            "None is already signalled and uses sign-extended -1"
        ),
        _ => panic!("sink fence duplicate must match the live BO-owned dependency"),
    }
    assert_ne!(bo_idx, 0, "the retained destination is not selected");
    fixture.events.wait_for_submitted_gpu_work();
    if let Some(duplicate) = retained_sink_fence.as_ref() {
        let mut sink_poll = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(duplicate),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: the retained duplicate stays owned through the readiness check.
        assert_eq!(unsafe { libc::poll(&mut sink_poll, 1, 0) }, 1);
        assert_ne!(sink_poll.revents & libc::POLLIN, 0, "sink copy is complete");
    }
    let group_id = accepted_event_id(&group).expect("accepted copied replacement");
    inject_output(
        &mut fixture,
        device_fd,
        &group,
        0,
        group_id,
        73,
        Duration::from_millis(730),
    );
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Boundary test: the shared subcase releases a held platform row only at its
// deadline; the copied subcase replaces readiness with an unsignalled eventfd
// after the first safe retry. Mutation: route the second deadline through
// wait_idle_bounded or treat its timeout as writer safety.
fn issue281_unflip_prepare_timeout_recovers_lost_and_cancelled_readiness() {
    let Some((mut lost, lost_direct)) = direct_fixture(1, P1SceneRoute::Shared, 290, 0) else {
        return;
    };
    let lost_fd = fd(&lost);
    let lost_clock = Rc::new(Cell::new(Instant::now()));
    let test_clock = Rc::clone(&lost_clock);
    lost.events
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || test_clock.get()));
    let gate = Rc::new(crate::kms::render::platform::RecordingPreparedUnflipReadiness::default());
    gate.hold_output(0);
    lost.events
        .backend
        .platform
        .for_tests_set_prepared_unflip_readiness(gate.clone());
    let initial = accepted_records(&submit_direct(&mut lost, &lost_direct, lost_fd)).remove(0);
    let direct_id = accepted_event_id(&initial).expect("direct Present accepted");
    complete_direct_initial(&mut lost, &initial, direct_id, lost_fd);
    drain_present_notifications(&mut lost, &[lost_direct.candidate.present_id], &[]);
    request_unflip(&mut lost, &lost_direct);
    let offset = record_count(&lost, lost_fd);
    Backend::maybe_composite(&mut lost.events.backend).expect("prepare shared lost notification");
    let lost_deadline = lost
        .events
        .backend
        .pstate
        .scanout_m2
        .composed_unflip
        .as_ref()
        .and_then(|batch| batch.prepare_deadline)
        .expect("first prepare deadline is armed");
    assert_eq!(
        Backend::next_wakeup(&lost.events.backend),
        Some(lost_deadline)
    );
    gate.release_output(0);
    lost_clock.set(lost_deadline);
    Backend::maybe_composite(&mut lost.events.backend).expect("recheck the prepared-plane ledger");
    let lost_group = records_since(&lost, lost_fd, offset)
        .into_iter()
        .find(|record| record.events.len() == 1 && accepted_event_id(record).is_some())
        .expect("one deadline recheck submits the already-complete exact plane");
    assert_eq!(
        records_since(&lost, lost_fd, offset)
            .iter()
            .filter(|record| record.events.len() == 1 && accepted_event_id(record).is_some())
            .count(),
        1,
        "the timeout recheck does not duplicate rendering or grouped submission"
    );
    let lost_group_id = accepted_event_id(&lost_group).expect("accepted deadline group");
    lost.events.wait_for_submitted_gpu_work();
    inject_output(
        &mut lost,
        lost_fd,
        &lost_group,
        0,
        lost_group_id,
        74,
        Duration::from_millis(740),
    );
    drain_present_notifications(&mut lost, &[], &[lost_direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&lost, lost_direct.candidate.present_id);
    lost.finish_after_normal_drain();

    let Some((mut cancelled, cancelled_direct)) =
        direct_fixture(1, P1SceneRoute::CopiedSameDevice, 291, 0)
    else {
        return;
    };
    let cancelled_fd = fd(&cancelled);
    let cancelled_clock = Rc::new(Cell::new(Instant::now()));
    let test_clock = Rc::clone(&cancelled_clock);
    cancelled
        .events
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || test_clock.get()));
    let source_readiness = Rc::clone(
        cancelled
            .readiness
            .as_ref()
            .expect("copied route readiness"),
    );
    source_readiness.hold_ready_jobs();
    let initial = accepted_records(&submit_direct(
        &mut cancelled,
        &cancelled_direct,
        cancelled_fd,
    ))
    .remove(0);
    let cancelled_direct_id = accepted_event_id(&initial).expect("direct Present accepted");
    complete_direct_initial(&mut cancelled, &initial, cancelled_direct_id, cancelled_fd);
    drain_present_notifications(
        &mut cancelled,
        &[cancelled_direct.candidate.present_id],
        &[],
    );
    request_unflip(&mut cancelled, &cancelled_direct);
    Backend::maybe_composite(&mut cancelled.events.backend)
        .expect("start first copied prepare attempt");
    let first_job = cancelled
        .pending_render_job_ids()
        .last()
        .copied()
        .expect("first copied prepare attempt has a source job");
    let first_deadline = cancelled
        .events
        .backend
        .pstate
        .scanout_m2
        .composed_unflip
        .as_ref()
        .and_then(|batch| batch.prepare_deadline)
        .expect("first copied prepare deadline is armed");
    cancelled_clock.set(first_deadline);
    Backend::maybe_composite(&mut cancelled.events.backend)
        .expect("cancel only dependency-safe first attempt work");

    let retry_job = cancelled
        .pending_render_job_ids()
        .last()
        .copied()
        .expect("one full-repaint retry starts after safe cancellation");
    assert_ne!(
        retry_job, first_job,
        "retry receives a new render job identity"
    );
    assert_eq!(
        cancelled
            .events
            .backend
            .pstate
            .scanout_m2
            .composed_unflip
            .as_ref()
            .expect("retry batch remains active")
            .prepare_attempt,
        2,
        "there is one bounded retry"
    );

    let eventfd: std::os::fd::OwnedFd = nix::sys::eventfd::EventFd::from_value_and_flags(
        0,
        nix::sys::eventfd::EfdFlags::EFD_CLOEXEC | nix::sys::eventfd::EfdFlags::EFD_NONBLOCK,
    )
    .expect("create test-controlled unsignalled readiness source")
    .into();
    let witness = eventfd
        .try_clone()
        .expect("retain a live handle to the test readiness source");
    let replaced = cancelled
        .events
        .backend
        .platform
        .for_tests_replace_pending_render_completion_fd(retry_job, eventfd)
        .expect("replace the pending source fd at the platform readiness boundary");
    drop(replaced);
    let mut witness_poll = libc::pollfd {
        fd: std::os::fd::AsRawFd::as_raw_fd(&witness),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `witness` keeps the test-controlled readiness source live.
    assert_eq!(unsafe { libc::poll(&mut witness_poll, 1, 0) }, 0);

    let second_deadline = cancelled
        .events
        .backend
        .pstate
        .scanout_m2
        .composed_unflip
        .as_ref()
        .and_then(|batch| batch.prepare_deadline)
        .expect("second prepare deadline is armed");
    cancelled_clock.set(second_deadline);
    let second_elapsed = Instant::now();
    Backend::maybe_composite(&mut cancelled.events.backend)
        .expect("second timeout polls only the batch-owned readiness source");
    assert!(
        second_elapsed.elapsed() <= Duration::from_millis(1500),
        "second deadline returns after its at-most-one-second dependency poll"
    );
    assert!(matches!(
        cancelled
            .events
            .backend
            .pstate
            .scanout_m2
            .composed_unflip
            .as_ref()
            .expect("writer-unsafe batch remains active")
            .phase,
        crate::kms::render::backend::ComposedUnflipPhase::Quarantined
    ));
    assert_eq!(
        direct_source_pins(&cancelled, cancelled_direct.source_id),
        1
    );
    assert_eq!(cancelled.events.backend.present_source_pins.len(), 2);
    let quarantined_wakeup = Backend::next_wakeup(&cancelled.events.backend)
        .expect("quarantine rearms one deadline without polling");
    assert!(quarantined_wakeup > cancelled_clock.get());
    let responsive_elapsed = Instant::now();
    Backend::maybe_composite(&mut cancelled.events.backend)
        .expect("quarantined Backend entry returns before its one-shot deadline");
    assert!(responsive_elapsed.elapsed() < Duration::from_millis(250));
    assert_eq!(
        direct_source_pins(&cancelled, cancelled_direct.source_id),
        1
    );

    // The old topology's complete all-off and device-quiescence boundary
    // safely reclaims the quarantined target and releases direct ownership.
    cancelled.events.wait_for_submitted_gpu_work();
    cancelled
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    cancelled.events.backend.drive_vt_event(
        &mut cancelled.events.state,
        crate::vt::state::VtEventKind::Disable,
    );
    drain_present_notifications(
        &mut cancelled,
        &[],
        &[cancelled_direct.candidate.present_id],
    );
    assert_issue281_unflip_end_state(&cancelled, cancelled_direct.candidate.present_id);
    cancelled.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: recover a deferred copied submit synchronously or publish a fake
// final-fence notification after export fails.
fn issue281_copied_sink_export_failure_quarantines_without_wait() {
    let Some((mut fixture, direct)) = direct_fixture(1, P1SceneRoute::CopiedSameDevice, 288, 0)
    else {
        return;
    };
    let device_fd = fd(&fixture);
    let readiness = Rc::clone(fixture.readiness.as_ref().expect("copied route readiness"));
    readiness.hold_ready_jobs();
    let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd)).remove(0);
    let direct_id = accepted_event_id(&initial).expect("direct Present accepted");
    complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
    drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
    request_unflip(&mut fixture, &direct);
    let offset = record_count(&fixture, device_fd);
    Backend::maybe_composite(&mut fixture.events.backend).expect("submit copied source render");
    let job_id = fixture
        .pending_render_job_ids()
        .last()
        .copied()
        .expect("copied unflip source render is pending");
    readiness.release_ready_job(job_id);
    let export_failure =
        std::sync::Arc::new(crate::kms::vk::scanout::RecordingSyncFileExporter::default());
    export_failure.fail_next_export();
    let Some(crate::kms::vk::scanout::OutputScanout::Copied(copied)) =
        fixture.events.backend.platform.scanout_pools[0].as_mut()
    else {
        panic!("the fixture output must use the copied route");
    };
    for bo in &mut copied.destinations.bos {
        bo.sync_file_exporter = export_failure.clone();
    }
    let began = Instant::now();
    Backend::on_scanout_render_completion(&mut fixture.events.backend, &mut fixture.events.state);
    assert_eq!(
        export_failure.failure_count(),
        1,
        "the injected descriptor-exhaustion result occurs at final sync-file export"
    );
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "the Backend returns after queue submission and failed final-fence export"
    );
    assert!(
        records_since(&fixture, device_fd, offset)
            .iter()
            .all(|record| accepted_event_id(record).is_none()),
        "no group or per-output commit is attempted without a final dependency"
    );
    assert!(matches!(
        fixture
            .events
            .backend
            .pstate
            .scanout_m2
            .composed_unflip
            .as_ref()
            .expect("quarantined batch remains active")
            .phase,
        crate::kms::render::backend::ComposedUnflipPhase::Quarantined
    ));
    let identity = fixture
        .events
        .backend
        .scene
        .pending_unflip_frame_identities()
        .into_iter()
        .next()
        .expect("quarantined scene frame remains pending");
    assert!(fixture.events.backend.scene.unflip_frame_is_quarantined(
        identity.output_idx,
        identity.bo_idx,
        identity.render_job_id,
    ));
    let pool = fixture.events.backend.platform.scanout_pools[0]
        .as_ref()
        .expect("copied output")
        .display_pool();
    assert_eq!(
        pool.bos[identity.bo_idx].state.phase,
        BoPhase::Submitted,
        "the possibly-writing destination stays unavailable"
    );
    assert_eq!(direct_source_pins(&fixture, direct.source_id), 1);
    assert_eq!(fixture.events.backend.present_source_pins.len(), 2);

    // The production VT boundary performs all-output disable, event drain,
    // scene cancellation, and device quiescence before releasing quarantined
    // copied resources and direct pins.
    fixture.events.wait_for_submitted_gpu_work();
    fixture
        .events
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.events.backend.drive_vt_event(
        &mut fixture.events.state,
        crate::vt::state::VtEventKind::Disable,
    );
    drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
    assert_issue281_unflip_end_state(&fixture, direct.candidate.present_id);
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Boundary test: keep preparation readiness held while each production lifecycle boundary quiesces it.
// Mutation: allow a prepared unflip row or direct pin to survive a VT, DPMS, or RandR all-off transition.
fn issue281_unflip_lifecycle_quiesces_vt_dpms_and_randr() {
    use yserver_core::backend::Backend;

    #[derive(Clone, Copy)]
    enum Lifecycle {
        Vt,
        Dpms,
        Randr,
    }

    for lifecycle in [Lifecycle::Vt, Lifecycle::Dpms, Lifecycle::Randr] {
        let output_count = if matches!(lifecycle, Lifecycle::Randr) {
            2
        } else {
            1
        };
        let present_id = match lifecycle {
            Lifecycle::Vt => 293,
            Lifecycle::Dpms => 294,
            Lifecycle::Randr => 295,
        };
        let Some((mut fixture, direct)) =
            direct_fixture(output_count, P1SceneRoute::Shared, present_id, 0)
        else {
            println!(
                "SKIP issue281_unflip_lifecycle_quiesces_vt_dpms_and_randr: no Vulkan test device"
            );
            return;
        };
        let device_fd = fd(&fixture);
        let gate =
            Rc::new(crate::kms::render::platform::RecordingPreparedUnflipReadiness::default());
        for output_idx in 0..output_count {
            gate.hold_output(output_idx);
        }
        fixture
            .events
            .backend
            .platform
            .for_tests_set_prepared_unflip_readiness(gate.clone());
        let initial = accepted_records(&submit_direct(&mut fixture, &direct, device_fd)).remove(0);
        let direct_id = accepted_event_id(&initial).expect("direct Present accepted");
        complete_direct_initial(&mut fixture, &initial, direct_id, device_fd);
        drain_present_notifications(&mut fixture, &[direct.candidate.present_id], &[]);
        request_unflip(&mut fixture, &direct);
        Backend::maybe_composite(&mut fixture.events.backend)
            .expect("start held unflip preparation before lifecycle transition");
        assert!(
            fixture
                .events
                .backend
                .pstate
                .scanout_m2
                .composed_unflip
                .is_some(),
            "the lifecycle begins with an active held preparation"
        );
        match lifecycle {
            Lifecycle::Vt => {
                fixture
                    .events
                    .backend
                    .platform
                    .for_tests_accept_output_power_commits();
                fixture.events.backend.drive_vt_event(
                    &mut fixture.events.state,
                    crate::vt::state::VtEventKind::Disable,
                );
            }
            Lifecycle::Dpms => {
                fixture
                    .events
                    .backend
                    .platform
                    .for_tests_accept_output_power_commits();
                Backend::set_dpms_power(&mut fixture.events.backend, 3)
                    .expect("DPMS off quiesces held unflip preparation");
            }
            Lifecycle::Randr => {
                let (outputs, modes) =
                    Backend::randr_outputs_and_modes(&mut fixture.events.backend);
                let (screen_width, screen_height) = Backend::fb_dimensions(&fixture.events.backend);
                fixture.events.state =
                    yserver_core::server::ServerState::with_randr_outputs_and_modes(
                        screen_width,
                        screen_height,
                        outputs.clone(),
                        modes,
                        yserver_core::server::BackendCapabilities::from_backend(
                            &fixture.events.backend,
                        ),
                    );
                let output = outputs.first().expect("RandR output exists");
                fixture
                    .events
                    .backend
                    .platform
                    .for_tests_accept_output_power_commits();
                fixture.events.backend.kms_outputs_active = false;
                assert!(
                    Backend::apply_crtc_config(
                        &mut fixture.events.backend,
                        output.output_id,
                        &output.name,
                        None,
                        0,
                        0,
                    )
                    .expect("RandR disable quiesces held unflip preparation")
                );
            }
        }
        drain_present_notifications(&mut fixture, &[], &[direct.candidate.present_id]);
        assert_issue281_unflip_end_state_after_all_off(&fixture, direct.candidate.present_id);
        fixture.finish();
    }
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
