use super::{round_a::damage_cursor_root, support::AtomicCursorFixture};

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: mark a rejected cursor commit in flight; the rejection end-state assertions must fail.
// Mutation: declare a rejected detach hidden; retained HW ownership assertions must fail.
fn atomic_cursor_only_atomic_hide_preserves_ownership_on_reject() {
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
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial scene");
    fixture.retire_output(0, 1);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    fixture.retire_output(0, 2);
    let records_before_reject = fixture.recorders.atomic.records().len();
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let binding = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states
        .get(&crtc)
        .unwrap()
        .applied
        .unwrap();

    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    fixture.recorders.atomic.script_result(Ok(()));
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::ENOMEM)));
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit rejected shared detach frame");
    let rejected = fixture.recorders.atomic.records();
    let rejected = &rejected[records_before_reject..];
    let rejection = rejected
        .iter()
        .find(|record| {
            !matches!(
                record.result,
                crate::drm::atomic::tests_support::ScriptedResult::Ok
            )
        })
        .expect("scripted detach rejection is recorded");
    assert_eq!(
        rejection.kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    assert_eq!(
        rejection.result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: std::io::ErrorKind::InvalidInput,
            raw_os_error: Some(libc::EINVAL),
        },
    );
    let retry = rejected
        .iter()
        .find(|record| {
            record.kind == crate::drm::atomic::AtomicCommitKind::Scene
                && !record
                    .flags
                    .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
        })
        .expect("cursorless primary retry keeps the ordinary flip error");
    assert_eq!(
        retry.result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: std::io::ErrorKind::OutOfMemory,
            raw_os_error: Some(libc::ENOMEM),
        }
    );
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert_eq!(
        rejection.events.len(),
        1,
        "the attempted request asks for its event"
    );
    assert!(fixture.backend.effective_cursor_hidden());
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert_ne!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw,
        "a rejected detach cannot expose a software cursor beside the retained hardware cursor",
    );
    let device = fixture.backend.platform.device_for_key(device_key).unwrap();
    let state = device.cursor.crtc_states.get(&crtc).unwrap();
    assert_eq!(state.applied, Some(binding));
    assert!(state.in_flight.is_none());
    assert!(matches!(
        state.intent,
        Some(crate::kms::render::platform::cursor_state::CursorIntent::Hide)
    ));
    assert_eq!(
        device
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(binding.lease),
        Some(1)
    );

    fixture
        .backend
        .disable_output()
        .expect("blocking Backend teardown detaches the retained binding");
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    fixture.assert_cursor_quiescent();
    let state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states
        .get(&crtc)
        .unwrap();
    assert!(state.applied.is_none());
    assert!(state.in_flight.is_none());
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .filter(|record| !record
                .flags
                .contains(::drm::control::AtomicCommitFlags::TEST_ONLY))
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "the failed cursorless retry preserves ownership until blocking lifecycle teardown",
    );

    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
