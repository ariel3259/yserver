use super::{round_a::damage_cursor_root, support::AtomicCursorFixture};

fn create_cursor_with_pixel(
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
    .expect("write cursor source through Backend");
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
// Mutation: clear every empty primary-successor delta, including one projected from a pending image replacement.
fn atomic_cursor_only_stationary_replacement_survives_primary_successor() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let setup = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor scene");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);

    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let prior = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc]
        .applied
        .expect("initial cursor binding was accepted");
    assert_eq!((prior.x, prior.y), (100, 90));

    let mut direct_entered = false;
    for attempt in 0..16_u64 {
        let (candidate, event) = super::round_a::direct_present_for_id(&setup, 21_000 + attempt);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit direct probe through Backend")
        {
            fixture.retire_output(0, 20 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend).expect("settle direct probe retirement");
            direct_entered = fixture.backend.pstate.scanout_m2.active();
            if direct_entered {
                break;
            }
        } else if fixture.backend.scene.has_pending_page_flips() {
            fixture.retire_output(0, 40 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle composed probation frame");
        }
    }
    assert!(direct_entered, "Backend probe reaches held direct scanout");

    let (candidate, event) = super::round_a::direct_present_for_id(&setup, 22_000);
    Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
    assert!(
        Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit an in-flight direct primary")
    );
    assert!(fixture.backend.pstate.scanout_m2.pending.is_some());
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    let replacement = create_cursor_with_pixel(&mut fixture, [0xde, 0xad, 0xbe, 0xff]);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("queue stationary image replacement through Backend");

    let records = fixture.recorders.atomic.records();
    let busy = records
        .last()
        .expect("replacement attempted while direct is pending");
    assert_eq!(busy.kind, crate::drm::atomic::AtomicCommitKind::CursorOnly);
    assert_eq!(
        busy.result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: std::io::ErrorKind::ResourceBusy,
            raw_os_error: Some(libc::EBUSY),
        },
    );
    let state = &fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc];
    let replacement_lease = match state.intent.expect("replacement stays pending") {
        crate::kms::render::platform::cursor_state::CursorIntent::Show(binding) => binding.lease,
        crate::kms::render::platform::cursor_state::CursorIntent::Hide => {
            panic!("stationary image replacement remains a show")
        }
    };
    assert_ne!(replacement_lease, prior.lease);

    assert!(!fixture.backend.pstate.scanout_m2.unflip_requested);
    let (candidate, event) = super::round_a::direct_present_for_id(&setup, 22_001);
    Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
    let records_before_queue = fixture.recorders.atomic.records().len();
    let _ = Backend::try_present_direct(&mut fixture.backend, candidate, event)
        .expect("queue direct primary successor behind the outstanding flip");
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_some());
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        records_before_queue
    );

    // The queued direct producer projects the accepted binding. Its empty
    // delta must not consume a different, staged replacement lease.
    fixture.retire_output(0, 50);
    Backend::maybe_composite(&mut fixture.backend).expect("submit queued direct successor");
    let after_successor = fixture.recorders.atomic.records();
    assert_ne!(
        after_successor.last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    let cursor_plane = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .unwrap()
        .properties_for(crtc)
        .unwrap()
        .plane;
    assert!(
        after_successor
            .last()
            .unwrap()
            .properties
            .iter()
            .all(|update| update.object != cursor_plane.into()),
        "direct successor projects the accepted binding and emits no cursor properties",
    );
    let state = &fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc];
    assert!(
        state.intent.is_some(),
        "direct successor keeps replacement intent"
    );
    assert_eq!(
        state.applied.map(|binding| binding.lease),
        Some(prior.lease)
    );

    fixture.retire_output(0, 51);
    let after_successor_event = fixture.recorders.atomic.records();
    assert_eq!(
        after_successor_event.last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly,
        "the direct successor event retries the retained image",
    );
    assert!(
        after_successor_event
            .last()
            .unwrap()
            .properties
            .iter()
            .any(|update| {
                update.object
                    == fixture.backend.platform.devices[0]
                        .cursor
                        .plane
                        .as_ref()
                        .unwrap()
                        .properties_for(crtc)
                        .unwrap()
                        .plane
                        .into()
                    && update.value != u64::from(prior.framebuffer)
            })
    );
    fixture.retire_output(0, 52);

    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle detach and output disable");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: reject Hide with permanently_disabled, leaving the accepted plane bound after a primary hide.
fn atomic_cursor_only_permanent_failure_still_detaches_accepted_binding() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let first = create_cursor_with_pixel(&mut fixture, [0x11, 0x22, 0x33, 0xff]);
    Backend::define_cursor(&mut fixture.backend, None, root, first.as_raw())
        .expect("show initial cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor scene");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));

    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let accepted = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc]
        .applied
        .expect("initial cursor binding was accepted");
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EOPNOTSUPP)));
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 101, 90);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("record permanent cursor motion rejection through Backend");
    assert!(
        fixture
            .backend
            .platform
            .for_tests_cursor_permanently_disabled(device_key)
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .device_for_key(device_key)
            .unwrap()
            .cursor
            .crtc_states[&crtc]
            .applied,
        Some(accepted),
        "permanent rejection keeps the accepted on-screen binding until detach acceptance",
    );

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    let records = fixture.recorders.atomic.records();
    let detach = records
        .last()
        .expect("permanent failure cleanup submits an atomic detach");
    assert_eq!(
        detach.kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    let properties = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .unwrap()
        .properties_for(crtc)
        .unwrap();
    assert!(detach.properties.iter().any(|update| {
        update.object == properties.plane.into()
            && update.property == properties.fb_id
            && update.value == 0
    }));
    assert!(detach.properties.iter().any(|update| {
        update.object == properties.plane.into()
            && update.property == properties.crtc_id
            && update.value == 0
    }));
    fixture.retire_output(0, 3);
    Backend::maybe_composite(&mut fixture.backend).expect("settle hidden software scene");
    if fixture.backend.scene.has_pending_page_flips() {
        fixture.retire_output(0, 4);
    }
    let state = &fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc];
    assert!(state.applied.is_none());
    assert!(state.in_flight.is_none());
    assert!(state.intent.is_none());

    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle output disable");
    fixture.assert_cursor_quiescent();
    let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}
