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

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: return for every submitted unflip; use the degraded-only mask; exclude the degraded output from scene readiness.
fn output_unflip_does_not_stall_other_output_compose() {
    use yserver_core::backend::Backend;

    let Some((fixture, direct)) = a3_output_fixture(1, 956) else {
        eprintln!(
            "SKIP output_unflip_does_not_stall_other_output_compose: Vulkan fixture unavailable"
        );
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    let device_fd = fixture.kms_fd(0);
    a3_enter_output_owner(&mut fixture, &direct, 1);
    Backend::unmap_subwindow(&mut fixture.events.backend, None, direct.target_xid)
        .expect("request Output(1) unflip through Backend");
    super::commit_identity::direct_unflip::damage_scene_root(&mut fixture);
    let before = fixture.events.recorders[&device_fd].records().len();

    Backend::maybe_composite(&mut fixture.events.backend).expect("schedule Output(1) unflip");
    fixture.events.wait_for_submitted_gpu_work();

    let records = a3_records_since(&fixture, device_fd, before);
    let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    let output1_crtc = fixture.events.backend.platform.outputs[1].output.crtc;
    assert!(
        a3_has_scene_commit_for(&records, output0_crtc),
        "output 0's ready scene damage composes in the same pass as Output(1)'s unflip"
    );
    let replacement = records
        .iter()
        .find(|record| {
            super::commit_identity::accepted_event_id(record).is_some()
                && record.events.iter().any(|event| event.crtc == output1_crtc)
        })
        .expect("Output(1) replacement accepted in the same scheduling pass");
    let replacement_id = super::commit_identity::accepted_event_id(replacement).unwrap();
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        records
            .iter()
            .find(|record| { a3_has_scene_commit_for(std::slice::from_ref(*record), output0_crtc) })
            .and_then(super::commit_identity::accepted_event_id),
        "output 0 owns its scene acknowledgement independently"
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .unflip_awaiting_outputs
            .contains_key(&1)
            || fixture.events.backend.scene.pending_page_flip_event_id(1) == Some(replacement_id),
        "the accepted replacement remains tied to output 1 until retirement"
    );

    a3_retire_records(&mut fixture, &records);
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_none(),
        "accepted output 1 replacement releases its direct owner"
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    fixture.finish_after_normal_drain();

    let Some((fixture, direct)) = a3_output_fixture(1, 957) else {
        eprintln!(
            "SKIP output_unflip_does_not_stall_other_output_compose: second Vulkan fixture unavailable"
        );
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    let device_fd = fixture.kms_fd(0);
    a3_enter_output_owner(&mut fixture, &direct, 1);
    Backend::unmap_subwindow(&mut fixture.events.backend, None, direct.target_xid)
        .expect("request degraded Output(1) unflip through Backend");
    super::commit_identity::direct_unflip::damage_scene_root(&mut fixture);
    let recorder = fixture.events.recorders[&device_fd].clone();
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    let before = recorder.records().len();

    Backend::maybe_composite(&mut fixture.events.backend).expect("degrade Output(1) unflip");
    fixture.events.wait_for_submitted_gpu_work();

    let records = a3_records_since(&fixture, device_fd, before);
    let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    let output1_crtc = fixture.events.backend.platform.outputs[1].output.crtc;
    assert!(fixture.events.backend.pstate.direct.outputs[1].degraded_composed_unflip);
    assert!(
        a3_has_scene_commit_for(&records, output0_crtc),
        "output 0 composes alongside the degraded output 1 replacement"
    );
    assert!(
        a3_has_scene_commit_for(&records, output1_crtc),
        "the degraded owner remains included until output 1's replacement is accepted"
    );
    let replacement = records
        .iter()
        .find(|record| {
            super::commit_identity::accepted_event_id(record).is_some()
                && record.events.iter().any(|event| event.crtc == output1_crtc)
        })
        .expect("degraded replacement accepted on output 1");
    let replacement_id = super::commit_identity::accepted_event_id(replacement).unwrap();
    assert_eq!(
        fixture.events.backend.pstate.direct.outputs[1]
            .unflip_awaiting_outputs
            .get(&1),
        Some(&replacement_id),
        "accepted replacement identity remains assigned to its output"
    );

    a3_retire_records(&mut fixture, &records);
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_none(),
        "retiring the accepted degraded replacement releases the owner"
    );
    assert!(fixture.events.backend.present_source_pins.is_empty());
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: invalidate every output's buffers; mark all outputs structurally dirty at fresh preparation, retirement, degraded setup, or degraded retry.
fn output_unflip_leaves_other_output_damage_alone() {
    use yserver_core::backend::Backend;

    fn request_output_unflip(
        fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixtureGuard,
        direct: &super::commit_identity::recording_scene::DirectPresentSetup,
        output_idx: usize,
    ) {
        let mut successor = direct.candidate;
        successor.present_id += 1;
        successor.valid_region_xid = 0x55;
        let mut event = direct.event.clone();
        event.present_id = successor.present_id;
        event.serial = event.serial.wrapping_add(1);
        assert!(
            !Backend::try_present_direct(&mut fixture.events.backend, successor, event,)
                .expect("ineligible Output successor through Backend"),
            "the successor requests composed unflip"
        );
        assert!(fixture.events.backend.pstate.direct.outputs[output_idx].unflip_requested);
    }

    fn settle_other_output(
        fixture: &mut super::commit_identity::recording_scene::RecordingSceneFixtureGuard,
    ) {
        let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
        for _ in 0..3 {
            let records = a3_tick(fixture);
            let has_output0_scene = a3_has_scene_commit_for(&records, output0_crtc);
            a3_retire_records(fixture, &records);
            if !has_output0_scene {
                break;
            }
        }
        assert!(!fixture.events.backend.scene.has_pending_page_flip(0));
    }

    let Some((fixture, direct)) = a3_output_fixture(1, 958) else {
        eprintln!(
            "SKIP output_unflip_leaves_other_output_damage_alone: Vulkan fixture unavailable"
        );
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    let fd = fixture.kms_fd(0);
    a3_enter_output_owner(&mut fixture, &direct, 1);
    settle_other_output(&mut fixture);
    let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    let output0_before = fixture
        .events
        .backend
        .scene
        .retirement_observation_for_tests(0);
    assert!(
        output0_before.contains("structure_damage=RegionSet { rects: [] }"),
        "output 0 starts without structure damage: {output0_before}"
    );
    request_output_unflip(&mut fixture, &direct, 1);
    let fresh_start = fixture.events.recorders[&fd].records().len();
    Backend::maybe_composite(&mut fixture.events.backend).expect("prepare fresh Output(1) unflip");
    fixture.events.wait_for_submitted_gpu_work();
    let fresh_records = a3_records_since(&fixture, fd, fresh_start);
    assert!(
        !a3_has_scene_commit_for(&fresh_records, output0_crtc),
        "fresh unflip preparation does not repaint output 0"
    );
    assert_eq!(
        fixture
            .events
            .backend
            .scene
            .retirement_observation_for_tests(0),
        output0_before,
        "fresh preparation leaves output 0's damage history and structural region unchanged"
    );
    assert!(fixture.events.backend.pstate.direct.outputs[1].fresh_composed_unflip);
    let fresh_replacement = fresh_records
        .iter()
        .find(|record| {
            super::commit_identity::accepted_event_id(record).is_some()
                && a3_has_scene_commit_for(
                    std::slice::from_ref(*record),
                    fixture.events.backend.platform.outputs[1].output.crtc,
                )
        })
        .expect("fresh output 1 replacement accepted");
    a3_retire_records(&mut fixture, std::slice::from_ref(fresh_replacement));
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_none()
    );
    assert_eq!(
        fixture
            .events
            .backend
            .scene
            .retirement_observation_for_tests(0),
        output0_before,
        "replacement retirement leaves output 0's damage history and structural region unchanged"
    );
    fixture.finish_after_normal_drain();

    let Some((fixture, direct)) = a3_output_fixture(1, 959) else {
        eprintln!(
            "SKIP output_unflip_leaves_other_output_damage_alone: degraded Vulkan fixture unavailable"
        );
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    let fd = fixture.kms_fd(0);
    a3_enter_output_owner(&mut fixture, &direct, 1);
    settle_other_output(&mut fixture);
    let output0_crtc = fixture.events.backend.platform.outputs[0].output.crtc;
    let output0_before = fixture
        .events
        .backend
        .scene
        .retirement_observation_for_tests(0);
    assert!(
        output0_before.contains("structure_damage=RegionSet { rects: [] }"),
        "output 0 starts without structure damage: {output0_before}"
    );
    request_output_unflip(&mut fixture, &direct, 1);
    let recorder = fixture.events.recorders[&fd].clone();
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    // Reject the first per-output replacement too, so the next scheduling
    // pass must reach the degraded retry through Backend bookkeeping.
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    let degraded_start = recorder.records().len();
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("degrade Output(1) to per-output scene replacement");
    fixture.events.wait_for_submitted_gpu_work();
    let degraded_records = a3_records_since(&fixture, fd, degraded_start);
    assert!(fixture.events.backend.pstate.direct.outputs[1].degraded_composed_unflip);
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .unflip_awaiting_outputs
            .is_empty(),
        "rejected degraded setup has no accepted replacement to retire"
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .unflip_outputs_to_replace
            .contains(&1),
        "output 1 remains an unflip replacement obligation until its event retires"
    );
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_some(),
        "the degraded owner remains active until the replacement event retires"
    );
    assert_eq!(
        fixture.events.backend.pstate.direct.active_scopes(),
        [crate::kms::render::backend::DirectScope::Output(1)],
        "the degraded retry remains reachable through its owner scope"
    );
    assert!(
        !a3_has_scene_commit_for(&degraded_records, output0_crtc),
        "degraded setup does not repaint output 0"
    );
    assert_eq!(
        fixture
            .events
            .backend
            .scene
            .retirement_observation_for_tests(0),
        output0_before,
        "degraded setup leaves output 0's damage history and structural region unchanged"
    );

    assert!(
        degraded_records.iter().any(|record| {
            record.result != crate::drm::atomic::tests_support::ScriptedResult::Ok
                && a3_has_scene_commit_for(
                    std::slice::from_ref(record),
                    fixture.events.backend.platform.outputs[1].output.crtc,
                )
        }),
        "the recorder shows the first Output(1) scene replacement was rejected"
    );
    assert!(
        degraded_records
            .iter()
            .all(|record| super::commit_identity::accepted_event_id(record).is_none()),
        "the scripted first replacement attempt is rejected"
    );

    let retry_deadline = Backend::next_wakeup(&fixture.events.backend)
        .expect("rejected replacement schedules a Backend retry wakeup");
    let until_retry = retry_deadline.saturating_duration_since(std::time::Instant::now());
    if !until_retry.is_zero() {
        // Follow the scheduler's reported KMS retry timer; the scripted
        // outcomes determine the rejection and acceptance independently.
        std::thread::sleep(until_retry);
    }
    Backend::before_block(&mut fixture.events.backend);
    let retry_start = recorder.records().len();
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("retry the rejected degraded Output(1) replacement through Backend");
    fixture.events.wait_for_submitted_gpu_work();
    let retry_records = a3_records_since(&fixture, fd, retry_start);
    assert!(
        !a3_has_scene_commit_for(&retry_records, output0_crtc),
        "degraded retry does not repaint output 0"
    );
    assert_eq!(
        fixture
            .events
            .backend
            .scene
            .retirement_observation_for_tests(0),
        output0_before,
        "degraded retry leaves output 0's damage history and structural region unchanged"
    );
    let retry_replacement = retry_records
        .iter()
        .find(|record| {
            super::commit_identity::accepted_event_id(record).is_some()
                && a3_has_scene_commit_for(
                    std::slice::from_ref(*record),
                    fixture.events.backend.platform.outputs[1].output.crtc,
                )
        })
        .expect("degraded retry accepts output 1's replacement");
    a3_retire_records(&mut fixture, std::slice::from_ref(retry_replacement));
    assert!(
        fixture.events.backend.pstate.direct.outputs[1]
            .current
            .is_none()
    );
    fixture.finish_after_normal_drain();
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
