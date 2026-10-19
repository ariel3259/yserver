//! Atomic-only final gate scenarios assembled from the Backend-driven C1-C9 regressions.

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: ignore a rejected virtual hotspot cap or admit direct scanout with cursor_hw assumed
// true on an ineligible output; read only OnScreen in the latest-frame pixel oracle; the per-frame
// C9 pixel and request checks must fail.
fn atomic_cursor_only_ineligible_outputs_never_attempt_hardware() {
    assert_ineligible_animations_and_direct_admission_stay_software();
    super::round_a::atomic_cursor_only_eligibility_matrix();
    super::round_a::atomic_cursor_only_composed_cursor_pixels_follow_output_local_eligibility();
    super::round_a::atomic_cursor_only_direct_probe_uses_backend_recorded_sink();
    super::round_b_producers::atomic_cursor_only_held_direct_services_show_hide_animation_and_first_bind();
    super::round_b_producers::atomic_cursor_only_vsync_direct_rebind_receipt_routes_motion_to_hardware();
}

fn assert_ineligible_animations_and_direct_admission_stay_software() {
    use std::{cell::Cell, rc::Rc, time::Duration};
    use yserver_core::backend::Backend;

    let cases: [(&str, &str, bool, Vec<&str>); 3] = [
        ("no plane", "amdgpu", true, Vec::new()),
        (
            "missing CRTC_X",
            "amdgpu",
            true,
            super::round_a::ATOMIC_CURSOR_PROPERTIES
                .into_iter()
                .filter(|name| *name != "CRTC_X")
                .collect(),
        ),
        (
            "virtual hotspot cap rejected",
            "virtio_gpu",
            false,
            super::round_a::ATOMIC_CURSOR_PROPERTIES
                .into_iter()
                .chain(["HOTSPOT_X", "HOTSPOT_Y"])
                .collect(),
        ),
    ];

    for (case_index, (name, driver, hotspot_cap_accepted, properties)) in
        cases.into_iter().enumerate()
    {
        let base = std::time::Instant::now();
        let now = Rc::new(Cell::new(base));
        let clock_now = Rc::clone(&now);
        let clock: Rc<dyn Fn() -> std::time::Instant> = Rc::new(move || clock_now.get());
        let Some(mut fixture) = super::support::AtomicCursorFixture::new_with_clock(clock)
            .expect("construct ineligible-output animation fixture")
        else {
            eprintln!("SKIP: {name}: lavapipe Backend fixture unavailable");
            continue;
        };
        if !fixture
            .backend
            .platform
            .gpu
            .vk
            .as_deref()
            .is_some_and(|vk| vk.supports_sync_fd())
        {
            eprintln!("SKIP: {name}: Vulkan SYNC_FD export is unavailable");
            continue;
        }
        fixture
            .backend
            .platform
            .for_tests_accept_output_power_commits();

        let device_key = fixture.backend.platform.devices[0].key;
        let crtc = fixture.backend.platform.outputs[0].output.crtc;
        let crtc_id = u32::from(crtc);
        let planes = if properties.is_empty() {
            Vec::new()
        } else {
            vec![(10, vec![crtc_id], properties)]
        };
        let snapshot = super::round_a::cursor_topology_snapshot(
            driver,
            hotspot_cap_accepted,
            &[crtc_id],
            &planes,
        );
        assert!(
            fixture
                .backend
                .platform
                .for_tests_queue_cursor_discovery_snapshot(device_key, snapshot)
        );
        let output = &fixture.backend.platform.outputs[0];
        fixture.recorders.connector_snapshot.queue_snapshot(vec![
            crate::kms::render::platform::ConnectorSnapshot {
                key: output.key.clone(),
                modes: vec![output.output.picked.clone()],
                mm_width: 0,
                mm_height: 0,
                edid: Vec::new(),
                connector_type: "test".to_owned(),
            },
        ]);
        fixture
            .recorders
            .hotplug
            .queue_change()
            .expect("queue topology change for ineligible cursor");
        Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
        let rescan_deadline = fixture
            .backend
            .pstate
            .hotplug_rescan_deadline
            .expect("Backend scheduled topology rediscovery");
        now.set(rescan_deadline);
        Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
        assert!(
            !fixture
                .backend
                .platform
                .cursor_plane_available_for_output(0),
            "{name}: Backend topology discovery must leave HW cursor unavailable",
        );

        let records_before = fixture.recorders.atomic.records().len();
        let root = fixture.backend.core.window_id;
        let width = fixture.backend.platform.fb_w;
        let height = fixture.backend.platform.fb_h;
        Backend::fill_rectangle(
            &mut fixture.backend,
            None,
            root,
            0x0011_2233,
            0,
            0,
            width,
            height,
        )
        .expect("paint background through Backend");
        let first = super::round_b_producers::create_argb_cursor_extent(
            &mut fixture.backend,
            [0x10, 0x20, 0xe0, 0xff],
            4,
            4,
            0,
            0,
        );
        let second = super::round_b_producers::create_argb_cursor_extent(
            &mut fixture.backend,
            [0xd0, 0x20, 0x10, 0xff],
            4,
            4,
            0,
            0,
        );
        let animation =
            Backend::create_anim_cursor(&mut fixture.backend, None, &[(first, 40), (second, 40)])
                .expect("create animation through Backend")
                .expect("animation handle");
        Backend::define_cursor(&mut fixture.backend, None, root, animation.as_raw())
            .expect("select animation through Backend");
        Backend::set_cursor_hidden(&mut fixture.backend, false);
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 80, 90);
        Backend::clear_area(
            &mut fixture.backend,
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
        .expect("damage animated software cursor through Backend");
        Backend::maybe_composite(&mut fixture.backend)
            .expect("compose first ineligible animation frame");
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        let cursor_plane = ::drm::control::from_u32(10).expect("synthetic cursor plane handle");
        let first_record = fixture
            .recorders
            .atomic
            .records()
            .last()
            .cloned()
            .expect("first animation frame has an accepted request");
        assert_eq!(
            first_record.kind,
            crate::drm::atomic::AtomicCommitKind::Scene
        );
        let first_pixel = super::round_c_reassign::assert_accepted_ineligible_software_frame(
            &mut fixture,
            &first_record,
            cursor_plane,
            100 + u32::try_from(case_index).expect("case index fits"),
            (80, 90),
        );

        let animation_deadline = Backend::next_wakeup(&fixture.backend)
            .expect("Backend schedules the next animation frame");
        now.set(animation_deadline + Duration::from_millis(1));
        Backend::maybe_composite(&mut fixture.backend)
            .expect("advance animation through Backend scheduler");
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        let second_record = fixture
            .recorders
            .atomic
            .records()
            .last()
            .cloned()
            .expect("next animation frame has an accepted request");
        assert_eq!(
            second_record.kind,
            crate::drm::atomic::AtomicCommitKind::Scene
        );
        let second_pixel = super::round_c_reassign::assert_accepted_ineligible_software_frame(
            &mut fixture,
            &second_record,
            cursor_plane,
            200 + u32::try_from(case_index).expect("case index fits"),
            (80, 90),
        );
        assert_ne!(
            first_pixel, second_pixel,
            "{name}: software animation advances its actual primary pixels",
        );

        let direct = super::round_a::setup_cursor_only_direct_present(&mut fixture);
        assert_eq!((direct.width, direct.height), (width, height));
        assert_eq!(direct.window_xid, 0x7380);
        let mut next_frame = 400 + u32::try_from(case_index).expect("case index fits") * 20;
        for attempt in 0..9_u64 {
            let cursor_position = (80 + i32::try_from(attempt).expect("attempt fits"), 90);
            Backend::warp_pointer_root(
                &mut fixture.backend,
                &mut fixture.state,
                cursor_position.0,
                cursor_position.1,
            );
            let (candidate, event) =
                super::round_a::direct_present_for_id(&direct, 24_000 + attempt);
            assert!(candidate.update_is_full);
            Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
            let records_before_attempt = fixture.recorders.atomic.records().len();
            let _handled = Backend::try_present_direct(&mut fixture.backend, candidate, event)
                .expect("advance fullscreen direct admission through Backend");
            assert!(
                !fixture.backend.pstate.scanout_m2.active()
                    && fixture.backend.pstate.scanout_m2.pending.is_none(),
                "{name}: attempt {attempt} cannot admit direct scanout without an eligible HW cursor",
            );

            if !fixture.backend.scene.has_pending_page_flip(0) {
                Backend::maybe_composite(&mut fixture.backend)
                    .expect("submit cursor-motion software frame through Backend");
            }
            if !fixture.backend.scene.has_pending_page_flip(0) {
                Backend::clear_area(
                    &mut fixture.backend,
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
                .expect("damage fullscreen frame through Backend after direct admission");
                Backend::maybe_composite(&mut fixture.backend)
                    .expect("submit fullscreen software-cursor frame through Backend");
            }
            let accepted = fixture.recorders.atomic.records()[records_before_attempt..]
                .iter()
                .rev()
                .find(|record| {
                    record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
                        && record.properties.iter().any(|update| {
                            update.object == fixture.backend.platform.outputs[0].output.plane.into()
                        })
                })
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: attempt {attempt} accepted no primary after direct and scene paths; before={records_before_attempt}, records={:?}, wants_compose={}, wants_scene={}, cursor={:?}",
                        fixture
                            .recorders
                            .atomic
                            .records()
                            .iter()
                            .map(|record| (record.kind, record.result, record.events.len()))
                            .collect::<Vec<_>>(),
                        fixture.backend.for_tests_scene_wants_compose(),
                        fixture.backend.for_tests_scene_wants_scene(),
                        fixture.backend.scene.cursor_mode(),
                    )
                });
            assert_eq!(
                accepted.kind,
                crate::drm::atomic::AtomicCommitKind::Scene,
                "{name}: direct-admission attempt {attempt} remains a software scene frame",
            );
            if fixture.backend.scene.has_pending_page_flip(0) {
                super::round_c_reassign::assert_accepted_ineligible_software_frame(
                    &mut fixture,
                    &accepted,
                    cursor_plane,
                    next_frame,
                    cursor_position,
                );
            } else {
                super::round_c_reassign::assert_accepted_ineligible_retired_software_frame(
                    &mut fixture,
                    &accepted,
                    cursor_plane,
                    cursor_position,
                );
            }
            if fixture.backend.scene.has_pending_page_flip(0) {
                fixture.retire_output(0, next_frame);
                next_frame += 1;
            }
            next_frame += 1;
        }

        Backend::set_cursor_hidden(&mut fixture.backend, true);
        Backend::clear_area(
            &mut fixture.backend,
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
        .expect("damage final software cursor hide through Backend");
        Backend::maybe_composite(&mut fixture.backend).expect("compose final cursor hide");
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        fixture.retire_output(0, next_frame);
        fixture.assert_cursor_quiescent();
        assert!(
            fixture.recorders.atomic.records()[records_before..]
                .iter()
                .all(
                    |record| record.kind != crate::drm::atomic::AtomicCommitKind::CursorOnly
                        && record.kind != crate::drm::atomic::AtomicCommitKind::SceneWithCursor
                        && record
                            .properties
                            .iter()
                            .all(|update| update.object != cursor_plane.into())
                ),
            "{name}: every animated, direct-admission, and hide request stays off the cursor plane",
        );
        assert_eq!(
            fixture.recorders.atomic.records()[records_before..]
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            vec![crate::drm::atomic::AtomicCommitKind::Scene; 12],
            "{name}: animation, nine direct-admission frames, and hide use only scene commits",
        );

        let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(
            resources.end_state(),
            (0, 0, 0),
            "{name}: no cursor FB leaks"
        );
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: allow Async while an accepted hardware binding remains by removing the visibility
// guards; the expected software-frame detach disappears and the recorded handoff assertion fails.
fn atomic_cursor_only_tearing_keeps_cursor_detached_and_async_clean() {
    super::super::tearing::run_atomic_cursor_only_tearing_gate_cases();
    super::round_c_reassign::atomic_cursor_only_active_reassignment_waits_for_software_frame();
}
