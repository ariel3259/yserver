use super::support::AtomicCursorFixture;
use yserver_core::backend::{AnyHandle, Backend};

fn create_oversized_cursor(fixture: &mut AtomicCursorFixture) -> u32 {
    let source = Backend::create_pixmap(&mut fixture.backend, None, 1, 65, 65)
        .expect("create oversized cursor source");
    Backend::create_cursor(
        &mut fixture.backend,
        None,
        source,
        None,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        2,
        3,
    )
    .expect("create oversized software cursor")
    .as_raw()
}

fn create_argb_cursor(backend: &mut crate::kms::render::backend::KmsBackend, size: u16) -> u32 {
    let source =
        Backend::create_pixmap(backend, None, 32, size, size).expect("create ARGB cursor pixmap");
    let pixels = [0x57, 0x68, 0x79, 0xff].repeat(usize::from(size) * usize::from(size));
    Backend::put_image(
        backend,
        None,
        source.as_raw(),
        32,
        size,
        size,
        0,
        0,
        &pixels,
    )
    .expect("write ARGB cursor pixels");
    let picture =
        Backend::render_create_picture(backend, None, AnyHandle::Pixmap(source), 0, 0, &[])
            .expect("create ARGB cursor picture")
            .expect("picture handle");
    Backend::render_create_cursor(backend, None, picture, 0, 0)
        .expect("create ARGB cursor")
        .expect("cursor handle")
        .as_raw()
}

fn settle_scene_outputs(fixture: &mut AtomicCursorFixture, first_frame: u32) {
    for attempt in 0..32_u32 {
        Backend::maybe_composite(&mut fixture.backend).expect("drive scene through Backend");
        for output_idx in 0..fixture.backend.platform.outputs.len() {
            if fixture.backend.scene.has_pending_page_flip(output_idx)
                || fixture
                    .backend
                    .platform
                    .cursor_commit_in_flight_for_output(output_idx)
            {
                fixture.retire_output(output_idx, first_frame + attempt * 4 + output_idx as u32);
            }
        }
        if !fixture.backend.scene.has_pending_page_flips()
            && !fixture.backend.platform.any_cursor_commit_in_flight()
            && !fixture.backend.for_tests_scene_wants_scene()
        {
            return;
        }
    }
    panic!(
        "scene did not settle: pending_flips={}, cursor_in_flight={}, scene_wants_scene={}",
        fixture.backend.scene.has_pending_page_flips(),
        fixture.backend.platform.any_cursor_commit_in_flight(),
        fixture.backend.for_tests_scene_wants_scene(),
    );
}

