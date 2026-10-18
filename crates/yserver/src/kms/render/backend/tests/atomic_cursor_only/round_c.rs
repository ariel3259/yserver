use super::{round_a::damage_cursor_root, support::AtomicCursorFixture};

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: emit an ioctl for an equal accepted state or leave a retry wake after it is consumed.
fn atomic_cursor_only_equal_state_consumes_without_ioctl() {
    use yserver_core::{
        backend::Backend,
        core_loop::{HostInputEvent, InputOrigin},
    };

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor scene");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
        ],
    );

    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit scene before equal motion");
    assert!(fixture.backend.scene.has_pending_page_flips());
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 210, 95);
    Backend::maybe_composite(&mut fixture.backend).expect("queue first motion behind scene");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    Backend::maybe_composite(&mut fixture.backend).expect("coalesce motion back to accepted B");
    assert_eq!(fixture.recorders.atomic.records().len(), 3);
    fixture.retire_output(0, 3);
    assert_eq!(fixture.recorders.atomic.records().len(), 3);

    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let state = &fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc];
    assert!(state.intent.is_none(), "equal accepted state was consumed");
    assert!(state.in_flight.is_none());
    assert!(
        !fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );

    Backend::on_host_input(
        &mut fixture.backend,
        &mut fixture.state,
        HostInputEvent::PointerMotion {
            origin: InputOrigin::NestedHost,
            x: 100,
            y: 90,
            time: 4,
            relative: true,
            dx: 0,
            dy: 0,
            motion_delta: Some([0.4, -0.4]),
        },
    );
    Backend::maybe_composite(&mut fixture.backend).expect("settle fractional host motion at B");
    Backend::maybe_composite(&mut fixture.backend).expect("repeat idle scheduling at B");
    assert_eq!(fixture.recorders.atomic.records().len(), 3);
    assert!(Backend::next_wakeup(&fixture.backend).is_none());

    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle detach and output disable");
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
    );
    let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: classify EBUSY as cursor failure or retry it on each idle scheduler tick.
fn atomic_cursor_only_busy_stream_keeps_latest_intent() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor scene");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);

    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 125, 95);
    Backend::maybe_composite(&mut fixture.backend).expect("exercise cursor-only EBUSY");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), 3);
    assert_eq!(
        records[2].kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert_eq!(
        records[2].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: std::io::ErrorKind::ResourceBusy,
            raw_os_error: Some(libc::EBUSY),
        },
    );
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let device = fixture.backend.platform.device_for_key(device_key).unwrap();
    let state = &device.cursor.crtc_states[&crtc];
    assert!(
        state.applied.is_some(),
        "EBUSY retains the accepted binding"
    );
    assert!(state.intent.is_some(), "latest full intent remains queued");
    assert!(state.in_flight.is_none());
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_cursor_fallback_remaining(device_key, crtc),
        None,
    );
    assert!(
        !fixture
            .backend
            .platform
            .for_tests_cursor_permanently_disabled(device_key)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    assert!(!fixture.backend.platform.scene_cursor_hide_failed);

    Backend::maybe_composite(&mut fixture.backend).expect("idle turn does not retry EBUSY");
    Backend::maybe_composite(&mut fixture.backend).expect("second idle turn stays quiescent");
    assert_eq!(fixture.recorders.atomic.records().len(), 3);
    assert!(Backend::next_wakeup(&fixture.backend).is_none());

    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("real primary flip carries the intent");
    assert_eq!(fixture.recorders.atomic.records().len(), 4);
    assert_eq!(
        fixture.recorders.atomic.records()[3].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    fixture.retire_output(0, 3);
    assert!(
        !fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );

    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle detach and output disable");
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
    );
    let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: keep unchanged X/Y, clear a newer intent on acceptance, or omit the scene producer's delta.
fn atomic_cursor_only_all_builders_use_accepted_deltas() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor frame");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);

    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let properties = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("initialized atomic cursor plane")
        .properties_for(crtc)
        .expect("assigned cursor plane properties");
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit stable cursor scene frame");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), 3);
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "a stable cursor adds no cursor properties to a primary scene frame",
    );
    assert!(
        records[2]
            .properties
            .iter()
            .all(|update| update.object != properties.plane.into()),
        "accepted X/Y are omitted when the intent equals applied state",
    );
    fixture.retire_output(0, 3);

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 130, 100);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit motion scene frame");
    let before_retirement = fixture.recorders.atomic.records();
    assert_eq!(before_retirement.len(), 4);
    let motion_scene = &before_retirement[3];
    assert_eq!(
        motion_scene.kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    let cursor_fields: Vec<_> = motion_scene
        .properties
        .iter()
        .filter(|update| update.object == properties.plane.into())
        .collect();
    assert_eq!(cursor_fields.len(), 2, "motion changes only CRTC_X/Y");
    for (property, value) in [
        (properties.crtc_x, 130_i64 as u64),
        (properties.crtc_y, 100_i64 as u64),
    ] {
        assert!(
            cursor_fields
                .iter()
                .any(|update| { update.property == property && update.value == value })
        );
    }
    assert!(cursor_fields.iter().all(|update| {
        update.property == properties.crtc_x || update.property == properties.crtc_y
    }));

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 145, 115);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("retain newer cursor motion behind scene retirement");
    assert_eq!(fixture.recorders.atomic.records().len(), 4);
    fixture.retire_output(0, 4);
    assert_eq!(fixture.recorders.atomic.records().len(), 5);
    let newest = &fixture.recorders.atomic.records()[4];
    assert_eq!(
        newest.kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly,
        "retirement services the newer intent after the accepted scene delta",
    );
    for (property, value) in [
        (properties.crtc_x, 145_i64 as u64),
        (properties.crtc_y, 115_i64 as u64),
    ] {
        assert!(newest.properties.iter().any(|update| {
            update.object == properties.plane.into()
                && update.property == property
                && update.value == value
        }));
    }
    fixture.retire_output(0, 5);

    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle detach and output disable");
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "the final blocking lifecycle disable follows the accepted scene and cursor deltas",
    );
    let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}
