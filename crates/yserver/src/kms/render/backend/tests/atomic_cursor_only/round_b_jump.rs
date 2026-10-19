use super::support::AtomicCursorFixture;

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: capture P0 in pending cursor-transition data and replay it at retirement, or omit FB_ID from the window-enter commit; either schedule must fail.
fn atomic_cursor_only_window_enter_never_restores_p0() {
    use crate::{
        drm::atomic::{AtomicCommitKind, tests_support::ScriptedResult},
        kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent},
    };
    use yserver_core::backend::Backend;
    use yserver_protocol::x11::ResourceId;

    for motion_before_acceptance in [true, false] {
        let Some(mut fixture) =
            AtomicCursorFixture::new().expect("build window-entry cursor fixture")
        else {
            return;
        };
        let sink_atomic = super::round_b_frames::install_copied_recording_route(&mut fixture);
        fixture
            .recorders
            .render_completion_readiness
            .hold_ready_jobs();
        let cursor_properties = {
            let output = &fixture.backend.platform.outputs[0];
            fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.plane.as_ref())
                .and_then(|plane| plane.properties_for(output.output.crtc))
                .expect("copied sink cursor properties")
        };

        let root_resource = yserver_core::resources::ROOT_WINDOW;
        let window_a = ResourceId(0x00b7_0001);
        let window_b = ResourceId(0x00b7_0002);
        let host_a = super::super::create_live_window(
            &mut fixture.state,
            &mut fixture.backend,
            window_a,
            root_resource,
            0,
            0,
            100,
            100,
        );
        let host_b = super::super::create_live_window(
            &mut fixture.state,
            &mut fixture.backend,
            window_b,
            root_resource,
            100,
            60,
            300,
            250,
        );

        let cursor_a = argb_cursor(&mut fixture, [0x10, 0x20, 0xf0, 0xff], 2, 3);
        let cursor_b = argb_cursor(&mut fixture, [0xe0, 0x30, 0x10, 0xff], 7, 9);
        let cursor_a_version = fixture.backend.cursor_records[&cursor_a.as_raw()].version;
        let cursor_b_version = fixture.backend.cursor_records[&cursor_b.as_raw()].version;
        assert_ne!(cursor_a_version, cursor_b_version);
        Backend::define_cursor(
            &mut fixture.backend,
            None,
            host_a.as_raw(),
            cursor_a.as_raw(),
        )
        .expect("define first window cursor through Backend");
        Backend::define_cursor(
            &mut fixture.backend,
            None,
            host_b.as_raw(),
            cursor_b.as_raw(),
        )
        .expect("define second window cursor through Backend");
        assert_ne!(cursor_a, cursor_b);

        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 40, 40);
        super::round_a::damage_cursor_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.backend).expect("submit initial accepted frame");
        complete_copied_frame(&mut fixture);
        let first_receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("initial frame has an accepted cursor receipt");
        assert_eq!(first_receipt.mode, CursorAcceptedMode::Hardware);
        assert_eq!(
            first_receipt.primary_cursor,
            CursorPrimaryContent::Cursorless
        );
        assert_eq!(
            fixture.backend.effective_cursor_xid,
            Some(cursor_a.as_raw())
        );
        let initial_records = sink_atomic.records();
        assert_eq!(initial_records.len(), 1);
        assert_eq!(initial_records[0].kind, AtomicCommitKind::SceneWithCursor);
        assert_eq!(initial_records[0].result, ScriptedResult::Ok);
        assert_eq!(
            cursor_position(&initial_records[0], cursor_properties),
            Some((38, 37)),
            "the accepted first cursor uses P=(40,40) minus A's (2,3) hotspot",
        );
        let initial_fb = cursor_framebuffer(&initial_records[0], cursor_properties)
            .expect("initial accepted cursor framebuffer");
        fixture.retire_output(0, 1);
        assert!(!fixture.backend.scene.has_pending_page_flip(0));

        let p0 = (120, 80);
        let p1 = (180, 130);
        let before_enter = (
            fixture.backend.effective_cursor_xid,
            fixture
                .backend
                .effective_cursor_xid
                .and_then(|xid| fixture.backend.cursor_records.get(&xid))
                .map(|record| record.version),
        );
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p0.0, p0.1);
        assert_eq!(
            fixture.backend.effective_cursor_xid,
            Some(cursor_b.as_raw())
        );
        let after_enter = (
            fixture.backend.effective_cursor_xid,
            fixture
                .backend
                .effective_cursor_xid
                .and_then(|xid| fixture.backend.cursor_records.get(&xid))
                .map(|record| record.version),
        );
        assert_ne!(
            before_enter, after_enter,
            "window entry changes cursor XID/version"
        );
        super::round_a::damage_cursor_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.backend).expect("start copied window-entry frame");
        let pending = fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_ids();
        assert_eq!(pending.len(), 1, "the copied frame waits before KMS submit");
        assert_eq!(sink_atomic.records().len(), 1);

        if !motion_before_acceptance {
            complete_copied_frame(&mut fixture);
            let accepted = fixture
                .backend
                .scene
                .for_tests_cursor_commit_receipt(0)
                .expect("copied window-entry frame is accepted");
            assert_eq!(accepted.mode, CursorAcceptedMode::Hardware);
            assert_eq!(accepted.primary_cursor, CursorPrimaryContent::Cursorless);
            let accepted_records = sink_atomic.records();
            assert_eq!(accepted_records.len(), 2);
            let record = accepted_records
                .last()
                .expect("accepted copied frame reaches recording atomic sink");
            assert_eq!(record.kind, AtomicCommitKind::SceneWithCursor);
            assert_eq!(record.result, ScriptedResult::Ok);
            assert_eq!(cursor_position(record, cursor_properties), Some((113, 71)));
            assert_window_enter_b_binding(
                &fixture,
                record,
                cursor_properties,
                initial_fb,
                cursor_b.as_raw(),
                cursor_b_version,
                (7, 9),
            );
            assert!(fixture.backend.scene.has_pending_page_flip(0));
        }

        let atomic_start = sink_atomic.records().len();
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, p1.0, p1.1);
        assert_eq!(
            (
                fixture.backend.core.cursor_x as i32,
                fixture.backend.core.cursor_y as i32
            ),
            p1
        );
        assert_eq!(
            sink_atomic.records().len(),
            atomic_start,
            "motion waits behind the accepted primary cursor obligation",
        );
        if motion_before_acceptance {
            complete_copied_frame(&mut fixture);
            let accepted = fixture
                .backend
                .scene
                .for_tests_cursor_commit_receipt(0)
                .expect("copied frame accepts after the in-window motion");
            assert_eq!(accepted.mode, CursorAcceptedMode::Hardware);
            assert_eq!(accepted.primary_cursor, CursorPrimaryContent::Cursorless);
            assert!(fixture.backend.scene.has_pending_page_flip(0));
            let records = sink_atomic.records();
            assert_eq!(records.len(), 2);
            assert_eq!(records[1].kind, AtomicCommitKind::SceneWithCursor);
            assert_eq!(records[1].result, ScriptedResult::Ok);
            assert_eq!(
                cursor_position(&records[1], cursor_properties),
                Some((173, 121)),
                "the copied frame built after motion samples P1",
            );
            assert_window_enter_b_binding(
                &fixture,
                &records[1],
                cursor_properties,
                initial_fb,
                cursor_b.as_raw(),
                cursor_b_version,
                (7, 9),
            );
        }

        fixture.retire_output(0, 2);
        Backend::maybe_composite(&mut fixture.backend)
            .expect("run the composition tail after direct retirement");
        Backend::before_block(&mut fixture.backend);
        if fixture.backend.platform.any_cursor_commit_in_flight() {
            fixture.retire_output(0, 3);
            Backend::maybe_composite(&mut fixture.backend)
                .expect("run the composition tail after cursor retirement");
            Backend::before_block(&mut fixture.backend);
        }
        let position_after_motion = cursor_position_trace(&fixture, &sink_atomic, atomic_start);
        let layout = &fixture.backend.platform.outputs[0];
        let expected = (p1.0 - 7 - layout.x, p1.1 - 9 - layout.y);
        assert!(
            !position_after_motion.contains(&((p0.0 - 7 - layout.x, p0.1 - 9 - layout.y), true)),
            "post-motion operations must not restore P0; schedule before_acceptance={motion_before_acceptance}; trace={position_after_motion:?}",
        );
        assert!(
            position_after_motion
                .iter()
                .any(|(position, accepted)| *accepted && *position == expected),
            "accepted post-motion cursor position must use P1 adjusted by hotspot and output origin; schedule before_acceptance={motion_before_acceptance}; trace={position_after_motion:?}",
        );
        let final_records = sink_atomic.records();
        assert_eq!(
            final_records[1..]
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            if motion_before_acceptance {
                vec![AtomicCommitKind::SceneWithCursor]
            } else {
                vec![
                    AtomicCommitKind::SceneWithCursor,
                    AtomicCommitKind::CursorOnly,
                ]
            },
            "complete accepted cursor operation sequence after entry: {final_records:#?}",
        );
        assert!(
            final_records
                .iter()
                .all(|record| record.result == ScriptedResult::Ok)
        );
        assert_eq!(
            cursor_position_trace(&fixture, &sink_atomic, atomic_start),
            vec![(expected, true)],
            "every post-motion position operation is the accepted P1 position",
        );
        assert!(!fixture.backend.scene.has_pending_page_flips());
        assert_eq!(
            fixture
                .backend
                .platform
                .for_tests_pending_scanout_render_completion_count(),
            0,
            "all copied render completions are delivered",
        );
        fixture
            .backend
            .disable_output()
            .expect("Backend disable drains cursor and primary obligations");
        let drained_records = sink_atomic.records();
        assert_eq!(
            drained_records.len(),
            final_records.len(),
            "the copied sink trace ends after its last accepted cursor event",
        );
        let source_drain_records = fixture.recorders.atomic.records();
        assert_eq!(source_drain_records.len(), 1);
        let drain = source_drain_records
            .last()
            .expect("Backend blocking output drain record");
        assert_eq!(drain.kind, AtomicCommitKind::Scene);
        assert_eq!(
            drain.flags,
            ::drm::control::AtomicCommitFlags::ALLOW_MODESET
        );
        assert!(drain.events.is_empty());
        assert!(drain.properties.iter().all(|update| update.value == 0));
        assert_eq!(drain.result, ScriptedResult::Ok);
        fixture.assert_cursor_quiescent();
        let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }
}

