use super::*;

// A3 uses the existing recording fixture for Vulkan and accepted KMS commits,
// but it does not seed ServerState window trees or drive their Backend events.
// These small helpers supply that missing scene setup and inspection while
// keeping each test on the normal Present, scheduler, and Backend paths.
fn a3_records_since(
    fixture: &super::commit_identity::recording_scene::RecordingSceneFixture,
    fd: std::os::fd::RawFd,
    offset: usize,
) -> Vec<crate::drm::atomic::tests_support::AtomicCommitRecord> {
    fixture.events.recorders[&fd]
        .records()
        .into_iter()
        .skip(offset)
        .filter(|record| record.device_fd == fd)
        .collect()
}

pub(crate) fn a3_admit_output_owner(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    direct: &super::commit_identity::recording_scene::DirectPresentSetup,
    output_idx: usize,
) -> Vec<crate::drm::atomic::tests_support::AtomicCommitRecord> {
    use yserver_core::backend::Backend;

    let fd = fixture.kms_fd(output_idx);
    let start = fixture.events.recorders[&fd].records().len();
    fixture
        .events
        .backend
        .for_tests_set_scanout_m1_prober(record_scanout_m1_probe);
    Backend::note_present_scanout_candidate(&mut fixture.events.backend, direct.candidate);
    let probation = usize::from(crate::kms::render::backend::SCANOUT_M2_ELIGIBLE_ROOT_PROBATION);
    for attempt in 0..probation {
        assert_eq!(
            Backend::try_present_direct(
                &mut fixture.events.backend,
                direct.candidate,
                direct.event.clone(),
            )
            .expect("Output Present enters through Backend"),
            attempt + 1 == probation,
            "Output({output_idx}) enters after normal probation"
        );
    }
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("scheduler submits the Output owner");
    fixture.events.wait_for_submitted_gpu_work();

    let owner = &fixture.events.backend.pstate.direct.outputs[output_idx];
    let pending_id = owner
        .pending
        .as_ref()
        .and_then(|frame| frame.accepted_event_id)
        .expect("recorded direct commit identity");
    let crtc = fixture.events.backend.platform.outputs[output_idx]
        .output
        .crtc;
    let records = a3_records_since(fixture, fd, start);
    assert!(
        records.iter().any(|record| {
            super::commit_identity::accepted_event_id(record) == Some(pending_id)
                && record.events.len() == 1
                && record.events[0].crtc == crtc
        }),
        "Output Present made one accepted CRTC commit on its owner"
    );
    records
}

pub(crate) fn a3_enter_output_owner(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    direct: &super::commit_identity::recording_scene::DirectPresentSetup,
    output_idx: usize,
) {
    use yserver_core::backend::Backend;

    let records = a3_admit_output_owner(fixture, direct, output_idx);
    a3_retire_records(fixture, &records);
    let completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(completed.len(), 1, "one Present completes once");
    assert_eq!(completed[0].present_id, direct.candidate.present_id);
    let current = fixture.events.backend.pstate.direct.outputs[output_idx]
        .current
        .as_ref()
        .expect("owner is held after its CRTC retirement");
    assert_eq!(current.candidate.present_id, direct.candidate.present_id);
}

fn a3_enter_root_owner(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    direct: &super::commit_identity::recording_scene::DirectPresentSetup,
) {
    use yserver_core::backend::Backend;

    let fd = fixture.kms_fd(0);
    let start = fixture.events.recorders[&fd].records().len();
    let probation = usize::from(crate::kms::render::backend::SCANOUT_M2_ELIGIBLE_ROOT_PROBATION);
    for attempt in 0..probation {
        assert_eq!(
            Backend::try_present_direct(
                &mut fixture.events.backend,
                direct.candidate,
                direct.event.clone(),
            )
            .expect("Root Present enters through Backend"),
            attempt + 1 == probation,
            "Root enters after normal probation"
        );
    }
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("scheduler submits the Root owner");
    fixture.events.wait_for_submitted_gpu_work();
    let pending_id = fixture
        .events
        .backend
        .pstate
        .direct
        .root()
        .pending
        .as_ref()
        .and_then(|frame| frame.accepted_event_id)
        .expect("recorded grouped direct commit identity");
    let record = a3_records_since(fixture, fd, start)
        .into_iter()
        .find(|record| {
            super::commit_identity::accepted_event_id(record) == Some(pending_id)
                && record.events.len() == fixture.events.backend.platform.outputs.len()
        })
        .expect("Root Present made one grouped commit");
    for event in &record.events {
        fixture.events.queue_commit_completion(
            fd,
            event.crtc,
            pending_id,
            51,
            std::time::Duration::from_millis(510),
        );
    }
    Backend::on_page_flip_ready(&mut fixture.events.backend, &mut fixture.events.state, fd);
    let completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(completed.len(), 1, "grouped Present completes once");
    assert_eq!(completed[0].present_id, direct.candidate.present_id);
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .current
            .is_some()
    );
}

fn a3_tick(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
) -> Vec<crate::drm::atomic::tests_support::AtomicCommitRecord> {
    use yserver_core::backend::Backend;

    let fd = fixture.kms_fd(0);
    let start = fixture.events.recorders[&fd].records().len();
    Backend::maybe_composite(&mut fixture.events.backend).expect("Backend scheduler pass");
    fixture.events.wait_for_submitted_gpu_work();
    a3_records_since(fixture, fd, start)
}

fn a3_retire_records(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    records: &[crate::drm::atomic::tests_support::AtomicCommitRecord],
) {
    use yserver_core::backend::Backend;

    let fd = fixture.kms_fd(0);
    for record in records {
        let Some(event_id) = super::commit_identity::accepted_event_id(record) else {
            continue;
        };
        for event in &record.events {
            fixture.events.queue_commit_completion(
                fd,
                event.crtc,
                event_id,
                60,
                std::time::Duration::from_millis(600),
            );
        }
    }
    Backend::on_page_flip_ready(&mut fixture.events.backend, &mut fixture.events.state, fd);
}

fn a3_has_scene_commit_for(
    records: &[crate::drm::atomic::tests_support::AtomicCommitRecord],
    crtc: ::drm::control::crtc::Handle,
) -> bool {
    records.iter().any(|record| {
        matches!(
            record.kind,
            crate::drm::atomic::AtomicCommitKind::Scene
                | crate::drm::atomic::AtomicCommitKind::SceneWithCursor
        ) && record.events.iter().any(|event| event.crtc == crtc)
    })
}

fn a3_seed_ordered_window(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    state: &mut yserver_core::server::ServerState,
    resource_id: yserver_protocol::x11::ResourceId,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    mapped: bool,
) -> u32 {
    use crate::kms::render::store::DrawableKind;
    use yserver_core::{backend::Backend, resources::ROOT_WINDOW};

    super::seed_state_window(
        state,
        &mut fixture.events.backend,
        resource_id,
        ROOT_WINDOW,
        x,
        y,
        width,
        height,
    );
    let host_xid = super::synth_host_xid(resource_id);
    let backend = &mut fixture.events.backend;
    let old = backend
        .store
        .lookup(host_xid)
        .expect("seeded window has a storage record");
    backend.store.detach_xid(host_xid);
    backend.store_decref_with_invalidate(old);
    let storage = backend
        .platform
        .gpu
        .allocate_drawable_storage(width, height, 24)
        .expect("allocate test scene window image");
    backend
        .store
        .allocate(host_xid, DrawableKind::Window, 24, true, storage)
        .expect("register test scene window image");
    Backend::put_image(
        backend,
        None,
        host_xid,
        24,
        1,
        1,
        0,
        0,
        &[0x20, 0x40, 0x80, 0],
    )
    .expect("initialize test scene window pixels");
    if mapped {
        let _ = state.resources.map_window(resource_id);
    } else {
        backend.windows.get_mut(&host_xid).unwrap().mapped = false;
        backend
            .store
            .set_scene_participating(backend.store.lookup(host_xid).unwrap(), false);
    }
    backend.core.top_level_order.push(host_xid);
    host_xid
}

fn a3_add_existing_window_to_state(
    state: &mut yserver_core::server::ServerState,
    resource_id: yserver_protocol::x11::ResourceId,
    host_xid: u32,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    mapped: bool,
) {
    use yserver_core::{
        backend::WindowHandle,
        resources::{ROOT_VISUAL, ROOT_WINDOW},
    };
    use yserver_protocol::x11::{ClientId, CreateWindowRequest};

    state.resources.create_window(
        ClientId(14),
        CreateWindowRequest {
            depth: 24,
            window: resource_id,
            parent: ROOT_WINDOW,
            x,
            y,
            width,
            height,
            border_width: 0,
            class: 1,
            visual: ROOT_VISUAL,
            ..Default::default()
        },
    );
    state
        .resources
        .window_mut(resource_id)
        .expect("register state window")
        .host_xid = WindowHandle::from_raw(host_xid);
    if mapped {
        let _ = state.resources.map_window(resource_id);
    }
}

fn a3_restack_top_level(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    state: &mut yserver_core::server::ServerState,
    window: yserver_protocol::x11::ResourceId,
    sibling: yserver_protocol::x11::ResourceId,
    window_host: u32,
    sibling_host: u32,
    stack_mode: u8,
) {
    use yserver_core::{backend::Backend, host_x11::HostSubwindowConfig};
    use yserver_protocol::x11::ConfigureWindowRequest;

    Backend::configure_subwindow(
        &mut fixture.events.backend,
        None,
        window_host,
        HostSubwindowConfig {
            sibling: Some(sibling_host),
            stack_mode: Some(stack_mode),
            ..Default::default()
        },
    )
    .expect("ConfigureWindow backend callback");
    state.resources.configure_window(ConfigureWindowRequest {
        window,
        value_mask: 0,
        x: None,
        y: None,
        width: None,
        height: None,
        border_width: None,
        sibling: Some(sibling),
        stack_mode: Some(stack_mode),
    });
    Backend::sync_top_level_order(&mut fixture.events.backend, state);
}

fn a3_output_fixture(
    output_idx: usize,
    present_id: u64,
) -> Option<(
    super::commit_identity::recording_scene::RecordingSceneFixture,
    super::commit_identity::recording_scene::DirectPresentSetup,
)> {
    super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 two-output recording fixture")
        .map(|mut fixture| {
            let direct = fixture.install_direct_output_present(output_idx, present_id, 0);
            (fixture, direct)
        })
}

fn a3_root_fixture(
    present_id: u64,
) -> Option<(
    super::commit_identity::recording_scene::RecordingSceneFixture,
    super::commit_identity::recording_scene::DirectPresentSetup,
)> {
    super::commit_identity::recording_scene::RecordingSceneFixture::with_route(
        2,
        super::commit_identity::recording_scene::P1SceneRoute::Shared,
    )
    .expect("A3 same-refresh Root recording fixture")
    .map(|mut fixture| {
        let direct = fixture.install_direct_present(present_id, 0);
        (fixture, direct)
    })
}

