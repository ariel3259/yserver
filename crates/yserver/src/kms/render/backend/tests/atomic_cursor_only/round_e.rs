use super::{round_a::damage_cursor_root, support::AtomicCursorFixture};

fn cursor_with_pixel(
    fixture: &mut AtomicCursorFixture,
    pixel: [u8; 4],
) -> yserver_core::backend::CursorHandle {
    use yserver_core::backend::{AnyHandle, Backend};

    let source = Backend::create_pixmap(&mut fixture.backend, None, 32, 1, 1)
        .expect("create cursor source through Backend");
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        32,
        1,
        1,
        0,
        0,
        &pixel,
    )
    .expect("write cursor image through Backend");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(source),
        0,
        0,
        &[],
    )
    .expect("create cursor picture through Backend")
    .expect("cursor picture");
    Backend::render_create_cursor(&mut fixture.backend, None, picture, 0, 0)
        .expect("create cursor through Backend")
        .expect("cursor handle")
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: coalesce a newer framebuffer into pending Shows without its hotspot; both the blocked retry and later CRTC must retain one sprite identity.
fn atomic_cursor_only_deferred_direct_replacement_reaches_later_crtcs() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_two_outputs().expect("build two-output cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    let source = Backend::create_pixmap(&mut fixture.backend, None, 32, 64, 1)
        .expect("create boundary-spanning cursor source through Backend");
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        32,
        64,
        1,
        0,
        0,
        &[0x33; 64 * 4],
    )
    .expect("write boundary-spanning cursor through Backend");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        yserver_core::backend::AnyHandle::Pixmap(source),
        0,
        0,
        &[],
    )
    .expect("create cursor picture through Backend")
    .expect("cursor picture");
    let cursor = Backend::render_create_cursor(&mut fixture.backend, None, picture, 32, 0)
        .expect("create cursor through Backend")
        .expect("cursor handle");
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("define boundary-spanning cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 800, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial two-output scene");
    fixture.retire_output(0, 1);
    fixture.retire_output(1, 1);
    fixture.retire_output(0, 2);
    fixture.retire_output(1, 2);
    for output_idx in 0..2 {
        assert!(
            fixture
                .backend
                .platform
                .cursor_plane_visible_for_output(output_idx)
        );
    }
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let crtc0 = fixture.backend.platform.outputs[0].output.crtc;
    let crtc1 = fixture.backend.platform.outputs[1].output.crtc;
    let prior_leases = [crtc0, crtc1].map(|crtc| {
        fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .crtc_states[&crtc]
            .applied
            .expect("initial cursor binding accepted")
            .lease
    });

    let mut direct_entered = false;
    for attempt in 0..16_u64 {
        let (candidate, event) = super::round_a::direct_present_for_id(&direct, 24_000 + attempt);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit direct probe through Backend")
        {
            fixture.retire_output(0, 10 + u32::try_from(attempt).expect("frame fits"));
            fixture.retire_output(1, 10 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend).expect("settle direct retirement");
            if fixture.backend.pstate.scanout_m2.active() {
                direct_entered = true;
                break;
            }
        } else if fixture.backend.scene.has_pending_page_flips() {
            fixture.retire_output(0, 40 + u32::try_from(attempt).expect("frame fits"));
            fixture.retire_output(1, 40 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle composed direct-probation frame");
        }
    }
    assert!(direct_entered, "Backend direct Presents enter held scanout");

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 801, 90);
    Backend::maybe_composite(&mut fixture.backend).expect("submit two-CRTC cursor motion");
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(1)
    );
    fixture.retire_output(1, 59);
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(1)
    );

    let (candidate, event) = super::round_a::direct_present_for_id(&direct, 24_100);
    Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
    let records_before_successor = fixture.recorders.atomic.records().len();
    assert!(
        Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("retain grouped direct successor behind cursor retirement")
    );
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_some());
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        records_before_successor
    );

    let root = fixture.backend.core.window_id;
    let replacement = cursor_with_pixel(&mut fixture, [0xa1, 0xb2, 0xc3, 0xff]);
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("replace cursor through Backend while output zero is busy");

    let device = fixture.backend.platform.device_for_key(device_key).unwrap();
    let state0 = &device.cursor.crtc_states[&crtc0];
    let state1 = &device.cursor.crtc_states[&crtc1];
    let intent0 = state0
        .intent
        .expect("blocked CRTC retains replacement intent");
    let lease0 = match intent0 {
        crate::kms::render::platform::cursor_state::CursorIntent::Show(binding) => binding.lease,
        crate::kms::render::platform::cursor_state::CursorIntent::Hide => {
            panic!("visible replacement is a show on CRTC zero")
        }
    };
    let lease1 = state1
        .applied
        .expect("later CRTC accepts the replacement while CRTC zero waits")
        .lease;
    assert_eq!(lease0, lease1, "both CRTCs use the staged sprite lease");
    assert_eq!(state0.applied.unwrap().lease, prior_leases[0]);
    assert_ne!(lease1, prior_leases[1]);
    assert_eq!(
        state1.applied.unwrap().lease,
        lease1,
        "the later CRTC accepts the replacement while CRTC zero waits",
    );
    assert_eq!(
        (state0.intent.and_then(|intent| match intent {
            crate::kms::render::platform::cursor_state::CursorIntent::Show(binding) => {
                Some((binding.hot_x, binding.hot_y))
            }
            crate::kms::render::platform::cursor_state::CursorIntent::Hide => None,
        })),
        Some((0, 0)),
        "the blocked replacement keeps its new hotspot with its staged sprite lease",
    );
    assert_eq!(
        (state1.applied.unwrap().hot_x, state1.applied.unwrap().hot_y),
        (0, 0),
        "the later CRTC binds the replacement framebuffer with its own hotspot",
    );
    assert!(state1.intent.is_none());
    assert!(fixture.backend.pstate.scanout_m2.cursor_bound_all);

    fixture.retire_output(1, 60);
    fixture.retire_output(0, 61);
    Backend::maybe_composite(&mut fixture.backend).expect("submit queued direct successor");
    assert!(fixture.backend.pstate.scanout_m2.pending.is_some());
    fixture.retire_output(0, 62);
    fixture.retire_output(1, 62);
    Backend::maybe_composite(&mut fixture.backend).expect("retry first CRTC after direct event");
    for output_idx in 0..2 {
        if fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(output_idx)
        {
            fixture.retire_output(output_idx, 63);
        }
    }
    let device = fixture.backend.platform.device_for_key(device_key).unwrap();
    for crtc in [crtc0, crtc1] {
        let state = &device.cursor.crtc_states[&crtc];
        assert_eq!(state.applied.unwrap().lease, lease1);
        assert!(state.intent.is_none());
        assert!(state.in_flight.is_none());
    }

    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle detaches both cursor planes");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: omit direct or composed-unflip cursor deltas; both recorded motion-only traces must fail.
