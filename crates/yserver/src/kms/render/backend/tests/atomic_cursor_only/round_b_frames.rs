use super::support::AtomicCursorFixture;

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: reuse F0's cached frame id/content receipt for F1; the exact F1 assertions fail.
fn atomic_cursor_only_frame_receipts_name_the_accepted_frame() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorFrameId, CursorPrimaryContent};
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_cursor_capabilities(None, None)
        .expect("build software cursor scene fixture")
    else {
        return;
    };
    {
        let output = &mut fixture.backend.platform.outputs[0].output;
        output.plane_fb_id_prop = ::drm::control::from_u32(71).expect("FB_ID property");
        output.plane_crtc_id_prop = ::drm::control::from_u32(72).expect("CRTC_ID property");
        output.plane_src_x_prop = ::drm::control::from_u32(73).expect("SRC_X property");
        output.plane_src_y_prop = ::drm::control::from_u32(74).expect("SRC_Y property");
        output.plane_src_w_prop = ::drm::control::from_u32(75).expect("SRC_W property");
        output.plane_src_h_prop = ::drm::control::from_u32(76).expect("SRC_H property");
        output.plane_crtc_x_prop = ::drm::control::from_u32(77).expect("CRTC_X property");
        output.plane_crtc_y_prop = ::drm::control::from_u32(78).expect("CRTC_Y property");
        output.plane_crtc_w_prop = ::drm::control::from_u32(79).expect("CRTC_W property");
        output.plane_crtc_h_prop = ::drm::control::from_u32(80).expect("CRTC_H property");
    }
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let setup = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    let (candidate, event) = super::round_a::direct_present_for_id(&setup, 20_900);
    let before_test_only = fixture.recorders.atomic.records().len();
    Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
    assert!(
        !Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("TEST_ONLY probe remains a non-presenting Backend operation"),
        "the first direct candidate performs only M1 TEST_ONLY validation"
    );
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None,
        "TEST_ONLY validation publishes no accepted cursor receipt"
    );
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_test_only,
        "TEST_ONLY emits no live atomic commit or event obligation"
    );
    let source_id = fixture
        .backend
        .store
        .lookup(setup.source_xid)
        .expect("direct source was created through Backend");
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m1
            .entries
            .get(&source_id)
            .and_then(|entry| entry.framebuffer())
            .is_some(),
        "the injected TEST_ONLY executor accepted M1"
    );
    assert!(!fixture.backend.pstate.scanout_m2.active());

    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        None,
        "no cursor receipt exists before the first accepted output frame"
    );

    let root = fixture.backend.core.window_id;
    let first_cursor = create_oversized_cursor(&mut fixture, (2, 3));
    Backend::define_cursor(&mut fixture.backend, None, root, first_cursor)
        .expect("define F0 cursor");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit F0 through Backend");

    let f0 = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted F0 cursor receipt");
    assert_eq!(f0.frame, Some(CursorFrameId(1)));
    assert_eq!(f0.mode, CursorAcceptedMode::Software);
    let CursorPrimaryContent::Software {
        pos: f0_pos,
        record_version: f0_version,
    } = f0.primary_cursor
    else {
        panic!("F0 primary contains its software cursor");
    };
    assert_eq!(f0_pos, (98, 87));
    assert_ne!(f0_version, 0);
    fixture.retire_output(0, 1);
    let f0_fb = u32::from(
        fixture
            .backend
            .platform
            .retained_composed_framebuffer(0)
            .expect("F0 is the retained primary framebuffer"),
    );

    let failed_cursor = create_oversized_cursor(&mut fixture, (3, 4));
    Backend::define_cursor(&mut fixture.backend, None, root, failed_cursor)
        .expect("define rejected cursor frame");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 115, 95);
    super::round_a::damage_cursor_root(&mut fixture);
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(5)));
    Backend::maybe_composite(&mut fixture.backend).expect("failed atomic handoff is recoverable");
    assert_eq!(
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        Some(f0),
        "a rejected handoff publishes no receipt or mode change"
    );
    assert!(
        !fixture.backend.scene.has_pending_page_flip(0),
        "a rejected handoff creates no presentation obligation"
    );
    assert!(
        fixture.backend.platform.staged_cursor_frames.is_empty(),
        "a rejected handoff releases its staged frame plan"
    );

    let cursor = create_oversized_cursor(&mut fixture, (4, 5));
    Backend::define_cursor(&mut fixture.backend, None, root, cursor).expect("define F1 cursor");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 130, 100);
    super::round_a::damage_cursor_root(&mut fixture);
    let retry_deadline = fixture
        .backend
        .scene
        .earliest_retry_deadline()
        .expect("rejected frame schedules the production retry deadline");
    while std::time::Instant::now() < retry_deadline {
        std::hint::spin_loop();
    }
    Backend::maybe_composite(&mut fixture.backend).expect("submit F1 through Backend");

    let f1 = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted F1 cursor receipt");
    assert_eq!(f1.frame, Some(CursorFrameId(3)));
    assert_eq!(f1.mode, CursorAcceptedMode::Software);
    let CursorPrimaryContent::Software {
        pos: f1_pos,
        record_version: f1_version,
    } = f1.primary_cursor
    else {
        panic!("F1 primary contains its software cursor");
    };
    assert_eq!(f1_pos, (126, 95));
    assert_ne!(f1_version, f0_version, "F1 records the replacement sprite");
    fixture.retire_output(0, 2);
    let f1_fb = u32::from(
        fixture
            .backend
            .platform
            .retained_composed_framebuffer(0)
            .expect("F1 is the retained primary framebuffer"),
    );

    let primary_fb_property = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
    let primary_plane = fixture.backend.platform.outputs[0].output.plane;
    let records = fixture.recorders.atomic.records();
    let frame_records: Vec<_> = records
        .iter()
        .filter(|record| record.events.len() == 1)
        .collect();
    assert_eq!(
        frame_records.len(),
        3,
        "F0, the rejection, and F1 have flip attempts"
    );
    let frame_fb = |index: usize| {
        frame_records[index]
            .properties
            .iter()
            .find(|update| {
                update.object == primary_plane.into() && update.property == primary_fb_property
            })
            .map(|update| update.value)
            .expect("the accepted frame records its primary framebuffer")
    };
    assert_eq!(
        frame_fb(0),
        u64::from(f0_fb),
        "F0 receipt accompanies its rendered FB: {frame_records:?}"
    );
    assert_eq!(
        frame_fb(2),
        u64::from(f1_fb),
        "F1 receipt accompanies its rendered FB: {frame_records:?}"
    );
    assert_eq!(
        frame_records[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Ok
    );
    assert!(matches!(
        frame_records[1].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            raw_os_error: Some(5),
            ..
        }
    ));
    assert_eq!(
        frame_records[2].result,
        crate::drm::atomic::tests_support::ScriptedResult::Ok
    );

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("hide cursor through Backend");
    fixture.retire_output(0, 3);
    fixture
        .backend
        .disable_output()
        .expect("disable and drain output through Backend");
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

fn create_oversized_cursor(fixture: &mut AtomicCursorFixture, hot: (u16, u16)) -> u32 {
    use yserver_core::backend::Backend;

    let source = Backend::create_pixmap(&mut fixture.backend, None, 1, 65, 65)
        .expect("create oversized cursor source");
    Backend::create_cursor(
        &mut fixture.backend,
        None,
        source,
        None,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        hot.0,
        hot.1,
    )
    .expect("create oversized software cursor")
    .as_raw()
}
