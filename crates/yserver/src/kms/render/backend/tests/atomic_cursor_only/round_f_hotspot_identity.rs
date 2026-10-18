use super::{round_a::damage_cursor_root, support::AtomicCursorFixture};

fn atomic_kinds(fixture: &AtomicCursorFixture) -> Vec<crate::drm::atomic::AtomicCommitKind> {
    fixture
        .recorders
        .atomic
        .records()
        .iter()
        .map(|record| record.kind)
        .collect()
}

fn argb_cursor(
    fixture: &mut AtomicCursorFixture,
    pixel: [u8; 4],
    hot_x: u16,
    hot_y: u16,
) -> yserver_core::backend::CursorHandle {
    argb_cursor_with_extent(fixture, pixel, 2, 2, hot_x, hot_y)
}

fn argb_cursor_with_extent(
    fixture: &mut AtomicCursorFixture,
    pixel: [u8; 4],
    width: u16,
    height: u16,
    hot_x: u16,
    hot_y: u16,
) -> yserver_core::backend::CursorHandle {
    use yserver_core::backend::{AnyHandle, Backend};

    let source = Backend::create_pixmap(&mut fixture.backend, None, 32, width, height)
        .expect("create hotspot test cursor through Backend");
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&pixel);
    }
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        32,
        width,
        height,
        0,
        0,
        &pixels,
    )
    .expect("write hotspot test cursor through Backend");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(source),
        0,
        0,
        &[],
    )
    .expect("create hotspot test picture through Backend")
    .expect("cursor picture");
    Backend::render_create_cursor(&mut fixture.backend, None, picture, hot_x, hot_y)
        .expect("create hotspot test cursor through Backend")
        .expect("cursor handle")
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: accept a queued TEST_ONLY result before Backend validates the source dimensions and output shape.
fn atomic_cursor_only_m1_test_result_cannot_bypass_backend_validation() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build scanout cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let setup = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    let source_id = fixture
        .backend
        .store
        .lookup(setup.source_xid)
        .expect("direct Present source is registered through Backend");
    fixture.recorders.direct_scanout_test_only.queue_source(
        source_id,
        yserver_render::kms::render::store::ImportedDmabufMetadata {
            fourcc: u32::from_le_bytes(*b"XR24"),
            vk_format: ash::vk::Format::B8G8R8A8_UNORM,
            modifier: 0,
            implicit_layout: false,
            planes: vec![yserver_render::kms::render::store::ImportedDmabufPlane {
                offset: 0,
                pitch: u32::from(setup.width).saturating_mul(4),
            }],
            width: setup.width - 1,
            height: setup.height,
            depth: 24,
            bpp: 32,
        },
        std::fs::File::open("/dev/null")
            .expect("open synthetic probe fd")
            .into(),
    );

    let (candidate, _) = super::round_a::direct_present_for_id(&setup, 31_000);
    Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
    let entry = fixture
        .backend
        .pstate
        .scanout_m1
        .entries
        .get(&source_id)
        .expect("Backend records the completed M1 rejection");
    assert!(
        entry.framebuffer().is_none(),
        "a TEST_ONLY result cannot accept source metadata whose width disagrees with the Present",
    );
    assert_eq!(fixture.backend.pstate.scanout_m0.m1_probe_pass, 0);
    assert_eq!(fixture.backend.pstate.scanout_m0.m1_probe_reject, 1);

    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle closes the probe scenario");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    assert_eq!(
        atomic_kinds(&fixture),
        vec![crate::drm::atomic::AtomicCommitKind::Scene],
        "invalid M1 metadata emits no direct TEST_ONLY or scanout commit",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: let DPMS wake accept the output-power executor's synthetic framebuffer when the scanout pool has no backing.