fn argb_cursor(
    fixture: &mut AtomicCursorFixture,
    pixel: [u8; 4],
    hot_x: u16,
    hot_y: u16,
) -> yserver_core::backend::CursorHandle {
    use yserver_core::backend::{AnyHandle, Backend};

    let source = Backend::create_pixmap(&mut fixture.backend, None, 32, 1, 1)
        .expect("create cursor source pixmap through Backend");
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
    .expect("cursor picture handle");
    Backend::render_create_cursor(&mut fixture.backend, None, picture, hot_x, hot_y)
        .expect("create ARGB cursor through Backend")
        .expect("cursor handle")
}

fn complete_copied_frame(fixture: &mut AtomicCursorFixture) {
    use yserver_core::backend::Backend;

    let job = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()
        .into_iter()
        .next()
        .expect("copied frame has a real pending render completion");
    fixture.backend.platform.wait_idle_bounded();
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(job);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
}

fn cursor_position_trace(
    fixture: &AtomicCursorFixture,
    sink_atomic: &crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor,
    atomic_start: usize,
) -> Vec<((i32, i32), bool)> {
    use crate::drm::atomic::tests_support::ScriptedResult;

    let layout = &fixture.backend.platform.outputs[0];
    let plane = fixture.backend.platform.devices[1]
        .cursor
        .plane
        .as_ref()
        .expect("copied sink owns cursor plane");
    let properties = plane
        .properties_for(layout.output.crtc)
        .expect("copied sink cursor properties");
    let mut positions = Vec::new();
    for record in sink_atomic.records().into_iter().skip(atomic_start) {
        let x = record.properties.iter().find_map(|update| {
            (update.object == properties.plane.into() && update.property == properties.crtc_x)
                .then_some(update.value as i64 as i32)
        });
        let y = record.properties.iter().find_map(|update| {
            (update.object == properties.plane.into() && update.property == properties.crtc_y)
                .then_some(update.value as i64 as i32)
        });
        if let (Some(x), Some(y)) = (x, y) {
            positions.push(((x, y), record.result == ScriptedResult::Ok));
        }
    }
    positions
}