fn create_spanning_window_through_core(fixture: &mut AtomicCursorFixture, window_xid: u32) {
    use std::{
        collections::{HashMap, HashSet, VecDeque},
        io::{Read, Write},
        os::unix::net::UnixStream,
        sync::{Arc, Mutex, atomic::AtomicU16},
        time::Duration,
    };
    use yserver_core::{
        core_loop::{
            CoreSender, Message, ResetPolicy, auth::AuthState, channel, client_reader,
            poll_tokens::ClientIdAllocator, run_core,
        },
        server::ClientState,
        transport::Transport,
    };
    use yserver_protocol::x11::{ClientByteOrder, ClientId, ResourceId};

    fn wire_request(opcode: u8, data: u8, body: &[u8]) -> Vec<u8> {
        let size = body.len() + 4;
        assert_eq!(size % 4, 0);
        let units = u16::try_from(size / 4).expect("request length fits");
        let mut bytes = vec![opcode, data];
        bytes.extend_from_slice(&units.to_le_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    fn create_window_body(window: u32, root: u32, x: i16, width: u16) -> Vec<u8> {
        let mut body = Vec::with_capacity(24);
        body.extend_from_slice(&window.to_le_bytes());
        body.extend_from_slice(&root.to_le_bytes());
        body.extend_from_slice(&x.to_le_bytes());
        body.extend_from_slice(&100_i16.to_le_bytes());
        body.extend_from_slice(&width.to_le_bytes());
        body.extend_from_slice(&300_u16.to_le_bytes());
        body.extend_from_slice(&0_u16.to_le_bytes());
        body.extend_from_slice(&1_u16.to_le_bytes());
        body.extend_from_slice(&yserver_core::resources::ROOT_VISUAL.0.to_le_bytes());
        body.extend_from_slice(&0_u32.to_le_bytes());
        body
    }

    let mut device_peers = Vec::new();
    for (index, device) in fixture.backend.platform.devices.iter_mut().enumerate() {
        let (device_fd, peer) = UnixStream::pair().expect("pollable fixture DRM fd");
        let replacement = std::rc::Rc::new(crate::drm::Device::from_inherited_kms_fd(
            std::os::fd::OwnedFd::from(device_fd),
            format!("<atomic cursor run_core device {index}>"),
        ));
        fixture
            .recorders
            .page_flip_events
            .register_device(&replacement);
        device.device = replacement;
        device_peers.push(peer);
    }

    let client_id = ClientId(1);
    let (transport, mut peer) = Transport::pair().expect("create run_core client transport");
    let writer = transport.try_clone().expect("clone core client writer");
    peer.set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set run_core response timeout");
    let (reader_control, reader_control_rx) = crossbeam_channel::unbounded();
    fixture.state.clients.insert(
        client_id.0,
        ClientState {
            writer: Arc::new(Mutex::new(writer)),
            is_local: true,
            fd_passing: true,
            internal: false,
            byte_order: ClientByteOrder::LittleEndian,
            last_sequence: Arc::new(AtomicU16::new(0)),
            resource_id_base: 0,
            resource_id_mask: u32::MAX,
            event_masks: HashMap::new(),
            save_set: HashSet::new(),
            big_requests_enabled: false,
            xi2_masks: HashMap::new(),
            xi1_event_classes: HashSet::new(),
            xi1_window_event_classes: HashMap::new(),
            outbound: VecDeque::new(),
            watching_writable: false,
            write_failed: false,
            focused_window: yserver_core::resources::ROOT_WINDOW,
            reader_control: Some(reader_control.clone()),
        },
    );
    let (poll, core_sender, receiver) = channel().expect("create run_core channel");
    client_reader::spawn(
        client_id,
        transport,
        ClientByteOrder::LittleEndian,
        0,
        reader_control_rx,
        core_sender.bind(),
    )
    .expect("spawn run_core X11 reader");
    let shutdown = core_sender.clone_handle();
    let window = ResourceId(window_xid);
    let cover = ResourceId(window_xid + 1);
    let root = yserver_core::resources::ROOT_WINDOW;
    let driver = std::thread::spawn(move || {
        struct ShutdownOnDrop(CoreSender);
        impl Drop for ShutdownOnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(Message::Shutdown);
            }
        }
        let _shutdown_on_drop = ShutdownOnDrop(shutdown);

        let mut requests = wire_request(1, 24, &create_window_body(window.0, root.0, 800, 1000));
        requests.extend_from_slice(&wire_request(8, 0, &window.0.to_le_bytes()));
        requests.extend_from_slice(&wire_request(
            1,
            24,
            &create_window_body(cover.0, root.0, 800, 800),
        ));
        requests.extend_from_slice(&wire_request(8, 0, &cover.0.to_le_bytes()));
        requests.extend_from_slice(&wire_request(43, 0, &[]));
        peer.write_all(&requests)
            .expect("send spanning and covering CreateWindow, MapWindow, and barrier");
        loop {
            let mut packet = [0; 32];
            peer.read_exact(&mut packet)
                .expect("read run_core response");
            if packet[0] == 0 {
                panic!("run_core rejected the window request: {packet:?}");
            }
            if packet[0] == 1 && u16::from_le_bytes([packet[2], packet[3]]) == 5 {
                break;
            }
        }
    });

    run_core(
        poll,
        receiver,
        core_sender.clone_handle(),
        &mut fixture.state,
        &mut fixture.backend,
        [],
        &ClientIdAllocator::new(),
        AuthState::new(None),
        ResetPolicy::NoReset,
        None,
    )
    .expect("run_core creates and maps the spanning drawable");
    driver.join().expect("join run_core request driver");
    let _ = reader_control.send(yserver_core::server::ReaderControl::Shutdown);
    let host_xid = fixture
        .state
        .resources
        .window(ResourceId(window_xid))
        .expect("run_core window resource")
        .host_xid
        .expect("run_core created the host window")
        .as_raw();
    assert!(fixture.backend.store.lookup(host_xid).is_some());
    drop(device_peers);
}