fn atomic_cursor_only_dpms_wake_requires_scanout_backing() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build DPMS cursor fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("Backend DPMS disable");
    let pool = fixture.backend.platform.scanout_pools[0]
        .take()
        .expect("fixture has a backing scanout pool");
    let records_before_missing_wake = fixture.recorders.atomic.records().len();
    assert!(
        Backend::set_dpms_power(&mut fixture.backend, 0).is_err(),
        "a missing scanout framebuffer keeps the output asleep",
    );
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        records_before_missing_wake,
        "missing backing submits no wake modeset",
    );
    assert!(
        !fixture.backend.platform.active_cursor_crtcs.contains(
            &crate::kms::render::platform::CrtcKey::new(device_key, crtc)
        ),
        "failed wake keeps the CRTC inactive",
    );

    fixture.backend.platform.scanout_pools[0] = Some(pool);
    Backend::set_dpms_power(&mut fixture.backend, 0)
        .expect("Backend retries wake after restoring the real fixture pool");
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        records_before_missing_wake + 2,
        "the retry submits the modeset and the invalidated detached cursor state",
    );
    let wake_records = fixture.recorders.atomic.records();
    assert_eq!(
        wake_records[records_before_missing_wake..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
        ],
        "successful wake records the modeset before the full detached cursor commit",
    );
    assert!(
        fixture.backend.platform.active_cursor_crtcs.contains(
            &crate::kms::render::platform::CrtcKey::new(device_key, crtc)
        ),
        "successful retry restores the active CRTC",
    );
    fixture.retire_output(0, 7);
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("Backend final DPMS disable");
    fixture
        .backend
        .disable_output()
        .expect("Backend final output teardown");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    assert_eq!(
        atomic_kinds(&fixture),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "DPMS retry and teardown retain the recorded modeset/cursor commit order",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: let pointer motion rewrite a pending Show's hotspot without switching its sprite lease and framebuffer.
fn atomic_cursor_only_motion_keeps_pending_sprite_hotspot_identity() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_hotspot_properties().expect("build hotspot cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let original = argb_cursor(&mut fixture, [0x11, 0x22, 0x33, 0xff], 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, original.as_raw())
        .expect("define original cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit original cursor");
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
        .expect("initial cursor binding accepted");
    assert_eq!((prior.hot_x, prior.hot_y), (0, 0));

    let replacement = argb_cursor(&mut fixture, [0xaa, 0xbb, 0xcc, 0xff], 1, 1);
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("define replacement cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 110, 100);

    let state = &fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states[&crtc];
    let pending = match state.intent.expect("motion retains a show intent") {
        crate::kms::render::platform::cursor_state::CursorIntent::Show(binding) => binding,
        crate::kms::render::platform::cursor_state::CursorIntent::Hide => {
            panic!("visible cursor motion must retain a Show")
        }
    };
    assert_eq!(pending.lease, prior.lease);
    assert_eq!(pending.framebuffer, prior.framebuffer);
    assert_eq!(
        (pending.hot_x, pending.hot_y),
        (prior.hot_x, prior.hot_y),
        "motion uses the accepted sprite's hotspot until the replacement framebuffer is ready",
    );
    assert_eq!((pending.x, pending.y), (110, 100));

    fixture
        .backend
        .disable_output()
        .expect("Backend lifecycle detaches the cursor plane");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    assert_eq!(
        atomic_kinds(&fixture),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "motion and teardown preserve the recorded commit sequence",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit upload-time hotspot and position updates for existing pending Show intents.
fn atomic_cursor_only_upload_coalesces_pending_sprite_identity() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_two_outputs()
        .expect("build two-output hotspot cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let original = argb_cursor_with_extent(&mut fixture, [0x11, 0x22, 0x33, 0xff], 64, 1, 32, 0);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, original.as_raw())
        .expect("define original boundary cursor through Backend");
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

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 801, 90);
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor motion on both CRTCs");
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
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 802, 90);
    Backend::maybe_composite(&mut fixture.backend).expect("retain latest motion behind both flips");
    assert!(
        fixture
            .backend
            .platform
            .for_tests_cursor_show_intent(1)
            .is_some()
    );
    fixture.retire_output(0, 3);
    if fixture
        .backend
        .platform
        .cursor_commit_in_flight_for_output(0)
    {
        fixture.retire_output(0, 4);
    }
    assert!(
        !fixture
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

    let replacement = argb_cursor_with_extent(&mut fixture, [0xaa, 0xbb, 0xcc, 0xff], 64, 1, 40, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("define replacement boundary cursor through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("compose replacement cursor scene");
    assert!(fixture.backend.scene.has_pending_page_flips());
    fixture.retire_output(0, 5);

    let device_key = fixture.backend.platform.outputs[1].key.device_key;
    let crtc0 = fixture.backend.platform.outputs[0].output.crtc;
    let crtc1 = fixture.backend.platform.outputs[1].output.crtc;
    let device = fixture.backend.platform.device_for_key(device_key).unwrap();
    let output0_binding = device.cursor.crtc_states[&crtc0]
        .in_flight
        .as_ref()
        .expect("output 0 accepted the replacement cursor commit")
        .next
        .as_ref()
        .expect("replacement cursor commit binds a sprite");
    let pending = match device.cursor.crtc_states[&crtc1]
        .intent
        .expect("the in-flight second CRTC retains its cursor intent")
    {
        crate::kms::render::platform::cursor_state::CursorIntent::Show(binding) => binding,
        crate::kms::render::platform::cursor_state::CursorIntent::Hide => {
            panic!("cursor motion remains a Show while its commit is in flight")
        }
    };
    let wanted_version = fixture.backend.cursor_records[&replacement.as_raw()].version;
    let plane = device.cursor.plane.as_ref().expect("fixture cursor plane");
    assert_eq!(plane.uploaded_version(), Some(wanted_version));
    assert_eq!(pending.lease, output0_binding.lease);
    assert_eq!(pending.framebuffer, output0_binding.framebuffer);
    assert_eq!((pending.hot_x, pending.hot_y), (40, 0));
    assert_eq!((pending.x, pending.y), (-38, 90));

    for frame in 6..18 {
        Backend::maybe_composite(&mut fixture.backend)
            .expect("retry retained cursor lifecycle work");
        for output_idx in 0..2 {
            if fixture.backend.scene.has_pending_page_flip(output_idx)
                || fixture
                    .backend
                    .platform
                    .cursor_commit_in_flight_for_output(output_idx)
            {
                fixture.retire_output(output_idx, frame);
            }
        }
        if !fixture.backend.scene.has_pending_page_flips()
            && !(0..2).any(|output_idx| {
                fixture
                    .backend
                    .platform
                    .cursor_commit_in_flight_for_output(output_idx)
            })
        {
            break;
        }
    }
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert!(!(0..2).any(|output_idx| {
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(output_idx)
    }));
    fixture
        .backend
        .disable_output()
        .expect("Backend teardown detaches the replacement cursor");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    assert_eq!(
        atomic_kinds(&fixture),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "motion, upload, and teardown preserve the recorded commit sequence",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
