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