fn cursor_position(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) -> Option<(i32, i32)> {
    let x = record.properties.iter().find_map(|update| {
        (update.object == properties.plane.into() && update.property == properties.crtc_x)
            .then_some(update.value as i64 as i32)
    })?;
    let y = record.properties.iter().find_map(|update| {
        (update.object == properties.plane.into() && update.property == properties.crtc_y)
            .then_some(update.value as i64 as i32)
    })?;
    Some((x, y))
}

fn cursor_framebuffer(
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) -> Option<u32> {
    record.properties.iter().find_map(|update| {
        (update.object == properties.plane.into() && update.property == properties.fb_id)
            .then(|| u32::try_from(update.value).expect("cursor FB_ID fits u32"))
    })
}

fn assert_window_enter_b_binding(
    fixture: &AtomicCursorFixture,
    record: &crate::drm::atomic::tests_support::AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
    initial_fb: u32,
    cursor_b_xid: u32,
    cursor_b_version: u64,
    cursor_b_hotspot: (u16, u16),
) {
    let framebuffer = cursor_framebuffer(record, properties)
        .expect("accepted window-enter commit includes a cursor FB_ID");
    assert_ne!(framebuffer, 0, "window entry binds a nonzero FB_ID");
    assert_ne!(
        framebuffer, initial_fb,
        "window entry replaces cursor A's FB"
    );

    let output = &fixture.backend.platform.outputs[0];
    let device = fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("window-enter output has a DRM device");
    let binding = device
        .cursor
        .crtc_states
        .get(&output.output.crtc)
        .and_then(|state| state.applied)
        .expect("accepted window-enter commit has an applied cursor binding");
    let (sprite_lease, allocated_framebuffer) = device
        .cursor
        .plane
        .as_ref()
        .and_then(|plane| plane.candidate_sprite())
        .expect("B's sprite lease resolves to an allocated framebuffer");
    assert_eq!(sprite_lease, binding.lease);
    assert_eq!(allocated_framebuffer, framebuffer);
    assert_eq!(binding.framebuffer, framebuffer);
    assert_eq!(
        fixture.backend.effective_cursor_xid,
        Some(cursor_b_xid),
        "the entered window selects cursor B",
    );
    assert_eq!(
        fixture.backend.cursor_records[&cursor_b_xid].version, cursor_b_version,
        "the selected cursor XID carries B's sprite version",
    );
    assert!(
        binding
            .lease
            .matches(cursor_b_version, cursor_b_hotspot.0, cursor_b_hotspot.1),
        "the accepted framebuffer lease carries B's version and hotspot",
    );
    assert_eq!(
        (binding.hot_x, binding.hot_y),
        cursor_b_hotspot,
        "the accepted binding carries B's hotspot",
    );
    for (property, value) in [
        (properties.hotspot_x, cursor_b_hotspot.0),
        (properties.hotspot_y, cursor_b_hotspot.1),
    ] {
        if let Some(property) = property {
            assert!(
                record.properties.iter().any(|update| {
                    update.object == properties.plane.into()
                        && update.property == property
                        && update.value == u64::from(value)
                }),
                "window-enter commit sends B's hotspot property {property:?}"
            );
        }
    }
}