fn a3_materialize_cow(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    state: &mut yserver_core::server::ServerState,
) -> u32 {
    use yserver_core::{backend::Backend, core_loop::process_request};
    use yserver_protocol::x11::{ClientId, RequestHeader, SequenceNumber, composite};

    super::super::install_client_for_render(state, 14);
    let body = yserver_core::resources::ROOT_WINDOW.0.to_le_bytes();
    process_request::process_request(
        state,
        &mut fixture.events.backend as &mut dyn Backend,
        ClientId(14),
        SequenceNumber(1),
        RequestHeader {
            opcode: 144,
            data: composite::GET_OVERLAY_WINDOW,
            length_units: 2,
        },
        &body,
        None,
    )
    .expect("materialize the Composite overlay through Backend");
    fixture
        .events
        .backend
        .cow_host_xid()
        .expect("GetOverlayWindow returns the materialized COW")
}

fn a3_install_root_present(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    present_id: u64,
    target_xid: u32,
) -> super::commit_identity::recording_scene::DirectPresentSetup {
    let mut direct = fixture.install_direct_present(present_id, 0);
    let backend = &mut fixture.events.backend;
    backend
        .core
        .top_level_order
        .retain(|&xid| xid != direct.target_xid);
    direct.candidate.dst_window_xid = target_xid;
    direct.candidate.paint_dst_host_xid = target_xid;
    direct.candidate.completion_dst_host_xid = target_xid;
    direct.event.dst_host_xid = target_xid;
    direct
}

fn a3_present_root_copy(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    direct: &super::commit_identity::recording_scene::DirectPresentSetup,
    color: u32,
) {
    use yserver_core::backend::Backend;

    let mut candidate = direct.candidate;
    candidate.valid_region_xid = 0xdead;
    Backend::fill_rectangle(
        &mut fixture.events.backend,
        None,
        candidate.src_host_xid,
        color,
        0,
        0,
        candidate.src_width,
        candidate.src_height,
    )
    .expect("paint the authoritative Present pixmap");
    assert!(
        !Backend::try_present_direct(&mut fixture.events.backend, candidate, direct.event.clone(),)
            .expect("ineligible authoritative Present through Backend"),
        "a nonzero valid-region id keeps the root-sized Present on Copy"
    );
    Backend::copy_area(
        &mut fixture.events.backend,
        None,
        candidate.src_host_xid,
        candidate.paint_dst_host_xid,
        0,
        0,
        0,
        0,
        candidate.src_width,
        candidate.src_height,
    )
    .expect("run the Present Copy fallback");
    Backend::note_present_pixmap(
        &mut fixture.events.backend,
        candidate.src_pixmap_xid,
        candidate.paint_dst_host_xid,
    );
}

