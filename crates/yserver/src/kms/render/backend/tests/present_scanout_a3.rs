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

fn a3_enter_output_owner(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    direct: &super::commit_identity::recording_scene::DirectPresentSetup,
    output_idx: usize,
) {
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
// Mutation: restore the root-wide unflip request for every authoritative Present.
fn shaped_away_stage_present_keeps_output_owner() {
    use yserver_core::backend::Backend;
    use yserver_protocol::x11::{ResourceId, xfixes::RegionRect};

    let Some((mut fixture, direct)) = a3_output_fixture(1, 940) else {
        eprintln!("SKIP shaped_away_stage_present_keeps_output_owner: Vulkan fixture unavailable");
        return;
    };
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
            width: 1920,
            height: 1080,
        }]),
    )
    .expect("shape the stage to output 0 through Backend");
    let stage = a3_install_root_present(&mut fixture, 941, cow_xid);
    a3_present_root_copy(&mut fixture, &stage, 0x00d1_7b31);

    let owner = &fixture.events.backend.pstate.direct.outputs[1];
    assert!(
        !owner.unflip_requested,
        "stage shaped away from output 1 is hidden there"
    );
    assert_eq!(
        owner.current.as_ref().unwrap().candidate.present_id,
        current_present,
        "Output(1) keeps the same current direct Present"
    );
    let records = a3_tick(&mut fixture);
    assert!(
        a3_has_scene_commit_for(
            &records,
            fixture.events.backend.platform.outputs[0].output.crtc
        ),
        "output 0 composes the COW stage"
    );
    assert!(
        !a3_has_scene_commit_for(
            &records,
            fixture.events.backend.platform.outputs[1].output.crtc
        ),
        "output 1 remains direct"
    );
    a3_retire_records(&mut fixture, &records);
    assert_eq!(
        &a3_visible_root_pixel(&mut fixture, 150, 150)[..3],
        &[0x31, 0x7b, 0xd1],
        "output 0 displays the stage pixels"
    );
    assert!(!fixture.events.backend.pstate.direct.outputs[1].unflip_requested);
    assert_eq!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .as_ref()
            .unwrap()
            .candidate
            .present_id,
        current_present,
        "Output(1) stays current through output 0's retirement"
    );
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: treat every shaped COW as hidden from every output.
fn stage_present_visible_on_output_revokes_it() {
    use yserver_core::backend::Backend;
    use yserver_protocol::x11::{ResourceId, xfixes::RegionRect};

    let Some((mut fixture, direct)) = a3_output_fixture(1, 942) else {
        eprintln!("SKIP stage_present_visible_on_output_revokes_it: Vulkan fixture unavailable");
        return;
    };
    let mut state = yserver_core::server::ServerState::new();
    let output = &fixture.events.backend.platform.outputs[1];
    a3_add_existing_window_to_state(
        &mut state,
        ResourceId(0x3161),
        direct.target_xid,
        output.x as i16,
        output.y as i16,
        output.width,
        output.height,
        true,
    );
    Backend::sync_top_level_order(&mut fixture.events.backend, &state);
    a3_enter_output_owner(&mut fixture, &direct, 1);

    let cow_xid = a3_materialize_cow(&mut fixture, &mut state);
    Backend::set_shape_rectangles(
        &mut fixture.events.backend,
        None,
        cow_xid,
        0,
        Some(&[RegionRect {
            x: 0,
            y: 0,
            width: 3840,
            height: 1080,
        }]),
    )
    .expect("shape the COW across both outputs through Backend");
    let stage = a3_install_root_present(&mut fixture, 943, cow_xid);
    a3_present_root_copy(&mut fixture, &stage, 0x0000_8fda);

    let owner = &fixture.events.backend.pstate.direct.outputs[1];
    assert!(owner.unflip_requested, "the stage intersects output 1");
    assert_eq!(
        owner.unflip_reason,
        Some("ineligible_authoritative_root_present")
    );
    assert_eq!(owner.unflip_fallback_source, None);
    assert!(
        owner.unflip_shadow_ready,
        "the Copy prepared the replacement"
    );
    assert!(
        owner.current.is_some(),
        "the last direct frame awaits replacement"
    );
    let records = a3_tick(&mut fixture);
    assert!(
        a3_has_scene_commit_for(
            &records,
            fixture.events.backend.platform.outputs[1].output.crtc
        ),
        "the next scheduler pass composes output 1's replacement"
    );
    a3_retire_records(&mut fixture, &records);
    fixture.finish();
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
// Mutation: on_top_level_order_changed unflips every owner.
fn restack_on_other_output_keeps_output_owner() {
    use yserver_core::{backend::Backend, server::ServerState};
    use yserver_protocol::x11::ResourceId;

    let Some((mut fixture, direct)) = a3_output_fixture(1, 930) else {
        eprintln!("SKIP restack_on_other_output_keeps_output_owner: Vulkan fixture unavailable");
        return;
    };
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
    a3_add_existing_window_to_state(
        &mut state,
        ResourceId(0x3102),
        direct.target_xid,
        1920,
        0,
        1920,
        1080,
        true,
    );
    Backend::sync_top_level_order(&mut fixture.events.backend, &state);
    a3_enter_output_owner(&mut fixture, &direct, 1);
    let before = fixture.events.backend.pstate.direct.outputs[1]
        .current
        .as_ref()
        .unwrap()
        .candidate
        .present_id;

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
        "output 0 restack leaves Output(1) held"
    );
    assert_eq!(owner.current.as_ref().unwrap().candidate.present_id, before);
    assert!(
        a3_has_scene_commit_for(
            &records,
            fixture.events.backend.platform.outputs[0].output.crtc
        ),
        "output 0 composes the changed top-level order"
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
        "the lower window raised above the upper window is visible"
    );
    assert_eq!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .as_ref()
            .unwrap()
            .candidate
            .present_id,
        before,
        "Output(1) remains current through the other output's scene retirement"
    );
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: the tick revalidation skips the stacking term.
fn raise_over_output_owner_revokes_it() {
    use yserver_core::{backend::Backend, server::ServerState};
    use yserver_protocol::x11::ResourceId;

    let Some((mut fixture, direct)) = a3_output_fixture(1, 931) else {
        eprintln!("SKIP raise_over_output_owner_revokes_it: Vulkan fixture unavailable");
        return;
    };
    let mut state = ServerState::new();
    let raised = a3_seed_ordered_window(
        &mut fixture,
        &mut state,
        ResourceId(0x3110),
        1920,
        0,
        1920,
        1080,
        true,
    );
    a3_add_existing_window_to_state(
        &mut state,
        ResourceId(0x3111),
        direct.target_xid,
        1920,
        0,
        1920,
        1080,
        true,
    );
    Backend::sync_top_level_order(&mut fixture.events.backend, &state);
    a3_enter_output_owner(&mut fixture, &direct, 1);

    a3_restack_top_level(
        &mut fixture,
        &mut state,
        ResourceId(0x3110),
        ResourceId(0x3111),
        raised,
        direct.target_xid,
        0,
    );
    assert!(
        !fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
        "Output owners wait for the scene check at the scheduler tick"
    );
    let records = a3_tick(&mut fixture);
    let owner = &fixture.events.backend.pstate.direct.outputs[1];
    assert!(
        owner.unflip_requested,
        "raised intersecting window revokes Output(1)"
    );
    assert_eq!(
        owner.unflip_reason,
        Some("composite_tick_output_scene_changed")
    );
    assert!(
        a3_has_scene_commit_for(
            &records,
            fixture.events.backend.platform.outputs[1].output.crtc
        ),
        "the next tick submits the composed replacement on output 1"
    );
    a3_retire_records(&mut fixture, &records);
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: remove Output-owner scene revalidation from the scheduler tick.
fn map_over_paused_output_owner_revokes_without_present() {
    use yserver_core::{backend::Backend, server::ServerState};
    use yserver_protocol::x11::ResourceId;

    let Some((mut fixture, direct)) = a3_output_fixture(1, 932) else {
        eprintln!(
            "SKIP map_over_paused_output_owner_revokes_without_present: Vulkan fixture unavailable"
        );
        return;
    };
    a3_enter_output_owner(&mut fixture, &direct, 1);
    let mut state = ServerState::new();
    let other_output = a3_seed_ordered_window(
        &mut fixture,
        &mut state,
        ResourceId(0x3120),
        0,
        0,
        1920,
        1080,
        false,
    );
    Backend::map_subwindow(&mut fixture.events.backend, None, other_output)
        .expect("map the output 0 window through Backend");
    let output0_records = a3_tick(&mut fixture);
    assert!(
        !fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
        "mapping on output 0 leaves paused Output(1) held"
    );
    assert!(
        a3_has_scene_commit_for(
            &output0_records,
            fixture.events.backend.platform.outputs[0].output.crtc
        ),
        "output 0 composes its newly mapped window"
    );
    a3_retire_records(&mut fixture, &output0_records);

    let over_game = a3_seed_ordered_window(
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
        over_game,
        0x00d1_7b31,
        0,
        0,
        1920,
        1080,
    )
    .expect("paint a distinguishable notification overlay");
    Backend::map_subwindow(&mut fixture.events.backend, None, over_game)
        .expect("map the output 1 window through Backend");
    let output1_records = a3_tick(&mut fixture);
    let owner = &fixture.events.backend.pstate.direct.outputs[1];
    assert!(
        owner.unflip_requested,
        "mapping over a paused owner revokes it"
    );
    assert!(
        a3_has_scene_commit_for(
            &output1_records,
            fixture.events.backend.platform.outputs[1].output.crtc
        ),
        "the replacement is composed on output 1 without a new Present"
    );
    a3_retire_records(&mut fixture, &output1_records);
    let root = fixture.events.backend.core.window_id;
    let visible = fixture
        .events
        .backend
        .get_image_pixels_for_tests(root, 2, 1930, 20, 1, 1, !0)
        .expect("read output 1 after its replacement retires")
        .expect("replacement pixel bytes");
    assert_eq!(
        &visible[..3],
        &[0x31, 0x7b, 0xd1],
        "the composed replacement shows the mapped notification"
    );
    fixture.finish();
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
// Mutation: configure-time scope checks ignore geometry changes onto another output.
fn configure_window_onto_output_owner_revokes() {
    use yserver_core::{backend::Backend, host_x11::HostSubwindowConfig, server::ServerState};
    use yserver_protocol::x11::ResourceId;

    let Some((mut fixture, direct)) = a3_output_fixture(1, 933) else {
        eprintln!("SKIP configure_window_onto_output_owner_revokes: Vulkan fixture unavailable");
        return;
    };
    a3_enter_output_owner(&mut fixture, &direct, 1);
    let mut state = ServerState::new();
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
    let output0 = fixture.events.backend.pstate.direct.outputs[1]
        .current
        .as_ref()
        .unwrap()
        .candidate
        .present_id;
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
    let output0_records = a3_tick(&mut fixture);
    assert!(!fixture.events.backend.pstate.direct.outputs[1].unflip_requested);
    assert!(a3_has_scene_commit_for(
        &output0_records,
        fixture.events.backend.platform.outputs[0].output.crtc
    ));
    a3_retire_records(&mut fixture, &output0_records);
    assert_eq!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .as_ref()
            .unwrap()
            .candidate
            .present_id,
        output0
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
    .expect("move the already-above window onto output 1 through Backend");
    let output1_records = a3_tick(&mut fixture);
    assert!(fixture.events.backend.pstate.direct.outputs[1].unflip_requested);
    assert!(a3_has_scene_commit_for(
        &output1_records,
        fixture.events.backend.platform.outputs[1].output.crtc
    ));
    a3_retire_records(&mut fixture, &output1_records);
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: revalidate inside configure_subwindow before core reprojects stacking.
fn combined_configure_onto_output_and_below_game_keeps_owner() {
    use yserver_core::{backend::Backend, host_x11::HostSubwindowConfig, server::ServerState};
    use yserver_protocol::x11::{ConfigureWindowRequest, ResourceId};

    let Some((mut fixture, direct)) = a3_output_fixture(1, 934) else {
        eprintln!(
            "SKIP combined_configure_onto_output_and_below_game_keeps_owner: Vulkan fixture unavailable"
        );
        return;
    };
    let mut state = ServerState::new();
    let game_rid = ResourceId(0x3141);
    a3_add_existing_window_to_state(
        &mut state,
        game_rid,
        direct.target_xid,
        1920,
        0,
        1920,
        1080,
        true,
    );
    let mover = a3_seed_ordered_window(
        &mut fixture,
        &mut state,
        ResourceId(0x3140),
        100,
        0,
        500,
        500,
        true,
    );
    let mover_rid = ResourceId(0x3140);
    // The mover was created after the game and is currently above it, but it
    // does not intersect output 1 yet.
    Backend::sync_top_level_order(&mut fixture.events.backend, &state);
    a3_enter_output_owner(&mut fixture, &direct, 1);
    Backend::configure_subwindow(
        &mut fixture.events.backend,
        None,
        mover,
        HostSubwindowConfig {
            x: Some(2000),
            sibling: Some(direct.target_xid),
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
    assert!(
        !fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
        "the final ConfigureWindow order leaves the game topmost on output 1"
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .as_ref()
            .is_some_and(|frame| frame.candidate.present_id == direct.candidate.present_id)
    );
    a3_retire_records(&mut fixture, &records);
    fixture.finish();
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
// Mutation: request_direct_unflip_for_host_drawable revokes every owner.
fn drawable_cause_revokes_only_its_owner() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 dual-owner fixture")
    else {
        eprintln!("SKIP drawable_cause_revokes_only_its_owner: Vulkan fixture unavailable");
        return;
    };
    let first = fixture.install_direct_output_present(0, 936, 0);
    let second = fixture.install_direct_output_present(1, 937, 0);
    a3_enter_output_owner(&mut fixture, &first, 0);
    a3_enter_output_owner(&mut fixture, &second, 1);

    Backend::unmap_subwindow(&mut fixture.events.backend, None, first.target_xid)
        .expect("unmap Output(0) drawable through Backend");
    let output0 = &fixture.events.backend.pstate.direct.outputs[0];
    let output1 = &fixture.events.backend.pstate.direct.outputs[1];
    assert!(output0.unflip_requested, "the referenced owner is revoked");
    assert_eq!(output0.unflip_reason, Some("unmap_direct_frame_drawable"));
    assert!(!output1.unflip_requested, "Output(1) stays held");
    assert_eq!(
        output1.current.as_ref().unwrap().candidate.present_id,
        second.candidate.present_id
    );
    let records = a3_tick(&mut fixture);
    assert!(a3_has_scene_commit_for(
        &records,
        fixture.events.backend.platform.outputs[0].output.crtc
    ));
    a3_retire_records(&mut fixture, &records);
    let unaffected = &fixture.events.backend.pstate.direct.outputs[1];
    assert!(
        !unaffected.unflip_requested,
        "Output(1) remains held after retirement"
    );
    assert_eq!(
        unaffected.current.as_ref().unwrap().candidate.present_id,
        second.candidate.present_id,
        "Output(1) stays current through Output(0)'s retirement"
    );
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: direct_scope_scene_eligible accepts a target classified as Other.
fn redirect_of_held_output_owner_revokes_without_present() {
    use yserver_core::backend::Backend;

    let Some((mut fixture, direct)) = a3_output_fixture(1, 938) else {
        eprintln!(
            "SKIP redirect_of_held_output_owner_revokes_without_present: Vulkan fixture unavailable"
        );
        return;
    };
    a3_enter_output_owner(&mut fixture, &direct, 1);
    let before = fixture.events.backend.pstate.direct.outputs[1]
        .current
        .as_ref()
        .unwrap()
        .candidate
        .present_id;
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
        "redirected target is no longer direct eligible"
    );
    assert_eq!(
        owner.unflip_reason,
        Some("composite_tick_output_scene_changed")
    );
    assert_eq!(owner.current.as_ref().unwrap().candidate.present_id, before);
    assert!(a3_has_scene_commit_for(
        &records,
        fixture.events.backend.platform.outputs[1].output.crtc
    ));
    a3_retire_records(&mut fixture, &records);
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: on_direct_shape_changed revokes every direct scope.
fn shape_on_one_output_owner_revokes_only_it() {
    use yserver_core::backend::Backend;
    use yserver_protocol::x11::xfixes::RegionRect;

    let Some(mut fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 dual-owner fixture")
    else {
        eprintln!("SKIP shape_on_one_output_owner_revokes_only_it: Vulkan fixture unavailable");
        return;
    };
    let first = fixture.install_direct_output_present(0, 939, 0);
    let second = fixture.install_direct_output_present(1, 940, 0);
    a3_enter_output_owner(&mut fixture, &first, 0);
    a3_enter_output_owner(&mut fixture, &second, 1);

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
    .expect("clip Output(1) direct window through Backend SHAPE");
    assert!(
        !fixture.events.backend.pstate.direct.outputs[1].unflip_requested,
        "Output owners revalidate at the next scheduler pass"
    );
    let records = a3_tick(&mut fixture);
    assert!(fixture.events.backend.pstate.direct.outputs[1].unflip_requested);
    assert_eq!(
        fixture.events.backend.pstate.direct.outputs[1].unflip_reason,
        Some("composite_tick_output_scene_changed")
    );
    assert!(!fixture.events.backend.pstate.direct.outputs[0].unflip_requested);
    assert!(
        fixture.events.backend.pstate.direct.outputs[0]
            .current
            .is_some()
    );
    assert!(a3_has_scene_commit_for(
        &records,
        fixture.events.backend.platform.outputs[1].output.crtc
    ));
    a3_retire_records(&mut fixture, &records);
    fixture.finish();
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

fn a3_present_with_id(
    direct: &super::commit_identity::recording_scene::DirectPresentSetup,
    present_id: u64,
    options: u32,
) -> (
    yserver_core::backend::PresentScanoutCandidate,
    yserver_core::backend::CompletedPresentEvent,
) {
    let mut candidate = direct.candidate;
    candidate.present_id = present_id;
    candidate.options = options;
    let mut event = direct.event.clone();
    event.present_id = present_id;
    event.serial = u32::try_from(present_id).expect("Present serial fits");
    event.options = options;
    (candidate, event)
}

fn a3_copy_declined_present(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    direct: &super::commit_identity::recording_scene::DirectPresentSetup,
    present_id: u64,
    options: u32,
    color: u32,
) {
    use yserver_core::backend::Backend;

    let (candidate, event) = a3_present_with_id(direct, present_id, options);
    Backend::fill_rectangle(
        &mut fixture.events.backend,
        None,
        candidate.src_host_xid,
        color,
        100,
        100,
        1,
        1,
    )
    .expect("paint the latest async source through Backend");
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
    .expect("serve declined Present with Copy through Backend");
    Backend::note_present_pixmap(
        &mut fixture.events.backend,
        candidate.src_pixmap_xid,
        candidate.paint_dst_host_xid,
    );
    Backend::enqueue_present_completion(
        &mut fixture.events.backend,
        event,
        candidate.dst_window_xid,
    );
    fixture.events.wait_for_submitted_gpu_work();
    let completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(
        completed.len(),
        1,
        "each Copy Present completes exactly once"
    );
    assert_eq!(completed[0].present_id, present_id);
}

fn a3_damage_scene(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    window_xid: u32,
) {
    use yserver_core::backend::Backend;

    Backend::fill_rectangle(
        &mut fixture.events.backend,
        None,
        window_xid,
        0x0034_5678,
        2,
        2,
        1,
        1,
    )
    .expect("damage the output's candidate window through Backend");
}

fn a3_record_present(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    candidate: yserver_core::backend::PresentScanoutCandidate,
    event: yserver_core::backend::CompletedPresentEvent,
) -> Option<crate::drm::atomic::tests_support::AtomicCommitRecord> {
    use yserver_core::backend::Backend;

    let fd = fixture.kms_fd(0);
    let start = fixture.events.recorders[&fd].records().len();
    Backend::note_present_scanout_candidate(&mut fixture.events.backend, candidate);
    let accepted =
        Backend::try_present_direct(&mut fixture.events.backend, candidate, event.clone())
            .expect("Backend handles direct candidate");
    if !accepted {
        return None;
    }
    let event_id = fixture.events.backend.pstate.direct.outputs[fixture
        .events
        .backend
        .present_crtc_output(candidate.crtc_id)
        .unwrap()
        .0]
        .pending
        .as_ref()
        .and_then(|frame| frame.accepted_event_id);
    let record = a3_records_since(fixture, fd, start)
        .into_iter()
        .find(|record| {
            super::commit_identity::accepted_event_id(record) == event_id
                && record.events.iter().any(|flip| {
                    flip.crtc
                        == fixture.events.backend.platform.outputs[fixture
                            .events
                            .backend
                            .present_crtc_output(candidate.crtc_id)
                            .unwrap()
                            .0]
                            .output
                            .crtc
                })
        })
        .expect("accepted Present has a recorded CRTC commit");
    Some(record)
}

fn a3_arm_output_entry_hold(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    direct: &super::commit_identity::recording_scene::DirectPresentSetup,
    busy_property_commit: bool,
    repeated_busy_declines: usize,
    clock_elapsed_ns: Option<&std::sync::atomic::AtomicU64>,
) -> yserver_core::backend::PresentScanoutCandidate {
    use yserver_core::backend::Backend;

    let output_idx = fixture
        .events
        .backend
        .present_crtc_output(direct.candidate.crtc_id)
        .expect("fixture CRTC maps to output")
        .0;
    let output_crtc = fixture.events.backend.platform.outputs[output_idx]
        .output
        .crtc;
    fixture
        .events
        .backend
        .for_tests_set_scanout_m1_prober(record_scanout_m1_probe);

    for attempt in
        0..usize::from(crate::kms::render::backend::SCANOUT_M2_ELIGIBLE_ROOT_PROBATION - 1)
    {
        let present_id = direct.candidate.present_id + u64::try_from(attempt).unwrap();
        let (candidate, event) = a3_present_with_id(direct, present_id, direct.candidate.options);
        Backend::note_present_scanout_candidate(&mut fixture.events.backend, candidate);
        assert!(
            !Backend::try_present_direct(&mut fixture.events.backend, candidate, event)
                .expect("probationary Present uses Copy"),
            "the first seven Presents remain in M2 probation"
        );
        a3_copy_declined_present(
            fixture,
            direct,
            present_id,
            direct.candidate.options,
            0x0010_0000 + u32::try_from(attempt).unwrap(),
        );
        let records = a3_tick(fixture);
        assert!(
            a3_has_scene_commit_for(&records, output_crtc),
            "each probationary Copy damages and composes its output"
        );
        a3_retire_records(fixture, &records);
    }

    let busy_records = if busy_property_commit {
        let fd = fixture.kms_fd(output_idx);
        let recorder = fixture.events.recorders[&fd].clone();
        let (crtc_xid, output_key) = {
            let layout = &mut fixture.events.backend.platform.outputs[output_idx];
            layout.output.crtc_gamma_lut_prop =
                Some(super::commit_identity::recording_scene::raw(0x9a01));
            layout.output.crtc_degamma_lut_prop =
                Some(super::commit_identity::recording_scene::raw(0x9a02));
            layout.output.crtc_ctm_prop =
                Some(super::commit_identity::recording_scene::raw(0x9a03));
            (u32::from(layout.output.crtc), layout.key.clone())
        };
        fixture
            .events
            .backend
            .pstate
            .crtc_key_by_id
            .insert(crtc_xid, output_key);
        recorder.set_gamma_size_for_tests(4);
        let start = recorder.records().len();
        Backend::set_crtc_gamma(
            &mut fixture.events.backend,
            crtc_xid,
            &[0x1111; 4],
            &[0x2222; 4],
            &[0x3333; 4],
        )
        .expect("queue a CRTC property update through Backend");
        fixture.events.backend.platform.outputs[output_idx]
            .output
            .pending_gamma_deadline
            .set(Some(fixture.events.backend.monotonic_now()));
        let _ = a3_tick(fixture);
        let records = a3_records_since(fixture, fd, start);
        assert!(
            records.iter().any(|record| {
                record.kind == crate::drm::atomic::AtomicCommitKind::GammaOnly
                    && record.events.iter().any(|flip| flip.crtc == output_crtc)
            }),
            "Backend submits the output's property-only commit: {records:?}"
        );
        records
    } else {
        a3_damage_scene(fixture, direct.target_xid);
        let records = a3_tick(fixture);
        assert!(
            a3_has_scene_commit_for(&records, output_crtc),
            "a scene page flip is pending on the output before its final probationary Present"
        );
        records
    };

    let present_id = direct.candidate.present_id
        + u64::from(crate::kms::render::backend::SCANOUT_M2_ELIGIBLE_ROOT_PROBATION - 1);
    let (candidate, event) = a3_present_with_id(direct, present_id, direct.candidate.options);
    Backend::note_present_scanout_candidate(&mut fixture.events.backend, candidate);
    assert!(
        !Backend::try_present_direct(&mut fixture.events.backend, candidate, event)
            .expect("busy first entry remains on Copy path"),
        "the final probationary Present is declined while its output is busy"
    );
    a3_copy_declined_present(
        fixture,
        direct,
        present_id,
        direct.candidate.options,
        0x00d1_7b31,
    );
    for repeat in 0..repeated_busy_declines {
        if let Some(clock_elapsed_ns) = clock_elapsed_ns {
            clock_elapsed_ns.fetch_add(1_000_000, std::sync::atomic::Ordering::SeqCst);
        }
        let repeated_id = present_id + 1 + u64::try_from(repeat).unwrap();
        let (candidate, event) = a3_present_with_id(direct, repeated_id, direct.candidate.options);
        Backend::note_present_scanout_candidate(&mut fixture.events.backend, candidate);
        assert!(
            !Backend::try_present_direct(&mut fixture.events.backend, candidate, event)
                .expect("a scene flip remains in flight during the next unpaced Present"),
            "the in-flight composed flip keeps declining first-entry Presents"
        );
        a3_copy_declined_present(
            fixture,
            direct,
            repeated_id,
            direct.candidate.options,
            0x0012_3456,
        );
    }
    a3_retire_records(fixture, &busy_records);
    let held_records = a3_tick(fixture);
    assert!(
        !a3_has_scene_commit_for(&held_records, output_crtc),
        "the held output skips composition after the busy transaction retires"
    );
    a3_retire_records(fixture, &held_records);
    direct.candidate
}

fn a3_enter_async_output1_tearing_owner(
    fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixture,
    present_id: u64,
) {
    for device in &mut fixture.events.backend.platform.devices {
        device.async_page_flip = true;
    }
    let direct = fixture.install_direct_output_present(
        1,
        present_id,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    );
    a3_enter_output_owner(fixture, &direct, 1);
    assert!(fixture.events.backend.pstate.tearing.is_on());
    let (candidate, event) = a3_present_with_id(
        &direct,
        present_id + 10_000,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    );
    let fd = fixture.kms_fd(1);
    let start = fixture.events.recorders[&fd].records().len();
    assert!(
        yserver_core::backend::Backend::try_present_direct(
            &mut fixture.events.backend,
            candidate,
            event,
        )
        .expect("tearing owner accepts its follow-up Present")
    );
    let record = a3_records_since(fixture, fd, start)
        .into_iter()
        .find(|record| {
            record.kind == crate::drm::atomic::AtomicCommitKind::Async
                && record
                    .events
                    .iter()
                    .any(|flip| flip.crtc == fixture.events.backend.platform.outputs[1].output.crtc)
        })
        .expect("the tearing owner's follow-up is asynchronous");
    a3_retire_records(fixture, &[record]);
    let completed =
        yserver_core::backend::Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].present_id, present_id + 10_000);
    let current = fixture.events.backend.pstate.direct.outputs[1]
        .current
        .as_ref()
        .expect("Output(1) direct frame is current");
    assert_eq!(
        current.async_outcome,
        crate::drm::async_flip::AsyncOutcome::Accepted
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: remove the bounded hold or leave it set after successful direct entry.
fn unpaced_client_enters_direct_while_output_composes_scene_flip() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 dual-output fixture")
    else {
        eprintln!("SKIP unpaced_client_enters_direct_while_output_composes_scene_flip: Vulkan fixture unavailable");
        return;
    };
    a3_enter_async_output1_tearing_owner(&mut fixture, 960);
    let _clock =
        super::commit_identity::support::install_controlled_clock(&mut fixture.events.backend);
    let direct = fixture.install_direct_output_present(
        0,
        961,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    );
    a3_arm_output_entry_hold(&mut fixture, &direct, false, 0, None);

    let (candidate, event) = a3_present_with_id(&direct, 970, direct.candidate.options);
    let accepted = a3_record_present(&mut fixture, candidate, event);
    assert!(accepted.is_some(), "the next unpaced Present enters direct");
    let record = accepted.unwrap();
    assert_ne!(record.kind, crate::drm::atomic::AtomicCommitKind::Async);
    assert!(
        fixture.events.backend.pstate.direct.outputs[0]
            .entry_hold
            .is_none(),
        "successful direct entry consumes the output's hold"
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[0]
            .pending
            .as_ref()
            .is_some_and(|frame| frame.candidate.present_id == 970),
        "Output(0) owns the accepted Present"
    );
    a3_retire_records(&mut fixture, &[record]);
    let completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(completed.len(), 1, "direct Present completes exactly once");
    assert_eq!(completed[0].present_id, 970);
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_some()
    );
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: arm entry holds for scene flips but not an in-flight cursor/property commit.
fn unpaced_client_enters_direct_while_output_commit_in_flight() {
    let Some(mut fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 dual-output fixture")
    else {
        eprintln!("SKIP unpaced_client_enters_direct_while_output_commit_in_flight: Vulkan fixture unavailable");
        return;
    };
    a3_enter_async_output1_tearing_owner(&mut fixture, 980);
    let (_base, elapsed_ns) =
        super::commit_identity::support::install_controlled_clock(&mut fixture.events.backend);
    let direct = fixture.install_direct_output_present(
        0,
        981,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    );
    a3_arm_output_entry_hold(&mut fixture, &direct, true, 0, Some(&elapsed_ns));
    let (candidate, event) = a3_present_with_id(&direct, 990, direct.candidate.options);
    let record = a3_record_present(&mut fixture, candidate, event)
        .expect("next Present enters direct after the property commit retires");
    assert_ne!(record.kind, crate::drm::atomic::AtomicCommitKind::Async);
    assert!(
        fixture.events.backend.pstate.direct.outputs[0]
            .pending
            .is_some()
    );
    a3_retire_records(&mut fixture, &[record]);
    let completed =
        yserver_core::backend::Backend::drain_completed_present_events(&mut fixture.events.backend);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].present_id, 990);
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: renew the hold on each decline, omit its next_wakeup deadline, or rearm before a composed flip retires.
fn entry_hold_deadline_is_absolute_and_expires_without_events() {
    use std::sync::atomic::Ordering;
    use yserver_core::backend::Backend;

    let Some(mut fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 dual-output fixture")
    else {
        eprintln!("SKIP entry_hold_deadline_is_absolute_and_expires_without_events: Vulkan fixture unavailable");
        return;
    };
    let (base, elapsed_ns) =
        super::commit_identity::support::install_controlled_clock(&mut fixture.events.backend);
    let direct = fixture.install_direct_output_present(0, 1_000, 0);
    a3_arm_output_entry_hold(&mut fixture, &direct, false, 3, Some(&elapsed_ns));

    let interval = std::time::Duration::from_nanos(1_000_000_000 / 75);
    let deadline = base + interval * 3;
    assert_eq!(
        Backend::next_wakeup(&fixture.events.backend),
        Some(deadline),
        "repeated busy declines do not renew the absolute three-refresh deadline"
    );

    elapsed_ns.fetch_add(
        u64::try_from((interval * 3).as_nanos()).unwrap(),
        Ordering::SeqCst,
    );
    assert!(
        Backend::next_wakeup(&fixture.events.backend).is_some_and(|wake| wake <= deadline),
        "the expired hold is due without another client or device event"
    );
    let expired_records = a3_tick(&mut fixture);
    let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    assert!(
        a3_has_scene_commit_for(&expired_records, output0_crtc),
        "deadline expiry resumes output 0 composition"
    );

    let (candidate, event) = a3_present_with_id(&direct, 2_000, 0);
    assert!(
        !Backend::try_present_direct(&mut fixture.events.backend, candidate, event)
            .expect("pending composed flip keeps the retry on Copy"),
        "the output remains busy until its composed replacement retires"
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[0]
            .entry_hold
            .is_none(),
        "an expired slot cannot rearm while its composed replacement is pending"
    );
    a3_copy_declined_present(&mut fixture, &direct, 2_000, 0, 0x0065_4321);
    a3_retire_records(&mut fixture, &expired_records);
    let resumed_records = a3_tick(&mut fixture);
    assert!(
        a3_has_scene_commit_for(&resumed_records, output0_crtc),
        "one composed flip retires before the slot can rearm a hold"
    );
    a3_retire_records(&mut fixture, &resumed_records);
    assert!(
        !fixture.events.backend.pstate.direct.outputs[0].entry_hold_rearm_blocked,
        "retiring a composed output flip releases the rearm block"
    );
    assert_eq!(
        &a3_visible_root_pixel(&mut fixture, 100, 100)[..3],
        &[0x21, 0x43, 0x65],
        "the composed replacement displays the latest declined Copy"
    );
    fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: validate entry holds only when retained frames belong to active owners.
fn entry_hold_cleared_by_overlay_window_or_cursor() {
    use yserver_core::backend::{Backend, DrawState, GcFunction, OriginContext, SubwindowMode};
    use yserver_protocol::x11::{ClientId, ResourceId};

    let Some(mut mapped_fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 dual-output fixture")
    else {
        eprintln!("SKIP entry_hold_cleared_by_overlay_window_or_cursor: Vulkan fixture unavailable");
        return;
    };
    let _clock = super::commit_identity::support::install_controlled_clock(
        &mut mapped_fixture.events.backend,
    );
    let direct = mapped_fixture.install_direct_output_present(0, 1_010, 0);
    a3_arm_output_entry_hold(&mut mapped_fixture, &direct, false, 0, None);
    let mut state = yserver_core::server::ServerState::new();
    let resource = ResourceId(0x31a0);
    let overlay = a3_seed_ordered_window(
        &mut mapped_fixture,
        &mut state,
        resource,
        200,
        100,
        400,
        400,
        false,
    );
    Backend::map_subwindow(&mut mapped_fixture.events.backend, None, overlay)
        .expect("map an overlapping top-level through Backend");
    let _ = state.resources.map_window(resource);
    Backend::sync_top_level_order(&mut mapped_fixture.events.backend, &state);
    let records = a3_tick(&mut mapped_fixture);
    assert!(a3_has_scene_commit_for(
        &records,
        mapped_fixture.events.backend.platform.outputs[0]
            .output
            .crtc
    ));
    assert!(!mapped_fixture.events.backend.pstate.direct.outputs[0].active());
    let (candidate, event) = a3_present_with_id(&direct, 1_020, 0);
    assert!(
        !Backend::try_present_direct(&mut mapped_fixture.events.backend, candidate, event)
            .expect("overlapping window blocks direct re-entry")
    );
    a3_retire_records(&mut mapped_fixture, &records);
    mapped_fixture.finish();

    let Some(mut overlay_fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 second dual-output fixture")
    else {
        eprintln!("SKIP entry_hold_cleared_by_overlay_window_or_cursor: second Vulkan fixture unavailable");
        return;
    };
    let _clock = super::commit_identity::support::install_controlled_clock(
        &mut overlay_fixture.events.backend,
    );
    let overlay_direct = overlay_fixture.install_direct_output_present(0, 1_030, 0);
    a3_arm_output_entry_hold(&mut overlay_fixture, &overlay_direct, false, 0, None);
    let origin = Some(OriginContext {
        client_id: ClientId(14),
        nested_seq: 0,
        opcode: 70,
    });
    let overlay_root = overlay_fixture.events.backend.core.window_id;
    Backend::apply_draw_state(
        &mut overlay_fixture.events.backend,
        origin,
        &DrawState {
            function: GcFunction::Invert,
            subwindow_mode: SubwindowMode::IncludeInferiors,
            ..DrawState::default()
        },
    )
    .expect("select the reversible root overlay operation through Backend");
    Backend::fill_rectangle(
        &mut overlay_fixture.events.backend,
        origin,
        overlay_root,
        u32::MAX,
        10,
        10,
        50,
        1,
    )
    .expect("draw a root overlay over output 0 through Backend");
    let records = a3_tick(&mut overlay_fixture);
    assert!(a3_has_scene_commit_for(
        &records,
        overlay_fixture.events.backend.platform.outputs[0]
            .output
            .crtc
    ));
    assert!(!overlay_fixture.events.backend.pstate.direct.outputs[0].active());
    a3_retire_records(&mut overlay_fixture, &records);
    overlay_fixture.finish();

    let Some(mut cursor_fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 third dual-output fixture")
    else {
        eprintln!("SKIP entry_hold_cleared_by_overlay_window_or_cursor: third Vulkan fixture unavailable");
        return;
    };
    let _clock = super::commit_identity::support::install_controlled_clock(
        &mut cursor_fixture.events.backend,
    );
    let cursor_direct = cursor_fixture.install_direct_output_present(0, 1_040, 0);
    a3_arm_output_entry_hold(&mut cursor_fixture, &cursor_direct, false, 0, None);
    let cursor = super::cursor_sprites::support::create_argb_cursor(
        &mut cursor_fixture.events.backend,
        (128, 128),
        [0xff; 4],
        (4, 4),
    );
    let root = cursor_fixture.events.backend.core.window_id;
    Backend::define_cursor(
        &mut cursor_fixture.events.backend,
        None,
        root,
        cursor.as_raw(),
    )
    .expect("install a visible cursor through Backend");
    assert!(
        Backend::get_active_cursor_image(&cursor_fixture.events.backend).is_some(),
        "Backend has an active root cursor"
    );
    Backend::warp_pointer_root(
        &mut cursor_fixture.events.backend,
        &mut cursor_fixture.events.state,
        40,
        50,
    );
    Backend::set_cursor_hidden(&mut cursor_fixture.events.backend, false);
    let root_width = cursor_fixture.events.backend.platform.fb_w;
    let root_height = cursor_fixture.events.backend.platform.fb_h;
    Backend::clear_area(
        &mut cursor_fixture.events.backend,
        None,
        root,
        0,
        None,
        0,
        0,
        root_width,
        root_height,
        (0, 0),
    )
    .expect("damage the root through Backend with a visible software cursor");
    let records = a3_tick(&mut cursor_fixture);
    assert!(
        cursor_fixture.events.backend.pstate.direct.outputs[0]
            .entry_hold
            .is_none(),
        "software cursor transition clears the entry hold"
    );
    assert!(a3_has_scene_commit_for(
        &records,
        cursor_fixture.events.backend.platform.outputs[0]
            .output
            .crtc
    ));
    assert!(!cursor_fixture.events.backend.pstate.direct.outputs[0].active());
    a3_retire_records(&mut cursor_fixture, &records);
    assert_eq!(
        cursor_fixture
            .events
            .backend
            .scene
            .cursor_mode_for_outputs(&[0]),
        crate::kms::render::scene::CursorPlaneMode::Sw,
        "oversized visible cursor uses the software scene path"
    );
    cursor_fixture.finish();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: let an entry hold survive drawable destruction and global storage teardown.
fn entry_hold_cleared_by_destroy_and_global_causes() {
    use yserver_core::backend::Backend;

    let Some(mut destroy_fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 dual-output fixture")
    else {
        eprintln!("SKIP entry_hold_cleared_by_destroy_and_global_causes: Vulkan fixture unavailable");
        return;
    };
    let _clock = super::commit_identity::support::install_controlled_clock(
        &mut destroy_fixture.events.backend,
    );
    let direct = destroy_fixture.install_direct_output_present(0, 1_050, 0);
    a3_arm_output_entry_hold(&mut destroy_fixture, &direct, false, 0, None);
    Backend::destroy_subwindow(&mut destroy_fixture.events.backend, None, direct.target_xid)
        .expect("destroy the held window through Backend");
    assert!(
        destroy_fixture.events.backend.pstate.direct.outputs[0]
            .entry_hold
            .is_none(),
        "Backend destroy clears a hold naming that window synchronously"
    );
    let records = a3_tick(&mut destroy_fixture);
    assert!(a3_has_scene_commit_for(
        &records,
        destroy_fixture.events.backend.platform.outputs[0]
            .output
            .crtc
    ));
    assert!(!destroy_fixture.events.backend.pstate.direct.outputs[0].active());
    a3_retire_records(&mut destroy_fixture, &records);
    destroy_fixture.finish();

    let Some(mut storage_fixture) = super::commit_identity::recording_scene::RecordingSceneFixture::with_output_admission_topology()
        .expect("A3 second dual-output fixture")
    else {
        eprintln!("SKIP entry_hold_cleared_by_destroy_and_global_causes: second Vulkan fixture unavailable");
        return;
    };
    let _clock = super::commit_identity::support::install_controlled_clock(
        &mut storage_fixture.events.backend,
    );
    let storage_direct = storage_fixture.install_direct_output_present(0, 1_060, 0);
    a3_arm_output_entry_hold(&mut storage_fixture, &storage_direct, false, 0, None);
    Backend::set_logical_screen_size(&mut storage_fixture.events.backend, 3_841, 1_080)
        .expect("reallocate root storage through Backend");
    assert!(
        storage_fixture.events.backend.pstate.direct.outputs[0]
            .entry_hold
            .is_none(),
        "Backend storage reallocation clears all output holds synchronously"
    );
    let records = a3_tick(&mut storage_fixture);
    assert!(a3_has_scene_commit_for(
        &records,
        storage_fixture.events.backend.platform.outputs[0]
            .output
            .crtc
    ));
    assert!(!storage_fixture.events.backend.pstate.direct.outputs[0].active());
    a3_retire_records(&mut storage_fixture, &records);
    storage_fixture.finish();
}