fn transparent_bitmap_cursor(backend: &mut crate::kms::render::backend::KmsBackend) -> u32 {
    let source = Backend::create_pixmap(backend, None, 1, 1, 1).expect("create bitmap source");
    let mask = Backend::create_pixmap(backend, None, 1, 1, 1).expect("create bitmap mask");
    Backend::put_image(backend, None, source.as_raw(), 1, 1, 1, 0, 0, &[0; 4])
        .expect("write transparent source");
    Backend::put_image(backend, None, mask.as_raw(), 1, 1, 1, 0, 0, &[0; 4])
        .expect("write transparent mask");
    Backend::create_cursor(
        backend,
        None,
        source,
        Some(mask),
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        0,
        0,
    )
    .expect("create transparent cursor")
    .as_raw()
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore the EINVAL-only condition; ENOSPC must still unflip and produce a Software receipt.
fn atomic_cursor_only_held_direct_enospc_attach_reveals_software_cursor() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent};
    use std::io;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build held-direct fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = super::round_a::setup_cursor_only_direct_present(&mut fixture);
    let root = fixture.backend.core.window_id;
    let transparent = transparent_bitmap_cursor(&mut fixture.backend);
    Backend::define_cursor(&mut fixture.backend, None, root, transparent)
        .expect("select transparent cursor before direct admission");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("accept cursorless primary");
    fixture.retire_output(0, 1);
    assert!(fixture.backend.scene.direct_scanout_primary_ready());
    super::round_b_producers::admit_held_direct_cursorless(&mut fixture, &direct, 31_000, 10);
    assert!(fixture.backend.pstate.scanout_m2.active());
    assert!(fixture.backend.pstate.scanout_m2.hold_direct);

    let visible = create_argb_cursor(&mut fixture.backend, 4);
    let before_attach = fixture.recorders.atomic.records().len();
    Backend::define_cursor(&mut fixture.backend, None, root, visible)
        .expect("show cursor on held direct scanout");
    fixture
        .recorders
        .atomic
        .script_result(Err(io::Error::from_raw_os_error(libc::ENOSPC)));
    Backend::maybe_composite(&mut fixture.backend).expect("reject the cursor-only attach");
    let rejected = fixture.recorders.atomic.records();
    assert_eq!(rejected.len(), before_attach + 1);
    assert_eq!(
        rejected[before_attach].kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly,
    );
    assert_eq!(
        rejected[before_attach].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: io::ErrorKind::StorageFull,
            raw_os_error: Some(libc::ENOSPC),
        },
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.pstate.scanout_m2.unflip_requested);

    let mut recovered = false;
    for frame in 20..40 {
        Backend::maybe_composite(&mut fixture.backend)
            .expect("compose the visible software cursor after attach rejection");
        if fixture.backend.scene.has_pending_page_flip(0)
            || fixture
                .backend
                .pstate
                .scanout_m2
                .unflip_awaiting_outputs
                .contains(&0)
        {
            fixture.retire_output(0, frame);
        }
        let receipt = fixture.backend.scene.for_tests_cursor_commit_receipt(0);
        if !fixture.backend.pstate.scanout_m2.active()
            && receipt.is_some_and(|receipt| {
                receipt.mode == CursorAcceptedMode::Software
                    && matches!(
                        receipt.primary_cursor,
                        CursorPrimaryContent::Software { .. }
                    )
            })
        {
            recovered = true;
            break;
        }
    }
    assert!(
        recovered,
        "ENOSPC recovery must accept the software cursor frame"
    );
    let software = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted software receipt");
    assert_eq!(software.mode, CursorAcceptedMode::Software);
    assert!(matches!(
        software.primary_cursor,
        CursorPrimaryContent::Software { .. }
    ));
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert!(!fixture.backend.pstate.scanout_m2.active());
    assert!(fixture.backend.pstate.scanout_m2.current.is_none());
    assert!(!fixture.backend.scene.has_pending_page_flips());

    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture.recorders.atomic.records()[before_attach..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "rejected attach, SW fallback, and lifecycle disable have the exact trace",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: drop a hide queued behind a SW→HW frame; the event must leave the HW plane detached.
fn atomic_cursor_only_hide_survives_pending_software_to_hardware_frame() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPlaneMode};

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor transition fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let oversized = create_oversized_cursor(&mut fixture);
    Backend::define_cursor(&mut fixture.backend, None, root, oversized)
        .expect("establish Software ownership through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("accept initial Software frame");
    let software = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted Software receipt");
    assert_eq!(software.mode, CursorAcceptedMode::Software);
    fixture.retire_output(0, 1);
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);

    let visible = create_argb_cursor(&mut fixture.backend, 4);
    Backend::define_cursor(&mut fixture.backend, None, root, visible)
        .expect("request Hardware ownership through Backend");
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("accept SW→HW primary frame");
    let hardware = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted Hardware receipt");
    assert_eq!(hardware.mode, CursorAcceptedMode::Hardware);
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));

    let before_queued_hide = fixture.recorders.atomic.records().len();
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("retain hide behind the scene event");
    assert_eq!(fixture.recorders.atomic.records().len(), before_queued_hide);
    fixture.retire_output(0, 2);

    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_queued_hide + 1,
        "the transition event immediately submits the queued hide",
    );
    assert_eq!(
        fixture.recorders.atomic.records().last().unwrap().kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly,
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        CursorPlaneMode::Hidden,
        "the queued hide publishes a Hidden receipt without another stimulus",
    );
    let hidden = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted Hidden receipt");
    assert_eq!(hidden.mode, CursorAcceptedMode::Hidden);
    fixture.retire_output(0, 3);

    fixture
        .backend
        .disable_output()
        .expect("disable and drain through Backend");
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "the SW frame, SW→HW frame, event-triggered hide, and disable are exact",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: remove the no-progress guard, restore the global scene_wants_scene cursor guard, or skip
// SW cursor repaint after content damage changes; each must fail a bounded-deadline or per-output commit assertion.
fn atomic_cursor_only_blocked_software_output_does_not_stall_hardware_peer() {
    use crate::kms::render::scene::CursorAcceptedMode;
    use std::{cell::Cell, rc::Rc, time::Duration};

    let Some(mut fixture) = AtomicCursorFixture::new_with_cursor_mixed_three_output_layout()
        .expect("build mixed HW/SW cursor fixture")
    else {
        return;
    };
    let scheduler_now = Rc::new(Cell::new(std::time::Instant::now()));
    let clock = Rc::clone(&scheduler_now);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || clock.get()));
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let cursor = create_argb_cursor(&mut fixture.backend, 64);
    Backend::define_cursor(&mut fixture.backend, None, root, cursor)
        .expect("define spanning cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 1000, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    settle_scene_outputs(&mut fixture, 1);

    let hardware_output = 1;
    let software_output = 2;
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(hardware_output)
            .expect("accepted HW peer receipt")
            .mode,
        CursorAcceptedMode::Hardware,
    );
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(software_output)
            .expect("accepted software peer receipt")
            .mode,
        CursorAcceptedMode::Hidden,
    );

    let before_motion = fixture.recorders.atomic.records().len();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 1590, 90);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("accept B's copied SW frame while scheduling HW cursor motion");
    assert!(fixture.backend.scene.has_pending_page_flip(software_output));
    assert!(
        fixture
            .backend
            .scene
            .output_cursor_transition_pending(software_output)
    );
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(software_output)
            .expect("B's accepted SW receipt")
            .mode,
        CursorAcceptedMode::Software,
        "B's copied frame is accepted while its flip event remains withheld",
    );
    let hardware_crtc = fixture.backend.platform.outputs[hardware_output]
        .output
        .crtc;
    let software_crtc = fixture.backend.platform.outputs[software_output]
        .output
        .crtc;
    let first_motion = fixture.recorders.atomic.records();
    let first_cursor_only: Vec<_> = first_motion[before_motion..]
        .iter()
        .filter(|record| record.kind == crate::drm::atomic::AtomicCommitKind::CursorOnly)
        .collect();
    assert_eq!(
        first_cursor_only.len(),
        1,
        "output A makes one cursor-only update while B's copied frame waits",
    );
    assert_eq!(
        first_cursor_only[0].events[0].crtc, hardware_crtc,
        "the accepted cursor-only event belongs to output A",
    );
    assert!(first_motion[before_motion..].iter().any(|record| {
        record
            .events
            .iter()
            .any(|event| event.crtc == software_crtc)
    }));
    let hardware_binding = fixture.backend.platform.devices[0].cursor.crtc_states[&hardware_crtc]
        .applied
        .expect("output A keeps its accepted cursor binding");
    assert_eq!(
        hardware_binding.x,
        1590 - 800 - i32::from(hardware_binding.hot_x)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(hardware_output)
    );

    fixture.retire_output(hardware_output, 100);
    assert!(fixture.backend.scene.has_pending_page_flip(software_output));

    let before_second_motion = fixture.recorders.atomic.records().len();
    Backend::on_host_input(
        &mut fixture.backend,
        &mut fixture.state,
        yserver_core::core_loop::HostInputEvent::PointerMotion {
            origin: yserver_core::core_loop::InputOrigin::NestedHost,
            x: 1595,
            y: 90,
            time: 1,
            relative: false,
            dx: 0,
            dy: 0,
            motion_delta: None,
        },
    );
    Backend::mark_dirty(&mut fixture.backend);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("run the host-input wake while B remains blocked");
    let after_second_motion = fixture.recorders.atomic.records();
    assert_eq!(
        after_second_motion[before_second_motion..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![crate::drm::atomic::AtomicCommitKind::CursorOnly],
        "the latest A motion is accepted without waiting for B: {:#?}",
        &after_second_motion[before_second_motion..],
    );
    let latest_binding = fixture.backend.platform.devices[0].cursor.crtc_states[&hardware_crtc]
        .applied
        .expect("latest A cursor binding");
    assert_eq!(
        latest_binding.x,
        1595 - 800 - i32::from(latest_binding.hot_x)
    );
    fixture.retire_output(hardware_output, 101);
    assert!(fixture.backend.scene.has_pending_page_flip(software_output));

    Backend::clear_area(
        &mut fixture.backend,
        None,
        root,
        0,
        None,
        1700,
        150,
        100,
        100,
        (0, 0),
    )
    .expect("damage only output B on the spanning root drawable");
    assert!(fixture.backend.store.has_pending_presentation_damage());
    Backend::on_host_input(
        &mut fixture.backend,
        &mut fixture.state,
        yserver_core::core_loop::HostInputEvent::PointerMotion {
            origin: yserver_core::core_loop::InputOrigin::NestedHost,
            x: 1596,
            y: 90,
            time: 2,
            relative: false,
            dx: 0,
            dy: 0,
            motion_delta: None,
        },
    );
    Backend::mark_dirty(&mut fixture.backend);
    let now = scheduler_now.get();
    assert!(Backend::next_wakeup(&fixture.backend).is_some_and(|deadline| deadline <= now));
    let before_blocked_host = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend)
        .expect("run the host-input wake while B content remains blocked");
    let blocked_host_records = fixture.recorders.atomic.records();
    assert!(
        blocked_host_records[before_blocked_host..]
            .iter()
            .any(|record| record
                .events
                .iter()
                .any(|event| event.crtc == hardware_crtc))
    );
    assert!(
        blocked_host_records[before_blocked_host..]
            .iter()
            .all(|record| !record
                .events
                .iter()
                .any(|event| event.crtc == software_crtc))
    );

    let mut immediate_deadlines = 0;
    let mut scheduler_passes = 1;
    for _ in 0..24 {
        scheduler_now.set(scheduler_now.get() + Duration::from_millis(1));
        let now = scheduler_now.get();
        let Some(deadline) = Backend::next_wakeup(&fixture.backend) else {
            continue;
        };
        if deadline <= now {
            immediate_deadlines += 1;
            scheduler_passes += 1;
            Backend::maybe_composite(&mut fixture.backend)
                .expect("run a core-scheduled pass while B's event is withheld");
        }
    }
    assert!(
        immediate_deadlines <= 2,
        "content damage plus host input is bounded to two immediate passes: {immediate_deadlines}",
    );
    assert!(
        scheduler_passes <= 3,
        "only the host-input pass and bounded progress retry ran: {scheduler_passes}",
    );
    let now = scheduler_now.get();
    assert!(!Backend::next_wakeup(&fixture.backend).is_some_and(|deadline| deadline <= now));
    assert!(fixture.backend.scene.has_pending_page_flip(software_output));
    if fixture.backend.scene.has_pending_page_flip(hardware_output)
        || fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(hardware_output)
    {
        fixture.retire_output(hardware_output, 102);
    }
    fixture.retire_output(software_output, 102);
    settle_scene_outputs(&mut fixture, 110);
    assert!(!fixture.backend.scene.has_pending_page_flips());
    fixture
        .backend
        .disable_output()
        .expect("disable and drain mixed outputs through Backend");
    fixture.assert_cursor_quiescent();
    let movement_kinds: Vec<_> = fixture.recorders.atomic.records()[before_motion..]
        .iter()
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        movement_kinds,
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "HW cursor-only work precedes B's deferred scene frame and exact output teardown",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
    let Some(mut software_fixture) =
        AtomicCursorFixture::new_with_cursor_mixed_three_output_layout()
            .expect("build all-software cursor fixture")
    else {
        return;
    };
    software_fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    for device in &mut software_fixture.backend.platform.devices {
        device.cursor.plane = None;
    }
    let layouts = software_fixture.backend.platform.scene_output_layouts();
    software_fixture.backend.scene = crate::kms::render::scene::SceneCompositor::new(
        &software_fixture.backend.platform,
        &layouts,
    )
    .expect("rebuild scene after construction-time software eligibility setup");
    let spanning_window = 0x3100;
    create_spanning_window_through_core(&mut software_fixture, spanning_window);
    let scheduler_now = Rc::new(Cell::new(std::time::Instant::now()));
    let clock = Rc::clone(&scheduler_now);
    software_fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || clock.get()));

    let root = software_fixture.backend.core.window_id;
    let cursor = create_argb_cursor(&mut software_fixture.backend, 64);
    Backend::define_cursor(&mut software_fixture.backend, None, root, cursor)
        .expect("define cursor for software output A");
    Backend::warp_pointer_root(
        &mut software_fixture.backend,
        &mut software_fixture.state,
        1000,
        150,
    );
    super::round_a::damage_cursor_root(&mut software_fixture);
    settle_scene_outputs(&mut software_fixture, 200);

    let software_output_a = 1;
    let blocked_output_b = 2;
    assert_eq!(
        software_fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(software_output_a)
            .expect("accepted software A receipt")
            .mode,
        CursorAcceptedMode::Software,
    );
    let spanning_host_xid = software_fixture
        .state
        .resources
        .window(yserver_protocol::x11::ResourceId(spanning_window))
        .expect("run_core spanning window")
        .host_xid
        .expect("spanning window host id")
        .as_raw();
    let spanning_drawable = software_fixture
        .backend
        .store
        .lookup(spanning_host_xid)
        .expect("mapped spanning window owns a drawable");
    Backend::clear_area(
        &mut software_fixture.backend,
        None,
        spanning_host_xid,
        0,
        None,
        870,
        100,
        100,
        100,
        (0, 0),
    )
    .expect("arm B-only content damage on the drawable spanning A and B");
    assert!(
        software_fixture
            .backend
            .store
            .armed_damaged_ids()
            .contains(&spanning_drawable)
    );
    Backend::maybe_composite(&mut software_fixture.backend)
        .expect("hold B's content frame while A remains available");
    assert!(
        software_fixture
            .backend
            .scene
            .has_pending_page_flip(blocked_output_b)
    );
    for output_idx in [0, software_output_a] {
        if software_fixture
            .backend
            .scene
            .has_pending_page_flip(output_idx)
        {
            software_fixture.retire_output(output_idx, 210 + output_idx as u32);
        }
    }
    let checked_content_generation = software_fixture.backend.store.scene_damage_generation();
    Backend::clear_area(
        &mut software_fixture.backend,
        None,
        spanning_host_xid,
        0,
        None,
        870,
        100,
        100,
        100,
        (0, 0),
    )
    .expect("change B-only content after A checked its prior scene epoch");
    assert!(software_fixture.backend.store.scene_damage_generation() > checked_content_generation);
    assert!(
        software_fixture
            .backend
            .store
            .armed_damaged_ids()
            .contains(&spanning_drawable)
    );

    let output_a_crtc = software_fixture.backend.platform.outputs[software_output_a]
        .output
        .crtc;
    let output_b_crtc = software_fixture.backend.platform.outputs[blocked_output_b]
        .output
        .crtc;
    for (step, x) in [1020, 1040, 1060].into_iter().enumerate() {
        scheduler_now.set(scheduler_now.get() + Duration::from_millis(16));
        let now = scheduler_now.get();
        Backend::on_host_input(
            &mut software_fixture.backend,
            &mut software_fixture.state,
            yserver_core::core_loop::HostInputEvent::PointerMotion {
                origin: yserver_core::core_loop::InputOrigin::NestedHost,
                x,
                y: 150,
                time: step as u32 + 2,
                relative: false,
                dx: 0,
                dy: 0,
                motion_delta: None,
            },
        );
        Backend::mark_dirty(&mut software_fixture.backend);
        assert!(
            Backend::next_wakeup(&software_fixture.backend).is_some_and(|deadline| deadline <= now)
        );
        let before = software_fixture.recorders.atomic.records().len();
        Backend::maybe_composite(&mut software_fixture.backend)
            .expect("run the clocked SW cursor repaint pass");
        let records = software_fixture.recorders.atomic.records();
        let repaint_records: Vec<_> = records[before..]
            .iter()
            .filter(|record| {
                record
                    .events
                    .iter()
                    .any(|event| event.crtc == output_a_crtc)
            })
            .collect();
        assert_eq!(
            repaint_records.len(),
            1,
            "SW output A repaints once at every paced cursor input while B waits: {:#?}",
            &records[before..],
        );
        assert_eq!(
            repaint_records[0].kind,
            crate::drm::atomic::AtomicCommitKind::Scene,
            "SW cursor movement belongs in A's primary frame",
        );
        assert!(records[before..].iter().all(|record| {
            !record
                .events
                .iter()
                .any(|event| event.crtc == output_b_crtc)
        }));
        assert!(
            software_fixture
                .backend
                .scene
                .has_pending_page_flip(blocked_output_b)
        );
        assert!(
            software_fixture
                .backend
                .scene
                .has_pending_page_flip(software_output_a)
        );
        assert!(matches!(
            software_fixture
                .backend
                .scene
                .for_tests_cursor_commit_receipt(software_output_a)
                .expect("accepted SW repaint receipt")
                .primary_cursor,
            crate::kms::render::scene::CursorPrimaryContent::Software { .. }
        ));
        software_fixture.retire_output(software_output_a, 220 + step as u32);
    }

    software_fixture.retire_output(blocked_output_b, 230);
    settle_scene_outputs(&mut software_fixture, 240);
    assert!(!software_fixture.backend.scene.has_pending_page_flips());
    software_fixture
        .backend
        .disable_output()
        .expect("disable and drain the software cursor fixture through Backend");
    software_fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&software_fixture.recorders.cursor_buffer_ledger);
    drop(software_fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: submit cursor-only work on A despite A's own primary damage; its primary-frame assertion must fail.
fn atomic_cursor_only_damaged_hardware_peer_carries_motion_in_primary_frame() {
    use crate::kms::render::scene::CursorAcceptedMode;
    use std::{cell::Cell, rc::Rc, time::Duration};
    use yserver_protocol::x11::ResourceId;

    let Some(mut fixture) = AtomicCursorFixture::new_with_cursor_mixed_three_output_layout()
        .expect("build mixed HW/SW cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let scheduler_now = Rc::new(Cell::new(std::time::Instant::now()));
    let clock = Rc::clone(&scheduler_now);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || clock.get()));

    let window_xid = 0x3400;
    create_spanning_window_through_core(&mut fixture, window_xid);
    let root = fixture.backend.core.window_id;
    let cursor = create_argb_cursor(&mut fixture.backend, 64);
    Backend::define_cursor(&mut fixture.backend, None, root, cursor)
        .expect("define spanning cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 1000, 150);
    super::round_a::damage_cursor_root(&mut fixture);
    settle_scene_outputs(&mut fixture, 400);

    let hardware_output = 1;
    let blocked_output = 2;
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(hardware_output)
            .expect("accepted hardware cursor receipt")
            .mode,
        CursorAcceptedMode::Hardware,
    );
    let window = fixture
        .state
        .resources
        .window(ResourceId(window_xid))
        .expect("run_core spanning window")
        .host_xid
        .expect("spanning window host id")
        .as_raw();
    let cover = fixture
        .state
        .resources
        .window(ResourceId(window_xid + 1))
        .expect("run_core A cover window")
        .host_xid
        .expect("cover window host id")
        .as_raw();
    let hardware_crtc = fixture.backend.platform.outputs[hardware_output]
        .output
        .crtc;
    let blocked_crtc = fixture.backend.platform.outputs[blocked_output].output.crtc;

    // Hold B on its own visible content damage while A remains idle.
    Backend::clear_area(
        &mut fixture.backend,
        None,
        window,
        0,
        None,
        870,
        50,
        40,
        40,
        (0, 0),
    )
    .expect("damage the exposed part of B's spanning window");
    let before_blocked_frame = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend).expect("submit B's damaged primary frame");
    let blocked_frame_records = fixture.recorders.atomic.records();
    assert!(fixture.backend.scene.has_pending_page_flip(blocked_output));
    assert!(
        blocked_frame_records[before_blocked_frame..]
            .iter()
            .any(|record| { record.events.iter().any(|event| event.crtc == blocked_crtc) })
    );
    for output_idx in 0..fixture.backend.platform.outputs.len() {
        if output_idx != blocked_output
            && (fixture.backend.scene.has_pending_page_flip(output_idx)
                || fixture
                    .backend
                    .platform
                    .cursor_commit_in_flight_for_output(output_idx))
        {
            fixture.retire_output(output_idx, 410 + output_idx as u32);
        }
    }
    assert!(!fixture.backend.scene.has_pending_page_flip(hardware_output));
    assert!(fixture.backend.scene.has_pending_page_flip(blocked_output));

    let mut accepted_kinds = Vec::new();
    for (step, x) in [1020, 1040, 1060].into_iter().enumerate() {
        // Each client paint targets A's visible cover, and the input arrives
        // before the compose pass that must carry both pieces of work.
        Backend::clear_area(
            &mut fixture.backend,
            None,
            cover,
            0,
            None,
            10 + step as i16,
            10,
            40,
            40,
            (0, 0),
        )
        .expect("damage A's visible cover window");
        assert!(fixture.backend.store.has_pending_presentation_damage());
        scheduler_now.set(scheduler_now.get() + Duration::from_millis(16));
        let now = scheduler_now.get();
        Backend::on_host_input(
            &mut fixture.backend,
            &mut fixture.state,
            yserver_core::core_loop::HostInputEvent::PointerMotion {
                origin: yserver_core::core_loop::InputOrigin::NestedHost,
                x,
                y: 150,
                time: step as u32 + 1,
                relative: false,
                dx: 0,
                dy: 0,
                motion_delta: None,
            },
        );
        Backend::mark_dirty(&mut fixture.backend);
        assert!(Backend::next_wakeup(&fixture.backend).is_some_and(|deadline| deadline <= now));

        let before_a_frame = fixture.recorders.atomic.records().len();
        Backend::maybe_composite(&mut fixture.backend)
            .expect("submit A's damaged primary frame while B remains held");
        let records = fixture.recorders.atomic.records();
        assert_eq!(
            records[before_a_frame..].len(),
            1,
            "A's damage and motion produce exactly one commit: {:#?}",
            &records[before_a_frame..],
        );
        let record = &records[before_a_frame];
        assert_eq!(
            record.kind,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            "A's primary frame owns the current HW cursor position",
        );
        assert_eq!(record.events.len(), 1);
        assert_eq!(record.events[0].crtc, hardware_crtc);
        assert!(fixture.backend.scene.has_pending_page_flip(hardware_output));
        assert!(fixture.backend.scene.has_pending_page_flip(blocked_output));
        accepted_kinds.push(record.kind);
        let binding = fixture.backend.platform.devices[0].cursor.crtc_states[&hardware_crtc]
            .applied
            .expect("A's accepted primary frame retains its hardware binding");
        assert_eq!(binding.x, x - 800 - i32::from(binding.hot_x));
        fixture.retire_output(hardware_output, 420 + step as u32);
    }

    assert_eq!(
        accepted_kinds,
        vec![crate::drm::atomic::AtomicCommitKind::SceneWithCursor; 3],
        "every paced damage/motion pair is carried by A's primary frame",
    );
    assert!(fixture.backend.scene.has_pending_page_flip(blocked_output));
    fixture.retire_output(blocked_output, 430);
    settle_scene_outputs(&mut fixture, 440);
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert!(!fixture.backend.store.has_pending_presentation_damage());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    fixture
        .backend
        .disable_output()
        .expect("disable and drain mixed outputs through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore the content-generation early return that suppresses a ready SW output's cursor repaint.
fn atomic_cursor_only_blocked_software_cursor_repaints_after_unrelated_content_damage() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent};
    use std::{cell::Cell, rc::Rc, time::Duration};

    let Some(mut fixture) = AtomicCursorFixture::new_with_cursor_mixed_three_output_layout()
        .expect("build software-cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    for device in &mut fixture.backend.platform.devices {
        device.cursor.plane = None;
    }
    let layouts = fixture.backend.platform.scene_output_layouts();
    fixture.backend.scene =
        crate::kms::render::scene::SceneCompositor::new(&fixture.backend.platform, &layouts)
            .expect("rebuild scene for software cursor outputs");
    let spanning_window = 0x3200;
    create_spanning_window_through_core(&mut fixture, spanning_window);
    let scheduler_now = Rc::new(Cell::new(std::time::Instant::now()));
    let clock = Rc::clone(&scheduler_now);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || clock.get()));

    let root = fixture.backend.core.window_id;
    let cursor = create_argb_cursor(&mut fixture.backend, 64);
    Backend::define_cursor(&mut fixture.backend, None, root, cursor)
        .expect("define software cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 1000, 150);
    settle_scene_outputs(&mut fixture, 300);

    let software_output_a = 1;
    let blocked_output_b = 2;
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(software_output_a)
            .expect("accepted software A receipt")
            .mode,
        CursorAcceptedMode::Software,
    );
    let host_xid = fixture
        .state
        .resources
        .window(yserver_protocol::x11::ResourceId(spanning_window))
        .expect("run_core spanning window")
        .host_xid
        .expect("spanning window host id")
        .as_raw();
    let drawable = fixture
        .backend
        .store
        .lookup(host_xid)
        .expect("mapped spanning window owns a drawable");
    let output_a_crtc = fixture.backend.platform.outputs[software_output_a]
        .output
        .crtc;
    let output_b_crtc = fixture.backend.platform.outputs[blocked_output_b]
        .output
        .crtc;

    Backend::clear_area(
        &mut fixture.backend,
        None,
        host_xid,
        0,
        None,
        870,
        100,
        100,
        100,
        (0, 0),
    )
    .expect("arm B-only damage on the spanning drawable");
    assert!(
        fixture
            .backend
            .store
            .armed_damaged_ids()
            .contains(&drawable)
    );
    Backend::maybe_composite(&mut fixture.backend).expect("submit B's damaged frame");
    assert!(
        fixture
            .backend
            .scene
            .has_pending_page_flip(blocked_output_b)
    );
    for output_idx in [0, software_output_a] {
        if fixture.backend.scene.has_pending_page_flip(output_idx) {
            fixture.retire_output(output_idx, 310 + output_idx as u32);
        }
    }
    Backend::maybe_composite(&mut fixture.backend)
        .expect("let ready A check the current scene while B remains held");
    if fixture
        .backend
        .scene
        .has_pending_page_flip(software_output_a)
    {
        fixture.retire_output(software_output_a, 320);
    }
    assert!(
        fixture
            .backend
            .scene
            .has_pending_page_flip(blocked_output_b)
    );

    let previous_content_generation = fixture.backend.store.scene_damage_generation();
    Backend::clear_area(
        &mut fixture.backend,
        None,
        host_xid,
        0,
        None,
        870,
        100,
        100,
        100,
        (0, 0),
    )
    .expect("advance B-only content generation after A's check");
    assert!(fixture.backend.store.scene_damage_generation() > previous_content_generation);
    assert!(
        fixture
            .backend
            .store
            .armed_damaged_ids()
            .contains(&drawable)
    );

    let mut paced_commit_sequence = Vec::new();
    for (step, x) in [1020, 1040, 1060].into_iter().enumerate() {
        scheduler_now.set(scheduler_now.get() + Duration::from_millis(16));
        let now = scheduler_now.get();
        Backend::on_host_input(
            &mut fixture.backend,
            &mut fixture.state,
            yserver_core::core_loop::HostInputEvent::PointerMotion {
                origin: yserver_core::core_loop::InputOrigin::NestedHost,
                x,
                y: 150,
                time: step as u32 + 1,
                relative: false,
                dx: 0,
                dy: 0,
                motion_delta: None,
            },
        );
        Backend::mark_dirty(&mut fixture.backend);
        assert!(
            Backend::next_wakeup(&fixture.backend).is_some_and(|deadline| deadline <= now),
            "each paced cursor input creates an immediate external-input wake",
        );
        let before = fixture.recorders.atomic.records().len();
        Backend::maybe_composite(&mut fixture.backend)
            .expect("repaint A's software cursor while B's flip is held");
        let records = fixture.recorders.atomic.records();
        assert_eq!(
            records[before..].len(),
            1,
            "each paced software-cursor update records exactly one atomic commit",
        );
        let a_records: Vec<_> = records[before..]
            .iter()
            .filter(|record| {
                record
                    .events
                    .iter()
                    .any(|event| event.crtc == output_a_crtc)
            })
            .collect();
        assert_eq!(
            a_records.len(),
            1,
            "one A frame per paced motion: {:#?}",
            &records[before..]
        );
        assert_eq!(
            a_records[0].kind,
            crate::drm::atomic::AtomicCommitKind::Scene,
            "software cursor motion is part of A's primary frame",
        );
        assert_eq!(
            a_records[0].events.len(),
            1,
            "the paced scene commit carries only output A's event",
        );
        assert_eq!(a_records[0].events[0].crtc, output_a_crtc);
        paced_commit_sequence.push(a_records[0].kind);
        assert!(records[before..].iter().all(|record| {
            record
                .events
                .iter()
                .all(|event| event.crtc != output_b_crtc)
        }));
        assert!(
            fixture
                .backend
                .scene
                .has_pending_page_flip(blocked_output_b)
        );
        assert!(
            fixture
                .backend
                .scene
                .has_pending_page_flip(software_output_a)
        );
        assert!(matches!(
            fixture
                .backend
                .scene
                .for_tests_cursor_commit_receipt(software_output_a)
                .expect("accepted software repaint receipt")
                .primary_cursor,
            CursorPrimaryContent::Software { .. }
        ));
        fixture.retire_output(software_output_a, 330 + step as u32);
    }
    assert_eq!(
        paced_commit_sequence,
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "the exact paced sequence is one A scene frame for each motion",
    );

    fixture.retire_output(blocked_output_b, 340);
    settle_scene_outputs(&mut fixture, 350);
    assert!(!fixture.backend.scene.has_pending_page_flips());
    fixture
        .backend
        .disable_output()
        .expect("disable and drain software cursor outputs through Backend");
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