fn a3_visible_root_pixel(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    x: i16,
    y: i16,
) -> Vec<u8> {
    crate::kms::render::backend::read_scanout_region(
        &mut fixture.events.backend,
        ash::vk::Rect2D {
            offset: ash::vk::Offset2D {
                x: i32::from(x),
                y: i32::from(y),
            },
            extent: ash::vk::Extent2D {
                width: 1,
                height: 1,
            },
        },
        crate::kms::render::backend::ScanoutReadSelection::PermissiveDump,
    )
    .expect("read the visible scanout buffer")
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: A01 makes every authoritative Present request a root-wide unflip;
// A02 treats every shaped COW as hidden from every output.
fn cow_present_visibility() {
    use yserver_core::backend::Backend;
    use yserver_protocol::x11::{ResourceId, xfixes::RegionRect};

    for (row, present_id, shape_width, expected_unflip) in [
        ("shaped-away", 940, 1920, false),
        ("intersecting", 943, 3840, true),
    ] {
        let Some((fixture, direct)) = a3_output_fixture(1, present_id) else {
            eprintln!("SKIP cow_present_visibility: Vulkan fixture unavailable");
            return;
        };
        let mut fixture = fixture.cleanup_guard();
        let mut state = yserver_core::server::ServerState::new();
        let game_xid = direct.target_xid;
        let (game_x, game_y, game_width, game_height) = {
            let output = &fixture.events.backend.platform.outputs[1];
            (
                output.x as i16,
                output.y as i16,
                output.width,
                output.height,
            )
        };
        a3_add_existing_window_to_state(
            &mut state,
            ResourceId(0x3160),
            game_xid,
            game_x,
            game_y,
            game_width,
            game_height,
            true,
        );
        Backend::sync_top_level_order(&mut fixture.events.backend, &state);
        a3_enter_output_owner(&mut fixture, &direct, 1);
        let current_present = fixture.events.backend.pstate.direct.outputs[1]
            .current
            .as_ref()
            .expect("Output(1) is held")
            .candidate
            .present_id;

        let cow_xid = a3_materialize_cow(&mut fixture, &mut state);
        Backend::set_shape_rectangles(
            &mut fixture.events.backend,
            None,
            cow_xid,
            0,
            Some(&[RegionRect {
                x: 0,
                y: 0,
                width: shape_width,
                height: 1080,
            }]),
        )
        .expect("shape the COW through Backend");
        let stage = a3_install_root_present(&mut fixture, present_id + 1, cow_xid);
        a3_present_root_copy(&mut fixture, &stage, 0x00d1_7b31);
        assert_eq!(
            fixture.events.backend.pstate.direct.outputs[1]
                .current
                .as_ref()
                .map(|frame| frame.candidate.present_id),
            Some(current_present),
            "{row}: the held direct owner remains until a replacement retires"
        );

        let owner = &fixture.events.backend.pstate.direct.outputs[1];
        assert_eq!(
            owner.unflip_requested, expected_unflip,
            "{row}: COW visibility controls only the intersecting output"
        );
        if expected_unflip {
            assert_eq!(
                owner.unflip_reason,
                Some("ineligible_authoritative_root_present"),
                "{row}: selected output records the authoritative Present reason"
            );
            assert_eq!(
                owner.unflip_fallback_source, None,
                "{row}: Output scope does not retain a fallback source"
            );
            assert!(
                owner.unflip_shadow_ready,
                "{row}: Copy prepares the replacement"
            );
            assert!(
                owner.current.is_some(),
                "{row}: direct frame awaits replacement"
            );
        }

        let records = a3_tick(&mut fixture);
        let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
        let output1_crtc = fixture.events.backend.platform.outputs[1].output.crtc;
        assert_eq!(
            [
                a3_has_scene_commit_for(&records, output0_crtc),
                a3_has_scene_commit_for(&records, output1_crtc),
            ],
            if expected_unflip {
                [true, true]
            } else {
                [true, false]
            },
            "{row}: only outputs selected by the shaped COW submit scene commits"
        );
        if expected_unflip {
            let replacement = records.iter().find(|record| {
                super::commit_identity::accepted_event_id(record).is_some()
                    && a3_has_scene_commit_for(std::slice::from_ref(*record), output1_crtc)
            });
            assert!(
                replacement.is_some(),
                "{row}: output 1 replacement is accepted"
            );
            a3_retire_records(&mut fixture, &records);
            assert!(
                fixture.events.backend.pstate.direct.outputs[1]
                    .current
                    .is_none(),
                "{row}: output 1 owner releases after its replacement retires"
            );
        } else {
            a3_retire_records(&mut fixture, &records);
            assert_eq!(
                &a3_visible_root_pixel(&mut fixture, 150, 150)[..3],
                &[0x31, 0x7b, 0xd1],
                "{row}: output 0 displays the copied stage pixels"
            );
            let owner = &fixture.events.backend.pstate.direct.outputs[1];
            assert!(!owner.unflip_requested, "{row}: output 1 remains direct");
            assert_eq!(
                owner.current.as_ref().unwrap().candidate.present_id,
                current_present,
                "{row}: output 1 stays current through output 0 retirement"
            );
        }
        drop(fixture);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: treat every root-covering Unredirected Present as visible on every output.
fn root_window_below_game_presenting_keeps_output_owner() {
    use yserver_core::backend::Backend;
    use yserver_protocol::x11::ResourceId;

    let Some((mut fixture, direct)) = a3_output_fixture(1, 944) else {
        eprintln!(
            "SKIP root_window_below_game_presenting_keeps_output_owner: Vulkan fixture unavailable"
        );
        return;
    };
    let mut state = yserver_core::server::ServerState::new();
    let root_width = fixture.events.backend.platform.fb_w;
    let root_height = fixture.events.backend.platform.fb_h;
    let stage_resource = ResourceId(0x3162);
    let stage_xid = a3_seed_ordered_window(
        &mut fixture,
        &mut state,
        stage_resource,
        0,
        0,
        root_width,
        root_height,
        true,
    );
    let game_resource = ResourceId(0x3163);
    let game = &fixture.events.backend.platform.outputs[1];
    a3_add_existing_window_to_state(
        &mut state,
        game_resource,
        direct.target_xid,
        game.x as i16,
        game.y as i16,
        game.width,
        game.height,
        true,
    );
    Backend::sync_top_level_order(&mut fixture.events.backend, &state);
    a3_enter_output_owner(&mut fixture, &direct, 1);

    let root_present = fixture.install_direct_present(945, 0);
    fixture
        .events
        .backend
        .core
        .top_level_order
        .retain(|&xid| xid != root_present.target_xid);
    let mut candidate = root_present.candidate;
    candidate.dst_window_xid = stage_xid;
    candidate.paint_dst_host_xid = stage_xid;
    candidate.completion_dst_host_xid = stage_xid;
    candidate.valid_region_xid = 0xdead;
    let mut event = root_present.event.clone();
    event.dst_host_xid = stage_xid;

    Backend::fill_rectangle(
        &mut fixture.events.backend,
        None,
        candidate.src_host_xid,
        0x00d1_7b31,
        0,
        0,
        candidate.src_width,
        candidate.src_height,
    )
    .expect("paint the below-game root Present");
    assert!(
        !Backend::try_present_direct(&mut fixture.events.backend, candidate, event.clone())
            .expect("below-game root Present remains a Copy"),
        "the valid-region gate serves this Present by Copy"
    );
    assert!(
        !fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
        "the root window covers output 0 only in the current stacking order"
    );
    Backend::copy_area(
        &mut fixture.events.backend,
        None,
        candidate.src_host_xid,
        candidate.paint_dst_host_xid,
        0,
        0,
        0,
        0,
        candidate.src_width,
        candidate.src_height,
    )
    .expect("copy the below-game Present");
    Backend::note_present_pixmap(
        &mut fixture.events.backend,
        candidate.src_pixmap_xid,
        candidate.paint_dst_host_xid,
    );
    let records = a3_tick(&mut fixture);
    assert!(
        a3_has_scene_commit_for(
            &records,
            fixture.events.backend.platform.outputs[0].output.crtc
        ),
        "the below-game root window is visible on output 0"
    );
    assert!(!fixture.events.backend.pstate.direct.outputs[1].unflip_requested);
    a3_retire_records(&mut fixture, &records);
    assert_eq!(
        &a3_visible_root_pixel(&mut fixture, 150, 150)[..3],
        &[0x31, 0x7b, 0xd1],
        "output 0 displays the root window below the game"
    );

    a3_restack_top_level(
        &mut fixture,
        &mut state,
        stage_resource,
        game_resource,
        stage_xid,
        direct.target_xid,
        0,
    );
    candidate.present_id = 946;
    event.present_id = 946;
    event.serial = 946;
    assert!(
        !Backend::try_present_direct(&mut fixture.events.backend, candidate, event)
            .expect("raised root Present remains a Copy"),
        "the valid-region gate keeps the raised Present composed"
    );
    let owner = &fixture.events.backend.pstate.direct.outputs[1];
    assert!(
        owner.unflip_requested,
        "the raised root window now covers output 1"
    );
    assert_eq!(
        owner.unflip_reason,
        Some("ineligible_authoritative_root_present")
    );
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: leave Copy fallback readiness updates restricted to the Root owner.
fn output_owner_copy_fallback_is_marked_ready() {
    use yserver_core::backend::Backend;
    use yserver_protocol::x11::ResourceId;

    for (present_id, cow_descendant) in [(947, false), (949, true)] {
        let Some((mut fixture, direct)) = a3_output_fixture(1, present_id) else {
            eprintln!(
                "SKIP output_owner_copy_fallback_is_marked_ready: Vulkan fixture unavailable"
            );
            return;
        };
        let mut state = yserver_core::server::ServerState::new();
        let root_width = fixture.events.backend.platform.fb_w;
        let root_height = fixture.events.backend.platform.fb_h;
        let stage_resource = ResourceId(if cow_descendant { 0x3167 } else { 0x3166 });
        let stage_xid = a3_seed_ordered_window(
            &mut fixture,
            &mut state,
            stage_resource,
            0,
            0,
            root_width,
            root_height,
            true,
        );
        let game = &fixture.events.backend.platform.outputs[1];
        a3_add_existing_window_to_state(
            &mut state,
            ResourceId(if cow_descendant { 0x3165 } else { 0x3164 }),
            direct.target_xid,
            game.x as i16,
            game.y as i16,
            game.width,
            game.height,
            true,
        );
        Backend::sync_top_level_order(&mut fixture.events.backend, &state);
        let cow_xid = cow_descendant.then(|| a3_materialize_cow(&mut fixture, &mut state));
        if cow_descendant {
            let cow_xid = cow_xid.unwrap();
            fixture
                .events
                .backend
                .windows
                .get_mut(&direct.target_xid)
                .expect("direct Output target window")
                .parent = Some(cow_xid);
            fixture
                .events
                .backend
                .core
                .top_level_order
                .retain(|&xid| xid != direct.target_xid);
            assert!(
                fixture
                    .events
                    .backend
                    .test_set_redirected_target(cow_xid, cow_xid),
                "the COW descendant resolves into the affected COW backing"
            );
            assert!(
                fixture
                    .events
                    .backend
                    .test_set_redirected_target(stage_xid, cow_xid),
                "the root Present Copy updates the same backing as the direct fallback"
            );
        }
        Backend::fill_rectangle(
            &mut fixture.events.backend,
            None,
            direct.candidate.src_host_xid,
            0x0000_00ff,
            0,
            0,
            direct.candidate.src_width,
            direct.candidate.src_height,
        )
        .expect("paint the retained direct source blue");
        a3_enter_output_owner(&mut fixture, &direct, 1);
        let owner = &fixture.events.backend.pstate.direct.outputs[1];
        if cow_descendant {
            assert_eq!(
                owner.current.as_ref().unwrap().fallback_target.backing_id(),
                fixture.events.backend.cow_id.unwrap(),
                "the COW-descendant fallback shares the root Present backing"
            );
        } else {
            a3_restack_top_level(
                &mut fixture,
                &mut state,
                stage_resource,
                ResourceId(0x3164),
                stage_xid,
                direct.target_xid,
                0,
            );
        }

        let stage = a3_install_root_present(&mut fixture, present_id + 1, stage_xid);
        a3_present_root_copy(&mut fixture, &stage, 0x00d1_7b31);
        let owner = &fixture.events.backend.pstate.direct.outputs[1];
        assert!(
            owner.unflip_requested,
            "visible authoritative Copy revokes Output(1)"
        );
        assert_eq!(owner.unflip_fallback_source, None);
        assert!(
            owner.unflip_shadow_ready,
            "the Copy already prepared the replacement"
        );

        let records = a3_tick(&mut fixture);
        assert!(
            a3_has_scene_commit_for(
                &records,
                fixture.events.backend.platform.outputs[1].output.crtc
            ),
            "output 1 submits its composed replacement"
        );
        a3_retire_records(&mut fixture, &records);
        assert!(
            !fixture.events.backend.pstate.direct.outputs[1].active(),
            "Output(1) retired its composed replacement"
        );
        assert_eq!(
            &a3_visible_root_pixel(&mut fixture, 1930, 20)[..3],
            &[0x31, 0x7b, 0xd1],
            "the replacement shows the new Present pixels, not the retained blue source"
        );
        fixture.finish();
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: A05 revokes every owner on any top-level restack; A06 omits stacking
// from tick revalidation; A10 revalidates before the core tree's stacking reprojection.
fn output_restack_revalidation() {
    use yserver_core::{backend::Backend, host_x11::HostSubwindowConfig, server::ServerState};
    use yserver_protocol::x11::{ConfigureWindowRequest, ResourceId};

    for (row, present_id) in [
        ("restack-elsewhere", 930),
        ("raise-over-owner", 931),
        ("move-and-lower", 934),
    ] {
        let Some((fixture, direct)) = a3_output_fixture(1, present_id) else {
            eprintln!("SKIP output_restack_revalidation: Vulkan fixture unavailable");
            return;
        };
        let mut fixture = fixture.cleanup_guard();
        let mut state = ServerState::new();
        let lower = a3_seed_ordered_window(
            &mut fixture,
            &mut state,
            ResourceId(0x3100),
            100,
            100,
            500,
            500,
            true,
        );
        let upper = a3_seed_ordered_window(
            &mut fixture,
            &mut state,
            ResourceId(0x3101),
            100,
            100,
            500,
            500,
            true,
        );
        Backend::fill_rectangle(
            &mut fixture.events.backend,
            None,
            lower,
            0x00ff_0000,
            0,
            0,
            500,
            500,
        )
        .expect("paint the lower output 0 window red");
        Backend::fill_rectangle(
            &mut fixture.events.backend,
            None,
            upper,
            0x0000_00ff,
            0,
            0,
            500,
            500,
        )
        .expect("paint the upper output 0 window blue");

        let game_rid = ResourceId(0x3111);
        let game_xid = direct.target_xid;
        let mut raised = None;
        let mut mover = None;
        if row == "raise-over-owner" {
            raised = Some(a3_seed_ordered_window(
                &mut fixture,
                &mut state,
                ResourceId(0x3110),
                1920,
                0,
                1920,
                1080,
                true,
            ));
        }
        a3_add_existing_window_to_state(&mut state, game_rid, game_xid, 1920, 0, 1920, 1080, true);
        if row == "move-and-lower" {
            mover = Some(a3_seed_ordered_window(
                &mut fixture,
                &mut state,
                ResourceId(0x3140),
                100,
                0,
                500,
                500,
                true,
            ));
        }
        Backend::sync_top_level_order(&mut fixture.events.backend, &state);
        a3_enter_output_owner(&mut fixture, &direct, 1);
        let owner_present = fixture.events.backend.pstate.direct.outputs[1]
            .current
            .as_ref()
            .expect("Output(1) owner held")
            .candidate
            .present_id;

        match row {
            "restack-elsewhere" => {
                a3_restack_top_level(
                    &mut fixture,
                    &mut state,
                    ResourceId(0x3100),
                    ResourceId(0x3101),
                    lower,
                    upper,
                    0,
                );
                let records = a3_tick(&mut fixture);
                let owner = &fixture.events.backend.pstate.direct.outputs[1];
                assert!(
                    !owner.unflip_requested,
                    "{row}: output 0 restack leaves Output(1) held"
                );
                assert_eq!(
                    owner.current.as_ref().unwrap().candidate.present_id,
                    owner_present,
                    "{row}: Output(1) retains the same direct Present"
                );
                assert!(
                    a3_has_scene_commit_for(
                        &records,
                        fixture.events.backend.platform.outputs[0].output.crtc
                    ),
                    "{row}: output 0 submits its restacked scene"
                );
                a3_retire_records(&mut fixture, &records);
                let root = fixture.events.backend.core.window_id;
                let visible = fixture
                    .events
                    .backend
                    .get_image_pixels_for_tests(root, 2, 150, 150, 1, 1, !0)
                    .expect("read restacked output 0 pixel")
                    .expect("restacked pixel bytes");
                assert_eq!(
                    &visible[..3],
                    &[0x00, 0x00, 0xff],
                    "{row}: raised lower window is visible in the committed scene"
                );
                assert_eq!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .as_ref()
                        .unwrap()
                        .candidate
                        .present_id,
                    owner_present,
                    "{row}: Output(1) stays current through output 0 retirement"
                );
            }
            "raise-over-owner" => {
                a3_restack_top_level(
                    &mut fixture,
                    &mut state,
                    ResourceId(0x3110),
                    game_rid,
                    raised.expect("raise row has an overlay"),
                    game_xid,
                    0,
                );
                assert!(
                    !fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
                    "{row}: revalidation waits for the scheduler tick"
                );
                let records = a3_tick(&mut fixture);
                let owner = &fixture.events.backend.pstate.direct.outputs[1];
                assert!(
                    owner.unflip_requested,
                    "{row}: intersecting raised window revokes the owner"
                );
                assert_eq!(
                    owner.unflip_reason,
                    Some("composite_tick_output_scene_changed"),
                    "{row}: tick records the scene-change reason"
                );
                assert!(
                    a3_has_scene_commit_for(
                        &records,
                        fixture.events.backend.platform.outputs[1].output.crtc
                    ),
                    "{row}: tick accepts the output 1 replacement"
                );
                a3_retire_records(&mut fixture, &records);
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .is_none(),
                    "{row}: output 1 owner retires after replacement acknowledgement"
                );
            }
            "move-and-lower" => {
                let mover = mover.expect("combined configure row has a mover");
                let mover_rid = ResourceId(0x3140);
                Backend::configure_subwindow(
                    &mut fixture.events.backend,
                    None,
                    mover,
                    HostSubwindowConfig {
                        x: Some(2000),
                        sibling: Some(game_xid),
                        stack_mode: Some(1),
                        ..Default::default()
                    },
                )
                .expect("combined ConfigureWindow backend callback");
                state.resources.configure_window(ConfigureWindowRequest {
                    window: mover_rid,
                    value_mask: 0,
                    x: Some(2000),
                    y: None,
                    width: None,
                    height: None,
                    border_width: None,
                    sibling: Some(game_rid),
                    stack_mode: Some(1),
                });
                Backend::sync_top_level_order(&mut fixture.events.backend, &state);
                let records = a3_tick(&mut fixture);
                let owner = &fixture.events.backend.pstate.direct.outputs[1];
                assert!(
                    !owner.unflip_requested,
                    "{row}: final core stacking order keeps the game topmost"
                );
                assert_eq!(
                    owner
                        .current
                        .as_ref()
                        .map(|frame| frame.candidate.present_id),
                    Some(owner_present),
                    "{row}: the same direct owner survives configure reprojection"
                );
                assert!(
                    !a3_has_scene_commit_for(
                        &records,
                        fixture.events.backend.platform.outputs[1].output.crtc
                    ),
                    "{row}: no output 1 replacement is submitted for the final eligible scene"
                );
                a3_retire_records(&mut fixture, &records);
            }
            _ => unreachable!("known Q10 row"),
        }
        drop(fixture);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: A07 removes tick scene revalidation; A09 ignores configured geometry
// during tick revalidation; A13 accepts an Output target classified as Other.
fn paused_output_scene_change() {
    use yserver_core::{backend::Backend, host_x11::HostSubwindowConfig, server::ServerState};
    use yserver_protocol::x11::ResourceId;

    for (row, present_id) in [("map", 932), ("geometry-move", 933), ("redirect", 938)] {
        let Some((fixture, direct)) = a3_output_fixture(1, present_id) else {
            eprintln!("SKIP paused_output_scene_change: Vulkan fixture unavailable");
            return;
        };
        let mut fixture = fixture.cleanup_guard();
        a3_enter_output_owner(&mut fixture, &direct, 1);
        let owner_present = fixture.events.backend.pstate.direct.outputs[1]
            .current
            .as_ref()
            .expect("Output(1) owner held")
            .candidate
            .present_id;
        let mut state = ServerState::new();

        match row {
            "map" => {
                let unrelated = a3_seed_ordered_window(
                    &mut fixture,
                    &mut state,
                    ResourceId(0x3120),
                    0,
                    0,
                    1920,
                    1080,
                    false,
                );
                Backend::map_subwindow(&mut fixture.events.backend, None, unrelated)
                    .expect("map output 0 window through Backend");
                let unrelated_records = a3_tick(&mut fixture);
                assert!(
                    !fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
                    "{row}: unrelated map leaves Output(1) held"
                );
                assert!(
                    a3_has_scene_commit_for(
                        &unrelated_records,
                        fixture.events.backend.platform.outputs[0].output.crtc
                    ),
                    "{row}: output 0 composes its newly mapped window"
                );
                a3_retire_records(&mut fixture, &unrelated_records);
                assert_eq!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .as_ref()
                        .unwrap()
                        .candidate
                        .present_id,
                    owner_present,
                    "{row}: unrelated map preserves the same owner"
                );

                let overlay = a3_seed_ordered_window(
                    &mut fixture,
                    &mut state,
                    ResourceId(0x3122),
                    1920,
                    0,
                    1920,
                    1080,
                    false,
                );
                Backend::fill_rectangle(
                    &mut fixture.events.backend,
                    None,
                    overlay,
                    0x00d1_7b31,
                    0,
                    0,
                    1920,
                    1080,
                )
                .expect("paint a distinguishable mapped overlay");
                Backend::map_subwindow(&mut fixture.events.backend, None, overlay)
                    .expect("map the intersecting window through Backend");
                let records = a3_tick(&mut fixture);
                let owner = &fixture.events.backend.pstate.direct.outputs[1];
                assert!(
                    owner.unflip_requested,
                    "{row}: intersecting map revokes the paused owner"
                );
                assert_eq!(
                    owner.unflip_reason,
                    Some("composite_tick_output_scene_changed"),
                    "{row}: tick records the scene-change reason"
                );
                assert_eq!(
                    owner.current.as_ref().unwrap().candidate.present_id,
                    owner_present,
                    "{row}: the held direct frame remains until replacement retirement"
                );
                assert!(
                    owner.pending.is_none() && owner.queued_successor.is_none(),
                    "{row}: map revalidation does not create another Present"
                );
                assert!(
                    a3_has_scene_commit_for(
                        &records,
                        fixture.events.backend.platform.outputs[1].output.crtc
                    ),
                    "{row}: replacement is composed without a new Present"
                );
                a3_retire_records(&mut fixture, &records);
                let root = fixture.events.backend.core.window_id;
                let visible = fixture
                    .events
                    .backend
                    .get_image_pixels_for_tests(root, 2, 1930, 20, 1, 1, !0)
                    .expect("read output 1 after replacement")
                    .expect("replacement pixel bytes");
                assert_eq!(
                    &visible[..3],
                    &[0x31, 0x7b, 0xd1],
                    "{row}: replacement displays the mapped overlay pixels"
                );
            }
            "geometry-move" => {
                let mover = a3_seed_ordered_window(
                    &mut fixture,
                    &mut state,
                    ResourceId(0x3130),
                    100,
                    0,
                    500,
                    500,
                    true,
                );
                Backend::configure_subwindow(
                    &mut fixture.events.backend,
                    None,
                    mover,
                    HostSubwindowConfig {
                        x: Some(300),
                        ..Default::default()
                    },
                )
                .expect("move within output 0 through Backend");
                let unrelated_records = a3_tick(&mut fixture);
                assert!(
                    !fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
                    "{row}: geometry remaining on output 0 keeps the owner"
                );
                assert!(
                    a3_has_scene_commit_for(
                        &unrelated_records,
                        fixture.events.backend.platform.outputs[0].output.crtc
                    ),
                    "{row}: output 0 composes the moved window"
                );
                a3_retire_records(&mut fixture, &unrelated_records);
                assert_eq!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .as_ref()
                        .unwrap()
                        .candidate
                        .present_id,
                    owner_present,
                    "{row}: unrelated geometry change preserves the owner"
                );

                Backend::configure_subwindow(
                    &mut fixture.events.backend,
                    None,
                    mover,
                    HostSubwindowConfig {
                        x: Some(2000),
                        ..Default::default()
                    },
                )
                .expect("move the mapped window onto output 1 through Backend");
                let records = a3_tick(&mut fixture);
                let owner = &fixture.events.backend.pstate.direct.outputs[1];
                assert!(
                    owner.unflip_requested,
                    "{row}: tick revalidation notices geometry newly intersecting output 1"
                );
                assert_eq!(
                    owner.unflip_reason,
                    Some("composite_tick_output_scene_changed"),
                    "{row}: configured geometry records the tick-revalidation reason"
                );
                assert_eq!(
                    owner.current.as_ref().unwrap().candidate.present_id,
                    owner_present,
                    "{row}: direct frame remains until output 1 replacement retires"
                );
                assert!(
                    owner.pending.is_none() && owner.queued_successor.is_none(),
                    "{row}: geometry revalidation does not submit a new Present"
                );
                assert!(
                    a3_has_scene_commit_for(
                        &records,
                        fixture.events.backend.platform.outputs[1].output.crtc
                    ),
                    "{row}: tick accepts the composed replacement"
                );
                a3_retire_records(&mut fixture, &records);
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .is_none(),
                    "{row}: owner retires after replacement acknowledgement"
                );
            }
            "redirect" => {
                Backend::allocate_redirected_backing(
                    &mut fixture.events.backend,
                    None,
                    yserver_core::backend::WindowHandle::from_raw_panicking(direct.target_xid),
                    direct.candidate.src_width,
                    direct.candidate.src_height,
                    24,
                )
                .expect("redirect the held window through Backend");
                let records = a3_tick(&mut fixture);
                let owner = &fixture.events.backend.pstate.direct.outputs[1];
                assert!(
                    owner.unflip_requested,
                    "{row}: redirected target is no longer eligible"
                );
                assert_eq!(
                    owner.unflip_reason,
                    Some("composite_tick_output_scene_changed"),
                    "{row}: redirect records the tick scene-change reason"
                );
                assert_eq!(
                    owner.current.as_ref().unwrap().candidate.present_id,
                    owner_present,
                    "{row}: retained direct frame awaits replacement"
                );
                assert!(
                    owner.pending.is_none() && owner.queued_successor.is_none(),
                    "{row}: redirect revalidation does not create a new Present"
                );
                assert!(
                    a3_has_scene_commit_for(
                        &records,
                        fixture.events.backend.platform.outputs[1].output.crtc
                    ),
                    "{row}: output 1 replacement is composed without a Present"
                );
                a3_retire_records(&mut fixture, &records);
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .is_none(),
                    "{row}: redirected owner retires after replacement acknowledgement"
                );
            }
            _ => unreachable!("known Q11 row"),
        }
        fixture.finish_after_normal_drain();
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: submit a queued successor after a mapped window makes its scene ineligible.
fn queued_successor_waits_for_scene_revalidation_after_overlay_map() {
    use yserver_core::{backend::Backend, server::ServerState};
    use yserver_protocol::x11::ResourceId;

    let Some((fixture, direct)) = a3_output_fixture(1, 945) else {
        eprintln!(
            "SKIP queued_successor_waits_for_scene_revalidation_after_overlay_map: Vulkan fixture unavailable"
        );
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    a3_enter_output_owner(&mut fixture, &direct, 1);

    let fd = fixture.kms_fd(1);
    let before = fixture.events.recorders[&fd].records().len();
    let mut pending_candidate = direct.candidate;
    pending_candidate.present_id = 946;
    let mut pending_event = direct.event.clone();
    pending_event.present_id = 946;
    pending_event.serial = 946;
    Backend::note_present_scanout_candidate(&mut fixture.events.backend, pending_candidate);
    assert!(
        Backend::try_present_direct(
            &mut fixture.events.backend,
            pending_candidate,
            pending_event,
        )
        .expect("submit the first successor through Backend Present")
    );
    let pending_id = fixture.events.backend.pstate.direct.outputs[1]
        .pending
        .as_ref()
        .and_then(|frame| frame.accepted_event_id)
        .expect("first successor has an accepted commit identity");
    let pending_record = a3_records_since(&fixture, fd, before)
        .into_iter()
        .find(|record| super::commit_identity::accepted_event_id(record) == Some(pending_id))
        .expect("record the pending direct successor commit");
    assert_eq!(pending_record.events.len(), 1);
    assert_eq!(
        pending_record.events[0].crtc,
        fixture.events.backend.platform.outputs[1].output.crtc
    );

    let mut successor_candidate = pending_candidate;
    successor_candidate.present_id = 947;
    let mut successor_event = direct.event.clone();
    successor_event.present_id = 947;
    successor_event.serial = 947;
    Backend::note_present_scanout_candidate(&mut fixture.events.backend, successor_candidate);
    assert!(
        Backend::try_present_direct(
            &mut fixture.events.backend,
            successor_candidate,
            successor_event,
        )
        .expect("queue the next Present through Backend")
    );
    assert_eq!(
        fixture.events.backend.pstate.direct.outputs[1]
            .queued_successor
            .as_ref()
            .map(|frame| frame.candidate.present_id),
        Some(947),
        "the successor waits behind the pending direct commit"
    );

    let mut state = ServerState::new();
    let overlay = a3_seed_ordered_window(
        &mut fixture,
        &mut state,
        ResourceId(0x3190),
        1920,
        0,
        1920,
        1080,
        false,
    );
    Backend::fill_rectangle(
        &mut fixture.events.backend,
        None,
        overlay,
        0x0001_2345,
        0,
        0,
        1920,
        1080,
    )
    .expect("paint distinguishable overlay pixels");
    Backend::map_subwindow(&mut fixture.events.backend, None, overlay)
        .expect("map overlay through Backend");

    a3_retire_records(&mut fixture, std::slice::from_ref(&pending_record));
    let owner = &fixture.events.backend.pstate.direct.outputs[1];
    assert!(
        owner.unflip_requested,
        "the newly ineligible scene requests unflip before queued direct submission"
    );
    assert!(
        owner.pending.is_none(),
        "the queued successor was not submitted after the pending frame retires"
    );

    let replacement = a3_tick(&mut fixture);
    assert!(
        a3_has_scene_commit_for(
            &replacement,
            fixture.events.backend.platform.outputs[1].output.crtc,
        ),
        "the next Backend scheduler pass composes output 1"
    );
    a3_retire_records(&mut fixture, &replacement);
    assert!(
        fixture
            .events
            .backend
            .direct_scanout_frame_for_output(1)
            .is_none(),
        "the invalid direct owner retires after the composed replacement"
    );
    let root_xid = fixture.events.backend.core.window_id;
    let visible = fixture
        .events
        .backend
        .get_image_pixels_for_tests(root_xid, 2, 1930, 20, 1, 1, !0)
        .expect("read output 1 after replacement")
        .expect("replacement pixel bytes");
    assert_eq!(
        &visible[..3],
        &[0x45, 0x23, 0x01],
        "the composed replacement shows the newly mapped overlay, not a direct successor"
    );
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: drop the Output-owner ancestor walk from direct_scopes_referencing_host_drawable.
fn ancestor_reparent_revokes_output_owner_immediately() {
    use yserver_core::backend::Backend;

    let Some((mut fixture, direct)) = a3_output_fixture(1, 935) else {
        eprintln!(
            "SKIP ancestor_reparent_revokes_output_owner_immediately: Vulkan fixture unavailable"
        );
        return;
    };
    let frame = 0x5200;
    let new_parent = 0x5201;
    super::seed_window(&mut fixture.events.backend, frame, None, 1920, 0);
    super::seed_window(&mut fixture.events.backend, new_parent, None, 0, 0);
    fixture
        .events
        .backend
        .windows
        .get_mut(&frame)
        .unwrap()
        .width = 1920;
    fixture
        .events
        .backend
        .windows
        .get_mut(&frame)
        .unwrap()
        .height = 1080;
    Backend::reparent_subwindow(
        &mut fixture.events.backend,
        None,
        direct.target_xid,
        frame,
        0,
        0,
    )
    .expect("put the direct window inside its WM frame before Present");
    fixture.events.backend.core.top_level_order = vec![new_parent, frame];
    a3_enter_output_owner(&mut fixture, &direct, 1);

    Backend::reparent_subwindow(
        &mut fixture.events.backend,
        None,
        frame,
        new_parent,
        1920,
        0,
    )
    .expect("reparent the ancestor at the same absolute position");
    let owner = &fixture.events.backend.pstate.direct.outputs[1];
    assert!(
        owner.unflip_requested,
        "ancestor changes revoke immediately"
    );
    assert_eq!(owner.unflip_reason, Some("reparent_direct_frame_chain"));
    assert_eq!(
        owner.current.as_ref().unwrap().candidate.present_id,
        direct.candidate.present_id,
        "the retained frame is still pinned until the composed replacement"
    );
    let records = a3_tick(&mut fixture);
    assert!(a3_has_scene_commit_for(
        &records,
        fixture.events.backend.platform.outputs[1].output.crtc
    ));
    a3_retire_records(&mut fixture, &records);
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: A12 makes drawable-specific unflip revoke every owner; A14 makes
// SHAPE revoke every scope immediately instead of waiting for tick revalidation.
fn drawable_change_isolation() {
    use yserver_core::backend::Backend;
    use yserver_protocol::x11::xfixes::RegionRect;

    for (row, first_id, second_id) in [("unmap", 936, 937), ("shape", 939, 940)] {
        let Some(fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
            .expect("A3 dual-owner fixture")
        else {
            eprintln!("SKIP drawable_change_isolation: Vulkan fixture unavailable");
            return;
        };
        let mut fixture = fixture.cleanup_guard();
        let first = fixture.install_direct_output_present(0, first_id, 0);
        let second = fixture.install_direct_output_present(1, second_id, 0);
        a3_enter_output_owner(&mut fixture, &first, 0);
        a3_enter_output_owner(&mut fixture, &second, 1);

        match row {
            "unmap" => {
                Backend::unmap_subwindow(&mut fixture.events.backend, None, first.target_xid)
                    .expect("unmap Output(0) drawable through Backend");
                let output0 = &fixture.events.backend.pstate.direct.outputs[0];
                let output1 = &fixture.events.backend.pstate.direct.outputs[1];
                assert!(
                    output0.unflip_requested,
                    "{row}: changed drawable revokes its owner"
                );
                assert_eq!(
                    output0.unflip_reason,
                    Some("unmap_direct_frame_drawable"),
                    "{row}: unmap keeps its scoped reason"
                );
                assert!(!output1.unflip_requested, "{row}: Output(1) stays held");
                assert_eq!(
                    output1.current.as_ref().unwrap().candidate.present_id,
                    second_id,
                    "{row}: Output(1) keeps its independent current frame"
                );
                let records = a3_tick(&mut fixture);
                let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
                let output1_crtc = fixture.events.backend.platform.outputs[1].output.crtc;
                assert!(
                    a3_has_scene_commit_for(&records, output0_crtc),
                    "{row}: Output(0) replacement is accepted"
                );
                assert!(
                    !a3_has_scene_commit_for(&records, output1_crtc),
                    "{row}: unaffected Output(1) gets no replacement"
                );
                a3_retire_records(&mut fixture, &records);
                let survivor = &fixture.events.backend.pstate.direct.outputs[1];
                assert!(
                    !survivor.unflip_requested,
                    "{row}: Output(1) survives retirement"
                );
                assert_eq!(
                    survivor.current.as_ref().unwrap().candidate.present_id,
                    second_id,
                    "{row}: Output(1) remains current through Output(0) retirement"
                );
            }
            "shape" => {
                Backend::set_shape_rectangles(
                    &mut fixture.events.backend,
                    None,
                    second.target_xid,
                    0,
                    Some(&[RegionRect {
                        x: 0,
                        y: 0,
                        width: 1000,
                        height: 1080,
                    }]),
                )
                .expect("clip Output(1) target through Backend SHAPE");
                assert!(
                    !fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
                    "{row}: SHAPE waits for the next scheduler pass"
                );
                assert!(
                    !fixture.events.backend.pstate.direct.outputs[0].unflip_requested,
                    "{row}: SHAPE does not eagerly revoke Output(0)"
                );
                let records = a3_tick(&mut fixture);
                let output0 = &fixture.events.backend.pstate.direct.outputs[0];
                let output1 = &fixture.events.backend.pstate.direct.outputs[1];
                assert!(
                    output1.unflip_requested,
                    "{row}: tick revalidation revokes Output(1)"
                );
                assert_eq!(
                    output1.unflip_reason,
                    Some("composite_tick_output_scene_changed"),
                    "{row}: SHAPE retains the tick scene-change reason"
                );
                assert!(!output0.unflip_requested, "{row}: Output(0) remains active");
                assert_eq!(
                    output0.current.as_ref().unwrap().candidate.present_id,
                    first_id,
                    "{row}: Output(0) keeps its direct frame"
                );
                assert!(
                    a3_has_scene_commit_for(
                        &records,
                        fixture.events.backend.platform.outputs[1].output.crtc
                    ),
                    "{row}: Output(1) replacement is accepted"
                );
                assert!(
                    !a3_has_scene_commit_for(
                        &records,
                        fixture.events.backend.platform.outputs[0].output.crtc
                    ),
                    "{row}: Output(0) gets no replacement"
                );
                a3_retire_records(&mut fixture, &records);
                let survivor = &fixture.events.backend.pstate.direct.outputs[0];
                assert!(
                    !survivor.unflip_requested,
                    "{row}: Output(0) survives Output(1) retirement"
                );
                assert_eq!(
                    survivor.current.as_ref().unwrap().candidate.present_id,
                    first_id,
                    "{row}: Output(0) remains current after Output(1) retirement"
                );
            }
            _ => unreachable!("known Q12 row"),
        }
        drop(fixture);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: on_top_level_order_changed omits Root's top-level unflip.
fn root_owner_triggers_unchanged() {
    use yserver_core::{backend::Backend, host_x11::HostSubwindowConfig, server::ServerState};
    use yserver_protocol::x11::{ResourceId, xfixes::RegionRect};

    let Some((mut fixture, direct)) = a3_root_fixture(941) else {
        eprintln!("SKIP root_owner_triggers_unchanged: Vulkan fixture unavailable");
        return;
    };
    a3_enter_root_owner(&mut fixture, &direct);
    let mut state = ServerState::new();
    let first = a3_seed_ordered_window(
        &mut fixture,
        &mut state,
        ResourceId(0x3150),
        10,
        10,
        100,
        100,
        true,
    );
    let second = a3_seed_ordered_window(
        &mut fixture,
        &mut state,
        ResourceId(0x3151),
        200,
        10,
        100,
        100,
        true,
    );
    let unmapped = a3_seed_ordered_window(
        &mut fixture,
        &mut state,
        ResourceId(0x3152),
        300,
        10,
        100,
        100,
        false,
    );
    let root_width = fixture.events.backend.platform.fb_w;
    let root_height = fixture.events.backend.platform.fb_h;
    a3_add_existing_window_to_state(
        &mut state,
        ResourceId(0x3153),
        direct.target_xid,
        0,
        0,
        root_width,
        root_height,
        true,
    );
    Backend::map_subwindow(&mut fixture.events.backend, None, unmapped)
        .expect("map unrelated window through Backend");
    Backend::configure_subwindow(
        &mut fixture.events.backend,
        None,
        first,
        HostSubwindowConfig {
            x: Some(30),
            ..Default::default()
        },
    )
    .expect("configure unrelated window through Backend");
    assert!(
        !fixture.events.backend.pstate.direct.root().unflip_requested,
        "unrelated map and configure keep Root direct as before"
    );

    a3_restack_top_level(
        &mut fixture,
        &mut state,
        ResourceId(0x3150),
        ResourceId(0x3151),
        first,
        second,
        0,
    );
    assert!(fixture.events.backend.pstate.direct.root().unflip_requested);
    assert_eq!(
        fixture.events.backend.pstate.direct.root().unflip_reason,
        Some("top_level_stack_changed")
    );
    Backend::set_shape_rectangles(
        &mut fixture.events.backend,
        None,
        direct.target_xid,
        0,
        Some(&[RegionRect {
            x: 0,
            y: 0,
            width: 1000,
            height: root_height,
        }]),
    )
    .expect("shape the Root direct window through Backend");
    assert_eq!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .unflip_last_reason,
        Some("shape_clips_direct_frame")
    );
    assert!(
        fixture
            .events
            .backend
            .pstate
            .direct
            .root()
            .current
            .is_some()
    );
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations by row: D02 returns after fresh unflip / uses a degraded-only pass mask;
// A16 returns after a submitted unflip / uses a degraded-only mask / excludes the degraded owner;
// T06 returns globally while an Output owner is held; T07 excludes unrelated output in degraded mode;
// T08 ignores scene damage when an Output owner is held.
fn other_output_progresses() {
    use yserver_core::backend::Backend;

    fn retire_held_output_owner(
        fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixtureGuard,
        direct: &super::commit_identity::recording_scene::DirectPresentSetup,
        row: &str,
    ) {
        Backend::unmap_subwindow(&mut fixture.events.backend, None, direct.target_xid)
            .expect("retire the held Output owner through Backend");
        let records = a3_tick(fixture);
        assert!(
            a3_has_scene_commit_for(
                &records,
                fixture.events.backend.platform.outputs[1].output.crtc
            ),
            "{row}: cleanup submits the owner replacement"
        );
        a3_retire_records(fixture, &records);
        assert!(
            fixture.events.backend.pstate.direct.outputs[1]
                .current
                .is_none(),
            "{row}: cleanup retires the held owner"
        );
        let _ = Backend::drain_completed_present_events(&mut fixture.events.backend);
        let _ = Backend::drain_retired_present_idle_events(&mut fixture.events.backend);
    }

    for (row, present_id) in [
        ("pending-direct", 956),
        ("held-direct", 957),
        ("fresh-unflip", 958),
        ("degraded-unflip", 959),
    ] {
        let Some((fixture, direct)) = a3_output_fixture(1, present_id) else {
            eprintln!("SKIP other_output_progresses: Vulkan fixture unavailable");
            return;
        };
        let mut fixture = fixture.cleanup_guard();
        let fd = fixture.kms_fd(0);
        let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
        let output1_crtc = fixture.events.backend.platform.outputs[1].output.crtc;
        a3_enter_output_owner(&mut fixture, &direct, 1);
        let warmup = a3_tick(&mut fixture);
        a3_retire_records(&mut fixture, &warmup);
        assert!(
            !fixture.events.backend.scene.has_pending_page_flip(0),
            "{row}: composed output is settled before the scenario damage"
        );
        assert_eq!(
            fixture.events.backend.pstate.direct.outputs[1]
                .current
                .as_ref()
                .map(|frame| frame.candidate.present_id),
            Some(direct.candidate.present_id),
            "{row}: scheduler warm-up leaves the held owner unchanged"
        );

        match row {
            // D02: an output 0 scene commit is accepted while a later output 1 direct frame is pending.
            "pending-direct" => {
                let before = fixture.events.recorders[&fd].records().len();
                let mut candidate = direct.candidate;
                candidate.present_id += 1;
                let mut event = direct.event.clone();
                event.present_id = candidate.present_id;
                event.serial = event.serial.wrapping_add(1);
                Backend::note_present_scanout_candidate(&mut fixture.events.backend, candidate);
                assert!(
                    Backend::try_present_direct(&mut fixture.events.backend, candidate, event)
                        .expect("pending successor through Backend Present"),
                    "{row}: successor is accepted while the owner remains current"
                );
                let pending_id = fixture.events.backend.pstate.direct.outputs[1]
                    .pending
                    .as_ref()
                    .and_then(|frame| frame.accepted_event_id)
                    .expect("pending output 1 frame has an accepted event identity");
                let pending_record = a3_records_since(&fixture, fd, before)
                    .into_iter()
                    .find(|record| {
                        super::commit_identity::accepted_event_id(record) == Some(pending_id)
                    })
                    .expect("record accepted output 1 pending commit");
                assert_eq!(
                    pending_record.events.len(),
                    1,
                    "{row}: pending commit has one CRTC event"
                );
                assert_eq!(
                    pending_record.events[0].crtc, output1_crtc,
                    "{row}: pending direct commit belongs to output 1"
                );

                super::commit_identity::direct_unflip::damage_scene_root(&mut fixture);
                let records = a3_tick(&mut fixture);
                let output0_record = records
                    .iter()
                    .find(|record| {
                        super::commit_identity::accepted_event_id(record).is_some()
                            && a3_has_scene_commit_for(std::slice::from_ref(*record), output0_crtc)
                    })
                    .expect("output 0 scene damage is accepted while output 1 is pending");
                let output0_id = super::commit_identity::accepted_event_id(output0_record).unwrap();
                assert_ne!(
                    output0_id, pending_id,
                    "{row}: independent CRTC acknowledgements use distinct event IDs"
                );
                let mut accepted_crtcs = vec![pending_record.events[0].crtc];
                accepted_crtcs.extend(records.iter().filter_map(|record| {
                    super::commit_identity::accepted_event_id(record).and_then(|_| {
                        record
                            .events
                            .iter()
                            .find(|event| event.crtc == output0_crtc)
                            .map(|event| event.crtc)
                    })
                }));
                assert_eq!(
                    accepted_crtcs,
                    [output1_crtc, output0_crtc],
                    "{row}: exact accepted commits cover the pending owner and damaged output"
                );
                assert_eq!(
                    fixture.events.backend.scene.pending_page_flip_event_id(0),
                    Some(output0_id),
                    "{row}: output 0 tracks its scene acknowledgement"
                );
                assert_eq!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .pending
                        .as_ref()
                        .and_then(|frame| frame.accepted_event_id),
                    Some(pending_id),
                    "{row}: output 1 tracks its direct acknowledgement independently"
                );
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .pending
                        .is_some(),
                    "{row}: output 1 remains pending after output 0 submission"
                );
                a3_retire_records(&mut fixture, std::slice::from_ref(output0_record));
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .pending
                        .is_some(),
                    "{row}: output 0 retirement leaves output 1 pending"
                );
                a3_retire_records(&mut fixture, std::slice::from_ref(&pending_record));
                assert_eq!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .as_ref()
                        .map(|frame| frame.candidate.present_id),
                    Some(candidate.present_id),
                    "{row}: output 1 advances only after its own acknowledgement"
                );
                retire_held_output_owner(&mut fixture, &direct, row);
            }
            // T06/T08: a held owner must not suppress an immediate wake or output 0's scene commit.
            "held-direct" => {
                super::commit_identity::direct_unflip::damage_scene_root(&mut fixture);
                let wakeup = Backend::next_wakeup(&fixture.events.backend);
                assert!(
                    wakeup.is_some_and(|deadline| deadline <= std::time::Instant::now()),
                    "{row}: output 0 scene damage arms an immediate wake"
                );
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .pending
                        .is_none(),
                    "{row}: wake assertion is driven only by held direct state, not pending work"
                );
                let records = a3_tick(&mut fixture);
                let accepted_crtcs: Vec<_> = records
                    .iter()
                    .filter(|record| super::commit_identity::accepted_event_id(record).is_some())
                    .flat_map(|record| record.events.iter().map(|event| event.crtc))
                    .collect();
                assert_eq!(
                    accepted_crtcs,
                    [output0_crtc],
                    "{row}: exact accepted CRTC set contains only composed output 0"
                );
                assert_eq!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .as_ref()
                        .map(|frame| frame.candidate.present_id),
                    Some(direct.candidate.present_id),
                    "{row}: held Output(1) remains the same owner"
                );
                a3_retire_records(&mut fixture, &records);
                assert_eq!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .as_ref()
                        .map(|frame| frame.candidate.present_id),
                    Some(direct.candidate.present_id),
                    "{row}: output 0 retirement leaves Output(1) held"
                );
                retire_held_output_owner(&mut fixture, &direct, row);
            }
            // D02/A16: fresh replacement and unrelated scene damage are both accepted in one pass.
            "fresh-unflip" => {
                Backend::unmap_subwindow(&mut fixture.events.backend, None, direct.target_xid)
                    .expect("request fresh unflip through Backend");
                super::commit_identity::direct_unflip::damage_scene_root(&mut fixture);
                let records = a3_tick(&mut fixture);
                let mut accepted_crtcs: Vec<_> = records
                    .iter()
                    .filter(|record| super::commit_identity::accepted_event_id(record).is_some())
                    .flat_map(|record| record.events.iter().map(|event| event.crtc))
                    .collect();
                accepted_crtcs.sort_by_key(|crtc| u32::from(*crtc));
                let mut expected_crtcs = vec![output0_crtc, output1_crtc];
                expected_crtcs.sort_by_key(|crtc| u32::from(*crtc));
                assert_eq!(
                    accepted_crtcs, expected_crtcs,
                    "{row}: exact accepted CRTC set includes composed output and unflip replacement"
                );
                let output0_record = records
                    .iter()
                    .find(|record| {
                        super::commit_identity::accepted_event_id(record).is_some()
                            && a3_has_scene_commit_for(std::slice::from_ref(*record), output0_crtc)
                    })
                    .expect("output 0 scene frame accepted in fresh-unflip pass");
                let replacement = records
                    .iter()
                    .find(|record| {
                        super::commit_identity::accepted_event_id(record).is_some()
                            && a3_has_scene_commit_for(std::slice::from_ref(*record), output1_crtc)
                    })
                    .expect("output 1 fresh replacement accepted in same pass");
                let output0_id = super::commit_identity::accepted_event_id(output0_record).unwrap();
                let replacement_id =
                    super::commit_identity::accepted_event_id(replacement).unwrap();
                assert_ne!(
                    output0_id, replacement_id,
                    "{row}: each output has an independent accepted acknowledgement ID"
                );
                assert_eq!(
                    fixture.events.backend.scene.pending_page_flip_event_id(0),
                    Some(output0_id),
                    "{row}: output 0 tracks its own scene acknowledgement"
                );
                assert_eq!(
                    fixture.events.backend.scene.pending_page_flip_event_id(1),
                    Some(replacement_id),
                    "{row}: output 1 tracks its replacement acknowledgement"
                );
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1].fresh_composed_unflip,
                    "{row}: accepted replacement is classified as a fresh unflip"
                );
                assert_eq!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .unflip_awaiting_outputs
                        .get(&1),
                    Some(&replacement_id),
                    "{row}: replacement identity remains assigned to output 1"
                );
                a3_retire_records(&mut fixture, std::slice::from_ref(output0_record));
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .is_some(),
                    "{row}: output 0 retirement does not release output 1's owner"
                );
                assert!(
                    fixture.events.backend.scene.has_pending_page_flip(1),
                    "{row}: output 1 acknowledgement remains pending independently"
                );
                a3_retire_records(&mut fixture, std::slice::from_ref(replacement));
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .is_none(),
                    "{row}: output 1 owner releases when its replacement retires"
                );
                assert!(
                    fixture.events.backend.present_source_pins.is_empty(),
                    "{row}: replacement retirement releases source pins"
                );
            }
            // D02/A16/T07: degraded replacement still composes unrelated ready output 0.
            "degraded-unflip" => {
                Backend::unmap_subwindow(&mut fixture.events.backend, None, direct.target_xid)
                    .expect("request degraded unflip through Backend");
                super::commit_identity::direct_unflip::damage_scene_root(&mut fixture);
                let recorder = fixture.events.recorders[&fd].clone();
                recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
                recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
                let records = a3_tick(&mut fixture);
                let mut accepted_crtcs: Vec<_> = records
                    .iter()
                    .filter(|record| super::commit_identity::accepted_event_id(record).is_some())
                    .flat_map(|record| record.events.iter().map(|event| event.crtc))
                    .collect();
                accepted_crtcs.sort_by_key(|crtc| u32::from(*crtc));
                let mut expected_crtcs = vec![output0_crtc, output1_crtc];
                expected_crtcs.sort_by_key(|crtc| u32::from(*crtc));
                assert_eq!(
                    accepted_crtcs, expected_crtcs,
                    "{row}: degraded pass accepts both unrelated scene and replacement CRTCs"
                );
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1].degraded_composed_unflip,
                    "{row}: EIO enters degraded replacement"
                );
                let output0_record = records
                    .iter()
                    .find(|record| {
                        super::commit_identity::accepted_event_id(record).is_some()
                            && a3_has_scene_commit_for(std::slice::from_ref(*record), output0_crtc)
                    })
                    .expect("unrelated output 0 composition is accepted in degraded pass");
                let replacement = records
                    .iter()
                    .find(|record| {
                        super::commit_identity::accepted_event_id(record).is_some()
                            && a3_has_scene_commit_for(std::slice::from_ref(*record), output1_crtc)
                    })
                    .expect("degraded output 1 replacement is accepted");
                let replacement_id =
                    super::commit_identity::accepted_event_id(replacement).unwrap();
                assert_eq!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .unflip_awaiting_outputs
                        .get(&1),
                    Some(&replacement_id),
                    "{row}: accepted degraded replacement identity is tracked"
                );
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .unflip_outputs_to_replace
                        .contains(&1),
                    "{row}: degraded replacement obligation remains until retirement"
                );
                a3_retire_records(&mut fixture, std::slice::from_ref(output0_record));
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .is_some(),
                    "{row}: unrelated output retirement leaves degraded owner active"
                );
                a3_retire_records(&mut fixture, std::slice::from_ref(replacement));
                assert!(
                    fixture.events.backend.pstate.direct.outputs[1]
                        .current
                        .is_none(),
                    "{row}: replacement retirement releases degraded owner"
                );
                assert!(
                    fixture.events.backend.present_source_pins.is_empty(),
                    "{row}: degraded retirement releases source pins"
                );
            }
            _ => unreachable!("known Q06 row"),
        }
        fixture.finish_after_normal_drain();
    }
}
#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: A17 invalidates every output's buffers; it also marks output 0
// structurally dirty during fresh preparation, retirement, degraded setup, or retry.
fn output_unflip_preserves_other_damage() {
    use yserver_core::backend::Backend;

    fn request_output_unflip(
        fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixtureGuard,
        direct: &super::commit_identity::recording_scene::DirectPresentSetup,
        output_idx: usize,
        row: &str,
    ) {
        let mut successor = direct.candidate;
        successor.present_id += 1;
        successor.valid_region_xid = 0x55;
        let mut event = direct.event.clone();
        event.present_id = successor.present_id;
        event.serial = event.serial.wrapping_add(1);
        assert!(
            !Backend::try_present_direct(&mut fixture.events.backend, successor, event)
                .expect("ineligible Output successor through Backend"),
            "{row}: successor requests composed unflip"
        );
        assert!(
            fixture.events.backend.pstate.direct.outputs[output_idx].unflip_requested,
            "{row}: ineligible successor marks its output for unflip"
        );
    }

    fn settle_other_output(
        fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixtureGuard,
        output0_crtc: ::drm::control::crtc::Handle,
        row: &str,
    ) {
        for _ in 0..3 {
            let records = a3_tick(fixture);
            let has_output0_scene = a3_has_scene_commit_for(&records, output0_crtc);
            a3_retire_records(fixture, &records);
            if !has_output0_scene {
                break;
            }
        }
        assert!(
            !fixture.events.backend.scene.has_pending_page_flip(0),
            "{row}: output 0 starts with no scene acknowledgement in flight"
        );
    }

    for (row, present_id, degraded) in [("fresh", 958, false), ("degraded-retry", 959, true)] {
        let Some((fixture, direct)) = a3_output_fixture(1, present_id) else {
            eprintln!("SKIP output_unflip_preserves_other_damage: Vulkan fixture unavailable");
            return;
        };
        let mut fixture = fixture.cleanup_guard();
        let fd = fixture.kms_fd(0);
        a3_enter_output_owner(&mut fixture, &direct, 1);
        let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
        settle_other_output(&mut fixture, output0_crtc, row);
        let output0_before = fixture
            .events
            .backend
            .scene
            .retirement_observation_for_tests(0);
        assert!(
            output0_before.contains("structure_damage=RegionSet { rects: [] }"),
            "{row}: output 0 starts clean: {output0_before}"
        );
        request_output_unflip(&mut fixture, &direct, 1, row);

        let recorder = fixture.events.recorders[&fd].clone();
        if degraded {
            recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
            recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
            // Reject the first per-output replacement too, forcing the Backend retry path.
            recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
        }
        let setup_start = recorder.records().len();
        Backend::maybe_composite(&mut fixture.events.backend)
            .expect("prepare output 1 composed unflip through Backend");
        fixture.events.wait_for_submitted_gpu_work();
        let setup_records = a3_records_since(&fixture, fd, setup_start);

        if !degraded {
            assert!(
                fixture.events.backend.pstate.direct.outputs[1].fresh_composed_unflip,
                "{row}: output 1 enters the fresh replacement path"
            );
            assert!(
                !a3_has_scene_commit_for(&setup_records, output0_crtc),
                "{row}: fresh preparation does not repaint output 0"
            );
            assert_eq!(
                fixture
                    .events
                    .backend
                    .scene
                    .retirement_observation_for_tests(0),
                output0_before,
                "{row}: fresh preparation preserves output 0 buffer history and structure damage"
            );
            let replacement = setup_records
                .iter()
                .find(|record| {
                    super::commit_identity::accepted_event_id(record).is_some()
                        && a3_has_scene_commit_for(
                            std::slice::from_ref(*record),
                            fixture.events.backend.platform.outputs[1].output.crtc,
                        )
                })
                .expect("fresh output 1 replacement accepted");
            a3_retire_records(&mut fixture, std::slice::from_ref(replacement));
            assert!(
                fixture.events.backend.pstate.direct.outputs[1]
                    .current
                    .is_none(),
                "{row}: fresh replacement retirement releases the output 1 owner"
            );
            assert_eq!(
                fixture
                    .events
                    .backend
                    .scene
                    .retirement_observation_for_tests(0),
                output0_before,
                "{row}: fresh replacement retirement preserves output 0 history and structure damage"
            );
        } else {
            let owner = &fixture.events.backend.pstate.direct.outputs[1];
            assert!(
                owner.degraded_composed_unflip,
                "{row}: scripted EIO enters degraded replacement"
            );
            assert!(
                owner.unflip_awaiting_outputs.is_empty(),
                "{row}: rejected degraded setup has no accepted replacement identity"
            );
            assert!(
                owner.unflip_outputs_to_replace.contains(&1),
                "{row}: output 1 replacement obligation remains tracked"
            );
            assert!(
                owner.current.is_some(),
                "{row}: owner remains active before replacement retirement"
            );
            assert_eq!(
                fixture.events.backend.pstate.direct.active_scopes(),
                [crate::kms::render::backend::DirectScope::Output(1)],
                "{row}: degraded work remains reachable through its owner scope"
            );
            assert!(
                !a3_has_scene_commit_for(&setup_records, output0_crtc),
                "{row}: degraded setup does not repaint output 0"
            );
            assert_eq!(
                fixture
                    .events
                    .backend
                    .scene
                    .retirement_observation_for_tests(0),
                output0_before,
                "{row}: degraded setup preserves output 0 history and structure damage"
            );
            assert!(
                setup_records.iter().any(|record| {
                    record.result != crate::drm::atomic::tests_support::ScriptedResult::Ok
                        && a3_has_scene_commit_for(
                            std::slice::from_ref(record),
                            fixture.events.backend.platform.outputs[1].output.crtc,
                        )
                }),
                "{row}: recorder confirms the first output 1 replacement was rejected"
            );
            assert!(
                setup_records
                    .iter()
                    .all(|record| super::commit_identity::accepted_event_id(record).is_none()),
                "{row}: setup contains no accepted replacement event"
            );

            let retry_deadline = Backend::next_wakeup(&fixture.events.backend)
                .expect("rejected replacement schedules a Backend retry");
            let until_retry = retry_deadline.saturating_duration_since(std::time::Instant::now());
            if !until_retry.is_zero() {
                std::thread::sleep(until_retry);
            }
            Backend::before_block(&mut fixture.events.backend);
            let retry_start = recorder.records().len();
            Backend::maybe_composite(&mut fixture.events.backend)
                .expect("retry the rejected degraded replacement through Backend");
            fixture.events.wait_for_submitted_gpu_work();
            let retry_records = a3_records_since(&fixture, fd, retry_start);
            assert!(
                !a3_has_scene_commit_for(&retry_records, output0_crtc),
                "{row}: degraded retry does not repaint output 0"
            );
            assert_eq!(
                fixture
                    .events
                    .backend
                    .scene
                    .retirement_observation_for_tests(0),
                output0_before,
                "{row}: degraded retry preserves output 0 history and structure damage"
            );
            let replacement = retry_records
                .iter()
                .find(|record| {
                    super::commit_identity::accepted_event_id(record).is_some()
                        && a3_has_scene_commit_for(
                            std::slice::from_ref(*record),
                            fixture.events.backend.platform.outputs[1].output.crtc,
                        )
                })
                .expect("degraded retry accepts output 1 replacement");
            a3_retire_records(&mut fixture, std::slice::from_ref(replacement));
            assert!(
                fixture.events.backend.pstate.direct.outputs[1]
                    .current
                    .is_none(),
                "{row}: retry retirement releases output 1's owner"
            );
            assert_eq!(
                fixture
                    .events
                    .backend
                    .scene
                    .retirement_observation_for_tests(0),
                output0_before,
                "{row}: retry replacement retirement keeps output 0 history and structure damage"
            );
        }
        fixture.finish_after_normal_drain();
    }
}
#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: use the global scene predicate when deciding whether to skip output 0's cursor-only commit.
fn retained_direct_damage_does_not_block_other_output_cursor_commit() {
    use yserver_core::backend::Backend;

    let Some(mut atomic_fixture) =
        super::cursor_sprites::support::AtomicPathFixture::with_output_count(
            super::cursor_sprites::support::PathKind::Physical,
            2,
        )
        .expect("two-output cursor scheduling fixture construction")
    else {
        eprintln!(
            "SKIP retained_direct_damage_does_not_block_other_output_cursor_commit: Vulkan fixture unavailable"
        );
        return;
    };
    let mut delivered = std::collections::HashSet::new();
    let cursor = super::cursor_sprites::support::create_bitmap_cursor(
        &mut atomic_fixture.scene.events.backend,
        true,
    );
    super::cursor_sprites::support::establish_hardware_cursor_on_output(
        &mut atomic_fixture,
        &mut delivered,
        cursor,
        0,
        (100, 100),
    );
    for output_idx in 0..2 {
        super::cursor_sprites::support::drain_unretired_output_events(
            &mut atomic_fixture,
            &mut delivered,
            output_idx,
        );
    }
    let direct = atomic_fixture
        .scene
        .install_direct_output_present(1, 958, 0);
    let fixture = &mut atomic_fixture.scene;
    let device_fd = fixture.kms_fd(0);
    let output1 = &fixture.events.backend.platform.outputs[1];
    let (x, y, width, height) = (
        output1.x as i16,
        output1.y as i16,
        output1.width,
        output1.height,
    );
    a3_add_existing_window_to_state(
        &mut fixture.events.state,
        yserver_protocol::x11::ResourceId(0x31a0),
        direct.target_xid,
        x,
        y,
        width,
        height,
        true,
    );
    Backend::sync_top_level_order(&mut fixture.events.backend, &fixture.events.state);
    Backend::set_cursor_hidden(&mut fixture.events.backend, false);
    for _ in 0..4 {
        Backend::maybe_composite(&mut fixture.events.backend)
            .expect("settle output 0's hardware cursor through Backend");
        fixture.events.wait_for_submitted_gpu_work();
        for output_idx in 0..2 {
            super::cursor_sprites::support::drain_output_events(
                &mut fixture.events,
                &mut delivered,
                output_idx,
            );
        }
    }
    assert_eq!(
        fixture.events.backend.scene.cursor_mode_for_outputs(&[0]),
        crate::kms::render::scene::CursorPlaneMode::Hw,
        "output 0 has an attached hardware cursor before Output(1) enters direct scanout"
    );
    Backend::warp_pointer_root(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        100,
        100,
    );
    super::commit_identity::direct_unflip::damage_scene_root(fixture);
    for _ in 0..4 {
        Backend::maybe_composite(&mut fixture.events.backend)
            .expect("compose the output window and establish output 0's cursor");
        fixture.events.wait_for_submitted_gpu_work();
        for output_idx in 0..2 {
            super::cursor_sprites::support::drain_output_events(
                &mut fixture.events,
                &mut delivered,
                output_idx,
            );
        }
    }
    let direct_records = a3_admit_output_owner(fixture, &direct, 1);
    let direct_id = direct_records
        .iter()
        .find_map(super::commit_identity::accepted_event_id)
        .expect("Output(1) direct request is accepted");
    assert_eq!(
        super::cursor_sprites::support::retire_pending_output(
            &mut fixture.events,
            &mut delivered,
            1,
            40,
        ),
        direct_id,
        "Output(1)'s direct owner retires by its accepted event ID"
    );
    assert_eq!(
        Backend::drain_completed_present_events(&mut fixture.events.backend)
            .into_iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        [direct.candidate.present_id]
    );

    Backend::fill_rectangle(
        &mut fixture.events.backend,
        None,
        direct.candidate.src_host_xid,
        0x0000_8fda,
        0,
        0,
        width,
        height,
    )
    .expect("paint the output-sized Present source through Backend");
    Backend::copy_area(
        &mut fixture.events.backend,
        None,
        direct.candidate.src_host_xid,
        direct.target_xid,
        0,
        0,
        0,
        0,
        width,
        height,
    )
    .expect("retain output-window damage through the Present Copy fallback");
    Backend::note_present_pixmap(
        &mut fixture.events.backend,
        direct.candidate.src_pixmap_xid,
        direct.target_xid,
    );
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("compose output 0 and clear the global wake while output 1 is direct");
    fixture.events.wait_for_submitted_gpu_work();
    assert!(
        !fixture.events.backend.scene.scene_structure_dirty,
        "the scheduler clears the global wake after output 0 inspects its clean scene"
    );
    for output_idx in 0..2 {
        super::cursor_sprites::support::drain_output_events(
            &mut fixture.events,
            &mut delivered,
            output_idx,
        );
    }
    assert!(
        fixture
            .events
            .backend
            .scene
            .output_wants_compose(1, &fixture.events.backend.store),
        "output 1 retains its direct-window damage; scene_dirty={}, armed={:?}",
        fixture.events.backend.scene.scene_structure_dirty,
        fixture.events.backend.store.armed_damaged_ids()
    );
    assert!(
        !fixture
            .events
            .backend
            .scene
            .output_wants_compose(0, &fixture.events.backend.store),
        "output 0 has no scene work"
    );

    Backend::warp_pointer_root(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        110,
        100,
    );
    assert!(
        Backend::next_wakeup(&fixture.events.backend)
            .is_some_and(|wakeup| wakeup <= std::time::Instant::now()),
        "Backend exposes the unsent pointer position as an immediate wake"
    );
    let before = fixture.events.recorders[&device_fd].records().len();

    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("schedule output 0 cursor motion while output 1 damage is retained");
    fixture.events.wait_for_submitted_gpu_work();

    let records = a3_records_since(fixture, device_fd, before);
    let cursor_only: Vec<_> = records
        .iter()
        .filter(|record| {
            record.kind == crate::drm::atomic::AtomicCommitKind::CursorOnly
                && super::commit_identity::accepted_event_id(record).is_some()
                && record.events.iter().any(|event| {
                    event.crtc == fixture.events.backend.platform.outputs[0].output.crtc
                })
        })
        .collect();
    assert!(
        cursor_only.len() == 1,
        "pointer motion submits one output 0 cursor-only request: cursor={cursor_only:?}, all={records:?}"
    );
    assert!(
        fixture
            .events
            .backend
            .scene
            .output_wants_compose(1, &fixture.events.backend.store),
        "output 1's retained damage remains pending"
    );

    for output_idx in 0..2 {
        super::cursor_sprites::support::drain_output_events(
            &mut fixture.events,
            &mut delivered,
            output_idx,
        );
    }

    Backend::unmap_subwindow(&mut fixture.events.backend, None, direct.target_xid)
        .expect("request Output(1) cleanup unflip through Backend");
    super::commit_identity::direct_unflip::damage_scene_root(fixture);
    let cleanup_start = fixture.events.recorders[&device_fd].records().len();
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("replace Output(1)'s direct scanout");
    fixture.events.wait_for_submitted_gpu_work();
    let cleanup = a3_records_since(fixture, device_fd, cleanup_start);
    for output_idx in 0..2 {
        super::cursor_sprites::support::drain_output_events(
            &mut fixture.events,
            &mut delivered,
            output_idx,
        );
    }
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_none(),
        "cleanup retires the Output(1) owner; state={:?}, scene_pending={}, kms_pending={}, cursor={:?}, cleanup={cleanup:?}",
        (
            fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
            fixture.events.backend.pstate.direct.outputs[1]
                .unflip_outputs_to_replace
                .len(),
            fixture.events.backend.pstate.direct.outputs[1]
                .unflip_awaiting_outputs
                .len(),
        ),
        fixture.events.backend.scene.has_pending_page_flip(1),
        fixture
            .events
            .backend
            .platform
            .scanout_commit_in_flight_for_output(1),
        fixture.events.backend.scene.cursor_mode_for_outputs(&[1]),
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    assert!(Backend::drain_completed_present_events(&mut fixture.events.backend).is_empty());
    assert_eq!(
        Backend::drain_retired_present_idle_events(&mut fixture.events.backend)
            .into_iter()
            .map(|event| event.present_id)
            .collect::<Vec<_>>(),
        [direct.candidate.present_id],
        "Output(1)'s idle notification follows its completed replacement"
    );
    Backend::set_cursor_hidden(&mut fixture.events.backend, true);
    super::commit_identity::direct_unflip::damage_scene_root(fixture);
    for _ in 0..4 {
        Backend::maybe_composite(&mut fixture.events.backend)
            .expect("detach the fixture cursor through Backend");
        fixture.events.wait_for_submitted_gpu_work();
        for output_idx in 0..2 {
            super::cursor_sprites::support::drain_output_events(
                &mut fixture.events,
                &mut delivered,
                output_idx,
            );
        }
    }
    assert!(!fixture.events.backend.platform.any_cursor_plane_visible());
    super::cursor_sprites::support::finish_atomic_fixture(atomic_fixture, &mut delivered);
}