fn atomic_cursor_only_direct_and_unflip_carry_motion_deltas() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build direct cursor fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial visible cursor frame");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);

    let mut direct_entered = false;
    for attempt in 0..16_u64 {
        let (candidate, event) = super::round_a::direct_present_for_id(&direct, 25_000 + attempt);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit direct probe through Backend")
        {
            fixture.retire_output(0, 10 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend).expect("settle direct retirement");
            if fixture.backend.pstate.scanout_m2.active() {
                direct_entered = true;
                break;
            }
        } else if fixture.backend.scene.has_pending_page_flips() {
            fixture.retire_output(0, 40 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle composed direct-probation frame");
        }
    }
    assert!(direct_entered, "Backend direct Presents enter held scanout");
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let properties = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .unwrap()
        .properties_for(crtc)
        .unwrap();

    let before_direct_motion = fixture.recorders.atomic.records().len();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 130, 100);
    Backend::maybe_composite(&mut fixture.backend).expect("submit direct-held motion");
    let direct_motion = fixture.recorders.atomic.records();
    assert_eq!(direct_motion.len(), before_direct_motion + 1);
    let direct_commit = direct_motion.last().unwrap();
    assert_eq!(
        direct_commit.kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert_motion_delta(direct_commit, &properties, 130, 100, "direct-held motion");
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0),
        "the direct cursor-only event owns the motion retirement",
    );
    fixture.retire_output(0, 60);

    Backend::unmap_subwindow(&mut fixture.backend, None, direct.window_xid)
        .expect("request composed unflip through Backend window unmap");
    assert!(fixture.backend.pstate.scanout_m2.unflip_requested);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 145, 115);
    Backend::maybe_composite(&mut fixture.backend).expect("submit composed unflip with motion");
    let unflip = fixture.recorders.atomic.records();
    let unflip_commit = unflip.last().unwrap();
    assert_eq!(
        unflip_commit.kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        "the composed replacement carries the newest cursor delta",
    );
    assert_motion_delta(
        unflip_commit,
        &properties,
        145,
        115,
        "composed unflip motion",
    );
    assert_eq!(
        fixture.backend.pstate.scanout_m2.unflip_awaiting_outputs,
        [0].into()
    );
    fixture.retire_output(0, 61);
    assert!(!fixture.backend.pstate.scanout_m2.active());

    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle detaches the cursor plane");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

fn assert_motion_delta(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    properties: &crate::kms::cursor_plane::topology::CursorPlaneProperties,
    x: i32,
    y: i32,
    path: &str,
) {
    let fields: Vec<_> = record
        .properties
        .iter()
        .filter(|update| update.object == properties.plane.into())
        .collect();
    assert_eq!(fields.len(), 2, "{path} changes only CRTC_X/Y");
    assert!(fields.iter().any(|update| {
        update.property == properties.crtc_x && update.value == i64::from(x) as u64
    }));
    assert!(fields.iter().any(|update| {
        update.property == properties.crtc_y && update.value == i64::from(y) as u64
    }));
    assert!(fields.iter().all(|update| {
        update.property == properties.crtc_x || update.property == properties.crtc_y
    }));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit successful Shared-scene cursor settlement; applied X/Y must remain stale.
fn atomic_cursor_only_shared_scene_settlement_and_ebusy_through_backend() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build shared cursor fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial scene through Backend");
    fixture.retire_output(0, 1);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor binding");
    if fixture
        .backend
        .platform
        .cursor_atomic_event_pending_for_output(0)
    {
        fixture.retire_output(0, 2);
    }
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let applied = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc]
        .applied
        .expect("initial Shared cursor binding accepted");
    let plane = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .plane
        .as_ref()
        .unwrap();
    assert_eq!(plane.sprite_user_count(applied.lease), Some(1));

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 110, 92);
    damage_cursor_root(&mut fixture);
    let before_success = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend).expect("submit accepted Shared cursor scene");
    let accepted = fixture.recorders.atomic.records();
    let shared_accepts = &accepted[before_success..];
    assert!(shared_accepts.iter().any(|record| {
        record.kind == crate::drm::atomic::AtomicCommitKind::SceneWithCursor
            && record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
    }));
    let device = fixture.backend.platform.device_for_key(device_key).unwrap();
    let shared_accepted = device.cursor.crtc_states[&crtc]
        .applied
        .expect("Shared motion accepted");
    assert_eq!(shared_accepted.lease, applied.lease);
    assert_eq!(shared_accepted.x, 110);
    assert_eq!(shared_accepted.y, 92);
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    fixture.retire_output(0, 3);

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 125, 95);
    damage_cursor_root(&mut fixture);
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    let before_failure = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend).expect("record rejected Shared cursor scene");
    let failed = fixture.recorders.atomic.records();
    let attempted = &failed[before_failure..];
    assert_eq!(
        attempted.len(),
        1,
        "EBUSY does not probe or retry the producer"
    );
    assert_eq!(
        attempted[0].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    assert!(matches!(
        attempted[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            raw_os_error: Some(libc::EBUSY),
            ..
        }
    ));
    let pending = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc]
        .intent
        .expect("rejected Shared motion remains pending");
    let wanted = match pending {
        crate::kms::render::platform::cursor_state::CursorIntent::Show(binding) => binding.lease,
        crate::kms::render::platform::cursor_state::CursorIntent::Hide => {
            panic!("cursor motion keeps the show intent")
        }
    };
    assert_eq!(wanted, applied.lease);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    assert!(
        fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .crtc_states[&crtc]
            .in_flight
            .is_none()
    );

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("detach cursor through Backend");
    for frame in 4..10 {
        if fixture.backend.scene.has_pending_page_flips()
            || fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(0)
        {
            fixture.retire_output(0, frame);
        }
        Backend::maybe_composite(&mut fixture.backend).expect("drain Shared cursor teardown");
        if fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .crtc_states[&crtc]
            .applied
            .is_none()
            && !fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(0)
        {
            break;
        }
    }
    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle disable completes Shared cursor teardown");
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture.recorders.atomic.records()[..before_failure]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
