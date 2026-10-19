//! Cursor arbitration after bounded core request dispatch.

use super::{
    round_a::{damage_cursor_root, setup_cursor_only_direct_present},
    round_b_producers::begin_hw_cursor,
    round_c_attribution::teardown,
    support::AtomicCursorFixture,
};
use crate::drm::atomic::{
    AtomicCommitEvent, AtomicCommitExecutor, AtomicCommitKind, AtomicCommitRequest,
    tests_support::AtomicCommitRecord,
};
use ::drm::control::AtomicCommitFlags;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    sync::{Arc, Mutex, atomic::AtomicU16},
    time::Duration,
};
use yserver_core::{
    backend::{Backend, PixmapHandle, WindowHandle},
    core_loop::{
        CoreSender, HostInputEvent, InputOrigin, Message, ResetPolicy, auth::AuthState, channel,
        poll_tokens::ClientIdAllocator, run_core,
    },
    host_x11::HostSubwindowVisual,
    resources::{COMPOSITE_OVERLAY_WINDOW, ROOT_VISUAL, ROOT_WINDOW},
    server::{ClientState, PresentEventSelection},
    transport::Transport,
};
use yserver_protocol::x11::{
    ClientByteOrder, ClientId, CreatePixmapRequest, RequestHeader, ResourceId, SequenceNumber,
    present as x11present,
};

const CLIENT: ClientId = ClientId(1);
const WINDOW: ResourceId = ResourceId(0x0100_3100);
const PIXMAP: ResourceId = ResourceId(0x0100_3101);
const PRESENT_OPCODE: u8 = 145;
const VBLANKS: u32 = 30;

struct ClockedCommitExecutor {
    inner: std::rc::Rc<crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor>,
    clock: std::rc::Rc<dyn Fn() -> std::time::Instant>,
    time_origin: std::time::Instant,
    current_decision_id: Arc<std::sync::atomic::AtomicU64>,
    cursor_submissions: Arc<Mutex<Vec<CursorSubmissionAtDecision>>>,
    driver_events: std::sync::mpsc::Sender<FloodDriverEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecisionPointRecord {
    id: u64,
    point: crate::kms::render::backend::kms::state::CursorDecisionPoint,
    output_idx: usize,
    elapsed_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorSubmissionAtDecision {
    decision_id: u64,
    elapsed_us: u64,
}

struct DecisionPointRendezvous {
    id: u64,
    point: crate::kms::render::backend::kms::state::CursorDecisionPoint,
    output_idx: usize,
    acknowledged: std::sync::mpsc::SyncSender<()>,
}

enum FloodDriverEvent {
    DecisionPoint(DecisionPointRendezvous),
    CursorSubmitted(u64),
    Stop,
}

impl AtomicCommitExecutor for ClockedCommitExecutor {
    fn atomic_commit(
        &self,
        device: &crate::drm::Device,
        flags: AtomicCommitFlags,
        kind: AtomicCommitKind,
        request: AtomicCommitRequest,
        events: &[AtomicCommitEvent],
    ) -> std::io::Result<()> {
        let result = self
            .inner
            .atomic_commit(device, flags, kind, request, events);
        if kind == AtomicCommitKind::CursorOnly && result.is_ok() {
            let decision_id = self
                .current_decision_id
                .load(std::sync::atomic::Ordering::SeqCst);
            if decision_id != 0 {
                let now = (self.clock)();
                let elapsed_us = u64::try_from(now.duration_since(self.time_origin).as_micros())
                    .unwrap_or(u64::MAX);
                self.cursor_submissions
                    .lock()
                    .expect("cursor submission recorder is not poisoned")
                    .push(CursorSubmissionAtDecision {
                        decision_id,
                        elapsed_us,
                    });
                let _ = self
                    .driver_events
                    .send(FloodDriverEvent::CursorSubmitted(decision_id));
            }
        }
        result
    }
}

struct CoreClient {
    peer: UnixStream,
}

struct PollableDevice {
    peer: UnixStream,
}

fn live_records(fixture: &AtomicCursorFixture) -> Vec<AtomicCommitRecord> {
    fixture
        .recorders
        .atomic
        .records()
        .into_iter()
        .filter(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .collect()
}

fn install_client(state: &mut yserver_core::server::ServerState, id: ClientId) -> CoreClient {
    let (transport, peer) = Transport::pair().expect("create run_core test client");
    peer.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set run_core peer timeout");
    state.clients.insert(
        id.0,
        ClientState {
            writer: Arc::new(Mutex::new(transport)),
            is_local: true,
            fd_passing: true,
            internal: false,
            byte_order: ClientByteOrder::LittleEndian,
            last_sequence: Arc::new(AtomicU16::new(0)),
            resource_id_base: 0,
            resource_id_mask: u32::MAX,
            big_requests_enabled: false,
            event_masks: HashMap::new(),
            save_set: HashSet::new(),
            xi2_masks: HashMap::new(),
            xi1_event_classes: HashSet::new(),
            xi1_window_event_classes: HashMap::new(),
            outbound: VecDeque::new(),
            watching_writable: false,
            write_failed: false,
            focused_window: ROOT_WINDOW,
            reader_control: None,
        },
    );
    CoreClient { peer }
}

fn install_present_window(fixture: &mut AtomicCursorFixture) {
    fixture
        .state
        .resources
        .window_mut(ROOT_WINDOW)
        .expect("core root resource")
        .host_xid = WindowHandle::from_raw(fixture.backend.window_id());
    assert!(
        Backend::get_overlay_window(&mut fixture.backend, None)
            .expect("materialize the backend COW")
    );
    let cow_host = Backend::cow_host_xid(&fixture.backend).expect("COW host window");
    let cow_handle = WindowHandle::from_raw(cow_host).expect("COW host handle");
    fixture.state.resources.materialize_cow_resource(cow_handle);
    let width = fixture.backend.platform.fb_w;
    let height = fixture.backend.platform.fb_h;
    let stage = Backend::create_subwindow(
        &mut fixture.backend,
        None,
        cow_handle,
        0,
        0,
        width,
        height,
        0,
        HostSubwindowVisual::DepthOnly { depth: 24 },
        None,
        None,
    )
    .expect("create COW Present target through Backend");
    fixture.state.resources.create_window(
        CLIENT,
        yserver_protocol::x11::CreateWindowRequest {
            depth: 24,
            window: WINDOW,
            parent: COMPOSITE_OVERLAY_WINDOW,
            x: 0,
            y: 0,
            width,
            height,
            class: 1,
            visual: ROOT_VISUAL,
            ..Default::default()
        },
    );
    fixture
        .state
        .resources
        .window_mut(WINDOW)
        .expect("COW Present resource")
        .host_xid = Some(stage);
    let _ = fixture.state.resources.map_window(WINDOW);
    Backend::realize_window_storage(
        &mut fixture.backend,
        None,
        stage.as_raw(),
        yserver_core::backend::RealizeBackground::Paint,
    )
    .expect("realize COW Present target storage");
    Backend::map_subwindow(&mut fixture.backend, None, stage.as_raw())
        .expect("map COW Present target");
    Backend::register_subwindow(&mut fixture.backend, None, WINDOW, stage.as_raw())
        .expect("register COW Present target");
    Backend::sync_top_level_order(&mut fixture.backend, &fixture.state);
}

fn install_present_pixmap(fixture: &mut AtomicCursorFixture) -> u32 {
    install_present_window(fixture);
    let width = fixture.backend.platform.fb_w;
    let height = fixture.backend.platform.fb_h;
    let host_pixmap = Backend::create_pixmap(&mut fixture.backend, None, 24, width, height)
        .expect("create Present source through Backend");
    Backend::fill_rectangle(
        &mut fixture.backend,
        None,
        host_pixmap.as_raw(),
        0x0030_4080,
        0,
        0,
        width,
        height,
    )
    .expect("paint Present source through Backend");
    fixture.state.resources.create_pixmap(
        CLIENT,
        CreatePixmapRequest {
            depth: 24,
            pixmap: PIXMAP,
            drawable: ROOT_WINDOW,
            width,
            height,
        },
    );
    assert!(
        fixture
            .state
            .resources
            .set_pixmap_host_xid(PIXMAP, host_pixmap)
    );
    fixture.state.present_event_selections.insert(
        0x0100_7efe,
        PresentEventSelection {
            owner: CLIENT,
            window: WINDOW,
            event_mask: x11present::EVENT_MASK_COMPLETE_NOTIFY,
        },
    );
    PIXMAP.0
}

fn make_devices_pollable(fixture: &mut AtomicCursorFixture) -> Vec<PollableDevice> {
    let mut peers = Vec::new();
    for (index, device) in fixture.backend.platform.devices.iter_mut().enumerate() {
        let (device_fd, peer) = UnixStream::pair().expect("create pollable synthetic DRM fd");
        let replacement = std::rc::Rc::new(crate::drm::Device::from_inherited_kms_fd(
            OwnedFd::from(device_fd),
            format!("<C12 synthetic DRM device {index}>"),
        ));
        fixture
            .recorders
            .page_flip_events
            .register_device(&replacement);
        device.device = replacement;
        peers.push(PollableDevice { peer });
    }
    peers
}

fn queue_output_flip(fixture: &AtomicCursorFixture, output_idx: usize, frame: u32) {
    let output = &fixture.backend.platform.outputs[output_idx];
    let device = &fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("output recording device")
        .device;
    fixture
        .recorders
        .page_flip_events
        .queue_flip_for_device(device, output.output.crtc, frame);
}

fn run_core_flip_iteration(fixture: &mut AtomicCursorFixture, drm_peer: &UnixStream) {
    let (poll, sender, receiver) = channel().expect("create core flip poll channel");
    let shutdown = sender.clone_handle();
    let handshake_sender = sender.bind();
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    handshake_sender
        .send(Message::SetupAllocate {
            id: ClientId(2),
            response_tx,
        })
        .expect("queue flip iteration handshake");
    let mut drm_peer = drm_peer
        .try_clone()
        .expect("clone synthetic DRM peer for core flip");
    drm_peer
        .write_all(&[1])
        .expect("signal synthetic DRM readiness");
    let driver = std::thread::spawn(move || {
        response_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("core processes flip iteration handshake");
        shutdown
            .send(Message::Shutdown)
            .expect("stop core after flip iteration");
    });
    run_core(
        poll,
        receiver,
        sender.clone_handle(),
        &mut fixture.state,
        &mut fixture.backend,
        [],
        &ClientIdAllocator::new(),
        AuthState::new(None),
        ResetPolicy::NoReset,
        None,
    )
    .expect("run_core handles the flip iteration");
    driver.join().expect("join flip iteration driver");
}

fn run_core_flood_motion_iteration(
    fixture: &mut AtomicCursorFixture,
    drm_peer: &UnixStream,
    first_sequence: u16,
    noops: usize,
    motion: (i32, i32),
) {
    let (poll, sender, receiver) = channel().expect("create bounded flood core iteration");
    let bound = sender.bind();
    let shutdown = sender.clone_handle();
    let mut sequence = first_sequence;
    for _ in 0..noops {
        request(&bound, CLIENT, sequence, 127, 0, 1, Vec::new());
        sequence = sequence.wrapping_add(1);
    }
    bound
        .send(Message::HostInput(HostInputEvent::PointerMotion {
            origin: InputOrigin::XTest(4),
            x: motion.0,
            y: motion.1,
            time: u32::from(sequence),
            relative: false,
            dx: 0,
            dy: 0,
            motion_delta: None,
        }))
        .expect("queue pointer motion behind the request flood");
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    bound
        .send(Message::SetupAllocate {
            id: ClientId(2),
            response_tx,
        })
        .expect("queue bounded core iteration handshake");
    let mut drm_peer = drm_peer.try_clone().expect("clone synthetic DRM peer");
    drm_peer
        .write_all(&[1])
        .expect("signal synthetic DRM readiness");
    let driver = std::thread::spawn(move || {
        response_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("core reaches bounded iteration handshake");
        shutdown
            .send(Message::Shutdown)
            .expect("stop core after bounded flood dispatch");
    });
    run_core(
        poll,
        receiver,
        sender.clone_handle(),
        &mut fixture.state,
        &mut fixture.backend,
        [],
        &ClientIdAllocator::new(),
        AuthState::new(None),
        ResetPolicy::NoReset,
        None,
    )
    .expect("run_core dispatches one bounded flood iteration");
    driver.join().expect("join bounded flood iteration driver");
}

fn request(
    sender: &yserver_core::core_loop::BoundSender,
    client: ClientId,
    sequence: u16,
    opcode: u8,
    minor: u8,
    length_units: u32,
    body: Vec<u8>,
) {
    sender
        .send(Message::Request {
            id: client,
            sequence: SequenceNumber(sequence),
            accepted_at: None,
            header: RequestHeader {
                opcode,
                data: minor,
                length_units,
            },
            body,
            attached_fd: None,
        })
        .expect("queue request for run_core");
}

fn present_body(window: u32, pixmap: u32, serial: u32) -> Vec<u8> {
    let mut body = vec![0_u8; 68];
    body[0..4].copy_from_slice(&window.to_le_bytes());
    body[4..8].copy_from_slice(&pixmap.to_le_bytes());
    body[8..12].copy_from_slice(&serial.to_le_bytes());
    body
}

fn run_core_pass(
    fixture: &mut AtomicCursorFixture,
    client: CoreClient,
    drm_peer: &UnixStream,
    sequence: u16,
    window: u32,
    pixmap: Option<u32>,
    wait_for_complete: bool,
    noops_before_present: usize,
    motion: Option<(i32, i32)>,
) -> (CoreClient, u16, Vec<u32>) {
    use std::thread;

    let (poll, sender, receiver) = channel().expect("create core poll channel");
    let bound = sender.bind();
    let shutdown = sender.clone_handle();
    let mut drm_peer = drm_peer.try_clone().expect("clone synthetic DRM peer");
    let mut sequence = sequence;
    for _ in 0..noops_before_present {
        request(&bound, CLIENT, sequence, 127, 0, 1, Vec::new());
        sequence = sequence.wrapping_add(1);
    }
    if let Some((x, y)) = motion {
        bound
            .send(Message::HostInput(HostInputEvent::PointerMotion {
                origin: InputOrigin::XTest(4),
                x,
                y,
                time: u32::from(sequence),
                relative: false,
                dx: 0,
                dy: 0,
                motion_delta: None,
            }))
            .expect("queue pointer motion for run_core");
    }
    let expected_serial = pixmap
        .filter(|_| wait_for_complete)
        .map(|_| u32::from(sequence));
    if let Some(pixmap) = pixmap {
        request(
            &bound,
            CLIENT,
            sequence,
            PRESENT_OPCODE,
            x11present::PIXMAP,
            18,
            present_body(window, pixmap, u32::from(sequence)),
        );
        sequence = sequence.wrapping_add(1);
    }
    request(
        &bound,
        CLIENT,
        sequence,
        PRESENT_OPCODE,
        x11present::QUERY_CAPABILITIES,
        2,
        0_u32.to_le_bytes().to_vec(),
    );
    drm_peer
        .write_all(&[1])
        .expect("signal synthetic DRM readiness");
    let driver = thread::spawn(move || {
        struct ShutdownOnDrop(CoreSender);
        impl Drop for ShutdownOnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(Message::Shutdown);
            }
        }
        let _shutdown_on_drop = ShutdownOnDrop(shutdown);
        let mut peer = client.peer;
        let mut completions = Vec::new();
        let mut got_reply = false;
        while !got_reply || expected_serial.is_some_and(|serial| !completions.contains(&serial)) {
            let mut first = [0_u8; 1];
            peer.read_exact(&mut first).expect("read core response");
            match first[0] {
                35 => {
                    let mut event = [0_u8; 40];
                    event[0] = first[0];
                    peer.read_exact(&mut event[1..])
                        .expect("read CompleteNotify");
                    assert_eq!(
                        u16::from_le_bytes([event[8], event[9]]),
                        u16::from(x11present::EVENT_COMPLETE_NOTIFY)
                    );
                    completions.push(u32::from_le_bytes(
                        event[20..24].try_into().expect("Present serial field"),
                    ));
                }
                1 => {
                    let mut reply = [0_u8; 32];
                    reply[0] = first[0];
                    peer.read_exact(&mut reply[1..])
                        .expect("read barrier reply");
                    got_reply = true;
                }
                0 => {
                    let mut error = [0_u8; 32];
                    error[0] = first[0];
                    peer.read_exact(&mut error[1..]).expect("read X11 error");
                    panic!("run_core returned X11 error packet {error:?}");
                }
                response => panic!("unexpected run_core response {response}"),
            }
        }
        (CoreClient { peer }, sequence, completions)
    });

    run_core(
        poll,
        receiver,
        sender.clone_handle(),
        &mut fixture.state,
        &mut fixture.backend,
        [],
        &ClientIdAllocator::new(),
        AuthState::new(None),
        ResetPolicy::NoReset,
        None,
    )
    .expect("run_core dispatches the queued requests and flip");
    let (client, sequence, completions) = driver.join().expect("join core client driver");
    (client, sequence, completions)
}

fn assert_only_accepted_scene_with_cursor(records: &[AtomicCommitRecord]) -> usize {
    let scenes = records
        .iter()
        .filter(|record| record.kind == AtomicCommitKind::SceneWithCursor)
        .collect::<Vec<_>>();
    assert!(
        records
            .iter()
            .filter(|record| record.kind == AtomicCommitKind::CursorOnly)
            .all(|record| {
                record.result != crate::drm::atomic::tests_support::ScriptedResult::Ok
            }),
        "no accepted cursor-only commit may use a queued frame's vblank"
    );
    scenes
        .iter()
        .filter(|record| record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok)
        .count()
}

fn carries_cursor_position(fixture: &AtomicCursorFixture, record: &AtomicCommitRecord) -> bool {
    let output = &fixture.backend.platform.outputs[0];
    let Some(properties) = fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
    else {
        return false;
    };
    record
        .properties
        .iter()
        .any(|update| update.property == properties.crtc_x)
}

fn carries_cursor_position_for_output(
    fixture: &AtomicCursorFixture,
    record: &AtomicCommitRecord,
    output_idx: usize,
) -> bool {
    let output = &fixture.backend.platform.outputs[output_idx];
    let Some(properties) = fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
    else {
        return false;
    };
    record
        .properties
        .iter()
        .any(|update| update.property == properties.crtc_x)
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore the flip-handler drain or decide before queued client requests; FIFO frames must carry the latest cursor.
fn atomic_cursor_only_fifo_present_queued_before_flip_uses_scene_cursor() {
    let injected_now = std::time::Instant::now();
    let clock_cell = std::rc::Rc::new(std::cell::Cell::new(injected_now));
    let clock_reader = std::rc::Rc::clone(&clock_cell);
    let clock: std::rc::Rc<dyn Fn() -> std::time::Instant> =
        std::rc::Rc::new(move || clock_reader.get());
    let Some(mut fixture) =
        AtomicCursorFixture::new_with_clock(clock).expect("build cursor fixture")
    else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let pixmap = install_present_pixmap(&mut fixture);
    let drm_peers = make_devices_pollable(&mut fixture);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("prime scene flip");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    let mut client = install_client(&mut fixture.state, CLIENT);
    let mut sequence = 1;
    let mut scene_cursor_count = 0;
    let mut completed = Vec::new();
    for vblank in 0..VBLANKS {
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        let x = 110 + i32::try_from(vblank).expect("vblank fits") % 30;
        let y = 100 + i32::try_from(vblank).expect("vblank fits") % 20;
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, x, y);
        queue_output_flip(&fixture, 0, 100 + vblank);
        let (next_client, next_sequence, serials) = run_core_pass(
            &mut fixture,
            client,
            &drm_peers[0].peer,
            sequence,
            WINDOW.0,
            Some(pixmap),
            true,
            0,
            None,
        );
        client = next_client;
        sequence = next_sequence;
        completed.extend(serials);
        assert!(
            !live_records(&fixture)
                .iter()
                .any(|record| record.kind == AtomicCommitKind::CursorOnly
                    && record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok),
            "no cursor-only commit while this FIFO successor is queued at vblank {vblank}"
        );
        scene_cursor_count = assert_only_accepted_scene_with_cursor(&live_records(&fixture));
        assert_eq!(clock_cell.get(), injected_now, "FIFO stays below 2 ms");
        if vblank + 1 < VBLANKS {
            assert!(fixture.backend.scene.has_pending_page_flip(0));
        }
    }
    assert_eq!(completed.len(), VBLANKS as usize);
    assert!(
        scene_cursor_count >= 28,
        "got {scene_cursor_count} SceneWithCursor commits over {VBLANKS} FIFO vblanks"
    );
    for output_idx in 0..fixture.backend.platform.outputs.len() {
        if fixture.backend.scene.has_pending_page_flip(output_idx) {
            fixture.retire_output(output_idx, 200);
        }
    }
    assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
    teardown(fixture);
    drop(drm_peers);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: defer the idle cursor-only decision until a later vblank; this pass must commit before run_core returns.
fn atomic_cursor_only_idle_scene_commits_cursor_in_retirement_iteration() {
    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let drm_peers = make_devices_pollable(&mut fixture);
    for vblank in 0..3_u32 {
        if vblank > 0 {
            fixture.retire_output(0, 201 + vblank * 2);
        }
        damage_cursor_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.backend).expect("prime idle scene flip");
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        Backend::warp_pointer_root(
            &mut fixture.backend,
            &mut fixture.state,
            132 + i32::try_from(vblank).unwrap(),
            117,
        );
        let before = live_records(&fixture).len();
        queue_output_flip(&fixture, 0, 200 + vblank * 2);
        run_core_flip_iteration(&mut fixture, &drm_peers[0].peer);
        let records = live_records(&fixture);
        let pass_records = &records[before..];
        assert_eq!(
            pass_records
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            [AtomicCommitKind::CursorOnly],
            "idle cursor decision is submitted at vblank {vblank}; pending={:?}, scene_pending={}, backlog={}, cursor_inflight={}, eligible={}, records={:?}",
            fixture.backend.pstate.pending_cursor_decisions,
            fixture.backend.scene.has_pending_page_flips(),
            fixture.backend.pstate.request_backlog,
            fixture.backend.platform.any_cursor_commit_in_flight(),
            fixture
                .backend
                .platform
                .has_eligible_atomic_cursor_intent_for_output(0),
            pass_records
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
        );
        assert!(!fixture.backend.scene.has_pending_page_flips());
    }
    fixture.retire_output(0, 207);
    assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
    teardown(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore the generic KMS mark_dirty wake for no-op requests; each cursor should then ride an unwanted SceneWithCursor commit.
fn atomic_cursor_only_static_scene_request_flood_keeps_cursor_only_commits() {
    use yserver_core::backend::Backend;

    const NOOPS_PER_VBLANK: usize = 40;
    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let drm_peers = make_devices_pollable(&mut fixture);

    // Leave a cursor commit in flight so the first run_core vblank retires a
    // real flip and makes its newly queued motion pass through C12 arbitration.
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 101, 91);
    Backend::maybe_composite(&mut fixture.backend).expect("submit seed cursor-only commit");
    assert_eq!(
        live_records(&fixture).last().map(|record| record.kind),
        Some(AtomicCommitKind::CursorOnly),
    );
    assert!(fixture.backend.platform.any_cursor_commit_in_flight());

    let mut client = install_client(&mut fixture.state, CLIENT);
    let mut sequence = 1_u16;
    let before_scenario = live_records(&fixture).len();
    let composite_submits_before = fixture.backend.telemetry.lifetime.composite_submits;
    for vblank in 0..VBLANKS {
        let x = 110 + i32::try_from(vblank).expect("vblank fits") % 30;
        let y = 100 + i32::try_from(vblank).expect("vblank fits") % 20;
        queue_output_flip(&fixture, 0, 300 + vblank);
        let before_pass = live_records(&fixture).len();
        let (next_client, next_sequence, completed) = run_core_pass(
            &mut fixture,
            client,
            &drm_peers[0].peer,
            sequence,
            WINDOW.0,
            None,
            false,
            NOOPS_PER_VBLANK,
            Some((x, y)),
        );
        let records = live_records(&fixture);
        let pass_records = &records[before_pass..];
        assert_eq!(completed.len(), 0);
        assert_eq!(
            pass_records
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            [AtomicCommitKind::CursorOnly],
            "static scene vblank {vblank} must accept only the motion's cursor commit; records={:?}, cursor=({}, {}), backlog={}, marks={:?}, eligible={}, scene_dirty={}",
            pass_records
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            fixture.backend.core.cursor_x,
            fixture.backend.core.cursor_y,
            fixture.backend.pstate.request_backlog,
            fixture.backend.pstate.pending_cursor_decisions,
            fixture.backend.platform.has_eligible_atomic_cursor_intent(),
            fixture.backend.scene.scene_structure_dirty,
        );
        assert_eq!(
            fixture.backend.telemetry.lifetime.composite_submits, composite_submits_before,
            "the static scene must not submit a composite at vblank {vblank}",
        );
        assert_eq!(
            pass_records[0].result,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
            "cursor motion vblank {vblank} is accepted",
        );
        assert!(
            carries_cursor_position(&fixture, &pass_records[0]),
            "cursor-only commit carries the motion at vblank {vblank}"
        );
        let output = &fixture.backend.platform.outputs[0];
        let cursor_properties = fixture
            .backend
            .platform
            .device_for_key(output.key.device_key)
            .and_then(|device| device.cursor.plane.as_ref())
            .and_then(|plane| plane.properties_for(output.output.crtc))
            .expect("recording output has cursor position properties");
        let property_value = |property| {
            pass_records[0]
                .properties
                .iter()
                .find_map(|update| (update.property == property).then_some(update.value))
        };
        assert_eq!(
            property_value(cursor_properties.crtc_x),
            Some(u64::try_from(x).expect("positive cursor x")),
            "cursor-only commit carries x for vblank {vblank}",
        );
        assert_eq!(
            property_value(cursor_properties.crtc_y),
            Some(u64::try_from(y).expect("positive cursor y")),
            "cursor-only commit carries y for vblank {vblank}",
        );
        assert!(
            !fixture.backend.scene.has_pending_page_flips(),
            "request-only work must not create a scene flip at vblank {vblank}"
        );
        assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
        assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
        assert!(fixture.backend.platform.any_cursor_commit_in_flight());
        sequence = next_sequence.wrapping_add(1);
        client = next_client;
    }

    let scenario_records = &live_records(&fixture)[before_scenario..];
    assert_eq!(scenario_records.len(), VBLANKS as usize);
    assert!(
        scenario_records
            .iter()
            .all(|record| record.kind == AtomicCommitKind::CursorOnly)
    );
    fixture.retire_output(0, 400);
    assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
    teardown(fixture);
    drop(drm_peers);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: let a pending hardware cursor intent alone trigger a scene submit; this must stay cursor-only.
fn atomic_cursor_only_static_scene_light_request_traffic_stays_cursor_only() {
    use yserver_core::backend::Backend;

    const LIGHT_REQUESTS_PER_VBLANK: usize = 4;
    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let drm_peers = make_devices_pollable(&mut fixture);

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 101, 91);
    Backend::maybe_composite(&mut fixture.backend).expect("submit seed cursor-only commit");
    assert_eq!(
        live_records(&fixture).last().map(|record| record.kind),
        Some(AtomicCommitKind::CursorOnly),
    );
    assert!(fixture.backend.platform.any_cursor_commit_in_flight());

    let mut client = install_client(&mut fixture.state, CLIENT);
    let mut sequence = 1_u16;
    let before_scenario = live_records(&fixture).len();
    let composite_submits_before = fixture.backend.telemetry.lifetime.composite_submits;
    for vblank in 0..VBLANKS {
        let x = 110 + i32::try_from(vblank).expect("vblank fits") % 30;
        let y = 100 + i32::try_from(vblank).expect("vblank fits") % 20;
        queue_output_flip(&fixture, 0, 300 + vblank);
        run_core_flip_iteration(&mut fixture, &drm_peers[0].peer);
        assert!(!fixture.backend.scene.has_pending_page_flips());
        assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
        let before_pass = live_records(&fixture).len();
        let (next_client, next_sequence, completed) = run_core_pass(
            &mut fixture,
            client,
            &drm_peers[0].peer,
            sequence,
            WINDOW.0,
            None,
            false,
            LIGHT_REQUESTS_PER_VBLANK,
            Some((x, y)),
        );
        let records = live_records(&fixture);
        let pass_records = &records[before_pass..];
        assert!(completed.is_empty());
        assert!(
            !fixture.backend.pstate.request_backlog,
            "light request traffic must remain below the dispatch budget at vblank {vblank}",
        );
        assert_eq!(
            pass_records
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            [AtomicCommitKind::CursorOnly],
            "static scene vblank {vblank} must accept only the cursor motion; records={:?}, cursor=({}, {}), marks={:?}, eligible={}, scene_dirty={}",
            pass_records
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            fixture.backend.core.cursor_x,
            fixture.backend.core.cursor_y,
            fixture.backend.pstate.pending_cursor_decisions,
            fixture.backend.platform.has_eligible_atomic_cursor_intent(),
            fixture.backend.scene.scene_structure_dirty,
        );
        assert_eq!(
            pass_records[0].result,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
            "cursor motion vblank {vblank} is accepted",
        );
        assert!(
            carries_cursor_position(&fixture, &pass_records[0]),
            "cursor-only commit carries the motion at vblank {vblank}"
        );
        let output = &fixture.backend.platform.outputs[0];
        let cursor_properties = fixture
            .backend
            .platform
            .device_for_key(output.key.device_key)
            .and_then(|device| device.cursor.plane.as_ref())
            .and_then(|plane| plane.properties_for(output.output.crtc))
            .expect("recording output has cursor position properties");
        let property_value = |property| {
            pass_records[0]
                .properties
                .iter()
                .find_map(|update| (update.property == property).then_some(update.value))
        };
        assert_eq!(
            property_value(cursor_properties.crtc_x),
            Some(u64::try_from(x).expect("positive cursor x")),
            "cursor-only commit carries x for vblank {vblank}",
        );
        assert_eq!(
            property_value(cursor_properties.crtc_y),
            Some(u64::try_from(y).expect("positive cursor y")),
            "cursor-only commit carries y for vblank {vblank}",
        );
        assert_eq!(
            fixture.backend.telemetry.lifetime.composite_submits, composite_submits_before,
            "a hardware cursor intent alone must not compose at vblank {vblank}",
        );
        assert!(
            !fixture.backend.scene.has_pending_page_flips(),
            "light request traffic must not create a scene flip at vblank {vblank}"
        );
        assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
        assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
        assert!(fixture.backend.platform.any_cursor_commit_in_flight());
        sequence = next_sequence.wrapping_add(1);
        client = next_client;
    }

    assert_eq!(
        live_records(&fixture)[before_scenario..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![AtomicCommitKind::CursorOnly; VBLANKS as usize],
        "the whole light-traffic run accepts exactly one cursor-only commit per vblank",
    );
    fixture.retire_output(0, 400);
    assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
    teardown(fixture);
    drop(drm_peers);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: drop the software-cursor repaint exception; every motion vblank loses its scene commit.
fn atomic_cursor_only_static_scene_request_flood_preserves_software_cursor_repaints() {
    use crate::kms::render::scene::{CursorAcceptedMode, CursorPrimaryContent};
    use yserver_core::backend::Backend;

    const NOOPS_PER_VBLANK: usize = 40;
    let Some(mut fixture) = AtomicCursorFixture::new_with_cursor_capabilities(Some(1), Some(1))
        .expect("build ineligible-cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let cursor = super::round_b_producers::create_argb_cursor(
        &mut fixture.backend,
        [0x11, 0x22, 0xdd, 0xff],
        0,
        0,
    );
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("define oversized cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial software cursor frame");
    assert_eq!(
        live_records(&fixture).last().map(|record| record.kind),
        Some(AtomicCommitKind::SceneWithCursor),
        "the cursor exceeds the output plane's 1x1 limits and is painted into the primary",
    );
    let initial_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("initial software cursor receipt");
    assert_eq!(initial_receipt.mode, CursorAcceptedMode::Software);
    assert!(matches!(
        initial_receipt.primary_cursor,
        CursorPrimaryContent::Software { pos: (100, 90), .. }
    ));
    fixture.retire_output(0, 299);
    assert!(fixture.backend.scene.output_uses_software_cursor(0));

    let drm_peers = make_devices_pollable(&mut fixture);
    let mut client = install_client(&mut fixture.state, CLIENT);
    let mut sequence = 1_u16;
    let mut observed_idle_flood = false;
    for settle in 0..8_u32 {
        let before_idle_pass = live_records(&fixture).len();
        let (next_client, next_sequence, completed) = run_core_pass(
            &mut fixture,
            client,
            &drm_peers[0].peer,
            sequence,
            WINDOW.0,
            None,
            false,
            NOOPS_PER_VBLANK,
            None,
        );
        assert!(completed.is_empty());
        client = next_client;
        sequence = next_sequence.wrapping_add(1);
        let records = live_records(&fixture);
        if records.len() == before_idle_pass {
            observed_idle_flood = true;
            break;
        }
        assert_eq!(
            records[before_idle_pass..]
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            [AtomicCommitKind::Scene],
            "settling the software cursor cannot submit a cursor-only update",
        );
        fixture.retire_output(0, 280 + settle);
    }
    assert!(
        observed_idle_flood,
        "the static software cursor reaches an idle scene before the motion flood",
    );
    drop(client);
    let before_scenario = live_records(&fixture).len();
    for vblank in 0..VBLANKS {
        let x = 110 + i32::try_from(vblank).expect("vblank fits") % 30;
        let y = 100 + i32::try_from(vblank).expect("vblank fits") % 20;
        queue_output_flip(&fixture, 0, 300 + vblank);
        let before_pass = live_records(&fixture).len();
        run_core_flood_motion_iteration(
            &mut fixture,
            &drm_peers[0].peer,
            sequence,
            NOOPS_PER_VBLANK,
            (x, y),
        );
        sequence = sequence.wrapping_add(
            u16::try_from(NOOPS_PER_VBLANK).expect("request flood length fits sequence"),
        );
        let records = live_records(&fixture);
        let pass_records = &records[before_pass..];
        assert!(
            fixture.backend.pstate.request_backlog,
            "the request flood remains queued at the vblank {vblank} composition tail",
        );
        assert_eq!(
            pass_records
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            [AtomicCommitKind::Scene],
            "software cursor motion must be scene work at vblank {vblank}; records={:?}, cursor=({}, {}), dirty={}",
            pass_records
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            fixture.backend.core.cursor_x,
            fixture.backend.core.cursor_y,
            fixture.backend.scene.scene_structure_dirty,
        );
        assert_eq!(
            pass_records[0].result,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
            "software cursor scene commit is accepted at vblank {vblank}",
        );
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted software cursor scene receipt");
        assert_eq!(receipt.mode, CursorAcceptedMode::Software);
        assert!(
            matches!(receipt.primary_cursor, CursorPrimaryContent::Software { pos, .. } if pos == (x, y)),
            "software primary must carry the vblank {vblank} motion at ({x}, {y}): {receipt:?}",
        );
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        assert!(!fixture.backend.scene.output_uses_hardware_cursor(0));
    }

    assert_eq!(
        live_records(&fixture)[before_scenario..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![AtomicCommitKind::Scene; VBLANKS as usize],
        "each motion vblank has exactly one scene repaint",
    );
    fixture.retire_output(0, 400);
    Backend::set_request_backlog(&mut fixture.backend, false);
    assert!(!fixture.backend.pstate.request_backlog);
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
    let cursor_pixel = super::round_c_frames::read_output_pixel(&mut fixture, 139, 109);
    let background_pixel = super::round_c_frames::read_output_pixel(&mut fixture, 150, 120);
    assert_ne!(
        &cursor_pixel[..3],
        &background_pixel[..3],
        "the retired primary contains the software cursor at its latest motion",
    );
    teardown(fixture);
    drop(drm_peers);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit the queued cursor from the Present successor; this pass must be exactly SceneWithCursor.
fn atomic_cursor_only_budget_backlog_waits_for_present_scene() {
    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let pixmap = install_present_pixmap(&mut fixture);
    let drm_peers = make_devices_pollable(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("prime scene flip");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 145, 133);
    let before_scenario = live_records(&fixture).len();
    queue_output_flip(&fixture, 0, 301);
    let client = install_client(&mut fixture.state, CLIENT);
    let (client, _, completed) = run_core_pass(
        &mut fixture,
        client,
        &drm_peers[0].peer,
        1,
        WINDOW.0,
        Some(pixmap),
        true,
        40,
        None,
    );
    assert_eq!(completed.len(), 1);
    let all_records = live_records(&fixture);
    let records = &all_records[before_scenario..];
    assert!(
        records
            .iter()
            .all(|record| record.kind != AtomicCommitKind::CursorOnly
                || record.result != crate::drm::atomic::tests_support::ScriptedResult::Ok),
        "queued Present stays protected while the dispatch budget leaves it behind"
    );
    assert!(
        records.iter().any(|record| {
            record.kind == AtomicCommitKind::SceneWithCursor
                && record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
        }),
        "the dispatched Present successor carries the cursor"
    );
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [AtomicCommitKind::SceneWithCursor],
        "the budgeted Present pass accepts exactly one scene commit carrying the cursor"
    );
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    fixture.retire_output(0, 302);
    assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
    let _ = client;
    teardown(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: classify a queued grouped direct successor only for its selected CRTC; output B would submit CursorOnly.
fn atomic_cursor_only_grouped_direct_successor_protects_every_output() {
    use yserver_core::backend::Backend;

    let injected_now = std::time::Instant::now();
    let clock_cell = std::rc::Rc::new(std::cell::Cell::new(injected_now));
    let clock_reader = std::rc::Rc::clone(&clock_cell);
    let clock: std::rc::Rc<dyn Fn() -> std::time::Instant> =
        std::rc::Rc::new(move || clock_reader.get());
    let Some(mut fixture) = AtomicCursorFixture::new_with_two_outputs_and_clock(clock)
        .expect("build two-output cursor fixture")
    else {
        return;
    };
    assert_eq!(fixture.backend.platform.outputs.len(), 2);
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    // Span both sides of the output boundary so the cursor plane is live on
    // output B before the grouped direct transaction.
    let root = fixture.backend.core.window_id;
    let spanning_cursor = super::round_b_producers::create_argb_cursor_extent(
        &mut fixture.backend,
        [0x45, 0x56, 0x67, 0xff],
        32,
        4,
        0,
        0,
    );
    Backend::define_cursor(&mut fixture.backend, None, root, spanning_cursor.as_raw())
        .expect("select spanning cursor");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 790, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("move cursor to output B");
    for output_idx in 0..2 {
        if fixture.backend.scene.has_pending_page_flip(output_idx) {
            fixture.retire_output(output_idx, 2 + u32::try_from(output_idx).unwrap());
        }
    }
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(1));

    let drm_peers = make_devices_pollable(&mut fixture);
    let direct = setup_cursor_only_direct_present(&mut fixture);
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let _current_present =
        super::round_b_producers::admit_held_direct_cursorless(&mut fixture, &direct, 24_300, 410);
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());

    // Submit a grouped predecessor, then let only output B retire. The next
    // queued Present selects A, while its grouped direct commit will target A
    // and B. B must keep its cursor intent parked through that successor.
    let (candidate, event) = super::round_a::direct_present_for_id(&direct, 24_400);
    Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
    assert!(
        Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit grouped direct predecessor")
    );
    assert_eq!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .pending
            .as_ref()
            .expect("grouped predecessor is in flight")
            .awaiting_outputs,
        HashSet::from([0, 1])
    );

    let (successor, successor_event) = super::round_a::direct_present_for_id(&direct, 24_401);
    Backend::note_present_scanout_candidate(&mut fixture.backend, successor);
    assert!(
        Backend::try_present_direct(&mut fixture.backend, successor, successor_event)
            .expect("queue grouped direct successor through Backend")
    );
    assert_eq!(
        fixture
            .backend
            .pstate
            .crtc_key_by_id
            .get(&successor.crtc_id),
        Some(&fixture.backend.platform.outputs[0].key),
        "the grouped successor is paced by output A",
    );
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_some());

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 791, 100);
    assert!(
        fixture
            .backend
            .platform
            .has_eligible_atomic_cursor_intent_for_output(1)
    );

    queue_output_flip(&fixture, 1, 24_401);
    let before_successor = live_records(&fixture).len();
    let (poll, sender, receiver) = channel().expect("create grouped-successor poll channel");
    let shutdown = sender.clone_handle();
    let handshake_sender = sender.bind();
    let (first_response_tx, first_response_rx) = crossbeam_channel::bounded(1);
    handshake_sender
        .send(Message::SetupAllocate {
            id: CLIENT,
            response_tx: first_response_tx,
        })
        .expect("queue first loop checkpoint");
    let mut drm_peer = drm_peers[0]
        .peer
        .try_clone()
        .expect("clone grouped-successor DRM peer");
    drm_peer
        .write_all(&[1])
        .expect("signal grouped predecessor retirement");
    let driver = std::thread::spawn(move || {
        first_response_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("run_core processes the B retirement iteration");
        let (second_response_tx, second_response_rx) = crossbeam_channel::bounded(1);
        handshake_sender
            .send(Message::SetupAllocate {
                id: CLIENT,
                response_tx: second_response_tx,
            })
            .expect("queue the following before_block checkpoint");
        second_response_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("run_core reaches the next before_block decision point");
        shutdown
            .send(Message::Shutdown)
            .expect("stop grouped-successor core loop");
    });
    run_core(
        poll,
        receiver,
        sender.clone_handle(),
        &mut fixture.state,
        &mut fixture.backend,
        [],
        &ClientIdAllocator::new(),
        AuthState::new(None),
        ResetPolicy::NoReset,
        None,
    )
    .expect("run_core handles grouped predecessor retirement");
    driver
        .join()
        .expect("join grouped-successor checkpoint driver");
    assert_eq!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .pending
            .as_ref()
            .expect("grouped predecessor remains on output A")
            .awaiting_outputs,
        HashSet::from([0])
    );
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_some());
    let after_b_retirement = live_records(&fixture);
    let b_pass = &after_b_retirement[before_successor..];
    assert!(
        b_pass.iter().all(|record| {
            record.kind != AtomicCommitKind::CursorOnly
                || record.result != crate::drm::atomic::tests_support::ScriptedResult::Ok
        }),
        "output B must wait for the grouped successor even though it selects A; records={:?}",
        b_pass.iter().map(|record| record.kind).collect::<Vec<_>>(),
    );
    assert!(
        fixture
            .backend
            .pstate
            .pending_cursor_decisions
            .contains_key(&crate::kms::render::platform::CrtcKey::for_output(
                &fixture.backend.platform.outputs[1]
            ))
    );
    assert!(!fixture.backend.pstate.request_backlog);

    // Retiring the last predecessor output submits the grouped successor;
    // its accepted transaction carries B's retained cursor position.
    fixture.retire_output(0, 24_402);
    let after_successor = live_records(&fixture);
    let successor_records = &after_successor[before_successor..];
    assert_eq!(
        successor_records
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        [AtomicCommitKind::SceneWithCursor],
        "the two-output run submits only the grouped successor",
    );
    assert!(carries_cursor_position_for_output(
        &fixture,
        &successor_records[0],
        1
    ));
    assert!(fixture.backend.pstate.scanout_m2.pending.is_some());
    fixture.retire_output(0, 24_403);
    fixture.retire_output(1, 24_404);
    assert!(fixture.backend.pstate.scanout_m2.pending.is_none());
    Backend::maybe_composite(&mut fixture.backend).expect("clear consumed decision mark");
    assert!(
        fixture.backend.pstate.pending_cursor_decisions.is_empty(),
        "retired grouped successor leaves stale marks {:?}",
        fixture.backend.pstate.pending_cursor_decisions
    );
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
    let retired_idles = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    assert!(
        !retired_idles.is_empty(),
        "grouped direct retirement drains both Present idle obligations",
    );
    fixture
        .backend
        .disable_output()
        .expect("blocking Backend teardown detaches the grouped cursor");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    fixture.shutdown_drawables();
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
    drop(drm_peers);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: submit one decision point late, submit before 2 ms, or remove the 2 ms bound.
fn atomic_cursor_only_request_flood_expires_at_first_decision_after_two_ms() {
    use std::{
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
        thread,
    };

    const FLOOD_REQUEST_COUNT: u16 = 128;
    const RETIREMENT_LOOP_TIME_US: u64 = 0;
    const EXPIRY_AFTER_US: u64 = 2_000;

    let loop_elapsed_us = Arc::new(AtomicU64::new(RETIREMENT_LOOP_TIME_US));
    let clock_elapsed = Arc::clone(&loop_elapsed_us);
    let clock_origin = std::time::Instant::now();
    let clock: std::rc::Rc<dyn Fn() -> std::time::Instant> = std::rc::Rc::new(move || {
        let elapsed = clock_elapsed.load(Ordering::SeqCst);
        clock_origin + Duration::from_micros(elapsed)
    });
    let decision_points = Arc::new(Mutex::new(Vec::<DecisionPointRecord>::new()));
    let next_decision_id = Arc::new(AtomicU64::new(0));
    let current_decision_id = Arc::new(AtomicU64::new(0));
    let observer_active = Arc::new(AtomicBool::new(true));
    let (driver_events, driver_events_rx) = std::sync::mpsc::channel();
    let observer_events = driver_events.clone();
    let observer_ids = Arc::clone(&next_decision_id);
    let observer_current_decision_id = Arc::clone(&current_decision_id);
    let observer_active_flag = Arc::clone(&observer_active);
    let observer: std::rc::Rc<
        dyn Fn(crate::kms::render::backend::kms::state::CursorDecisionPoint, usize),
    > = std::rc::Rc::new(move |point, output_idx| {
        if !observer_active_flag.load(Ordering::SeqCst) {
            return;
        }
        let id = observer_ids.fetch_add(1, Ordering::SeqCst) + 1;
        let (acknowledged, continue_decision) = std::sync::mpsc::sync_channel(0);
        observer_events
            .send(FloodDriverEvent::DecisionPoint(DecisionPointRendezvous {
                id,
                point,
                output_idx,
                acknowledged,
            }))
            .expect("flood driver observes each production decision point");
        if continue_decision.recv().is_err() {
            return;
        }
        observer_current_decision_id.store(id, Ordering::SeqCst);
    });
    let Some(mut fixture) = AtomicCursorFixture::new_with_clock_and_cursor_decision_observer(
        std::rc::Rc::clone(&clock),
        observer,
    )
    .expect("build clock-controlled cursor fixture") else {
        return;
    };
    begin_hw_cursor(&mut fixture);
    let drm_peers = make_devices_pollable(&mut fixture);
    let _flood_client = install_client(&mut fixture.state, ClientId(2));
    fixture
        .state
        .sync_awaits
        .insert(ClientId(2).0, yserver_core::server::SyncAwait::default());
    let cursor_submissions = Arc::new(Mutex::new(Vec::<CursorSubmissionAtDecision>::new()));
    let executor = std::rc::Rc::new(ClockedCommitExecutor {
        inner: std::rc::Rc::clone(&fixture.recorders.atomic),
        clock: std::rc::Rc::clone(&clock),
        time_origin: clock_origin,
        current_decision_id: Arc::clone(&current_decision_id),
        cursor_submissions: Arc::clone(&cursor_submissions),
        driver_events: driver_events.clone(),
    });
    fixture.backend.platform.devices[0].atomic_commit_sink =
        std::rc::Rc::new(crate::drm::atomic::AtomicCommitSink::new(executor));
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("prime scene flip");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    let before_scenario_submissions = cursor_submissions
        .lock()
        .expect("cursor submission recorder is not poisoned")
        .len();
    let (poll, sender, receiver) = channel().expect("create core poll channel");
    let bound = sender.bind();
    for sequence in 1..=FLOOD_REQUEST_COUNT {
        request(&bound, ClientId(2), sequence, 127, 0, 1, Vec::new());
    }
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 151, 139);
    assert!(
        fixture
            .backend
            .platform
            .has_eligible_atomic_cursor_intent_for_output(0)
    );
    assert_eq!(
        loop_elapsed_us.load(Ordering::SeqCst),
        RETIREMENT_LOOP_TIME_US
    );
    // Retirement samples the injected clock before any decision callback can
    // advance it. The observer rendezvous then holds each real decision point
    // until the driver has assigned that point's exact loop time.
    queue_output_flip(&fixture, 0, 350);
    let before_scenario = live_records(&fixture).len();
    let shutdown = sender.clone_handle();
    let driver_elapsed = Arc::clone(&loop_elapsed_us);
    let driver_decisions = Arc::clone(&decision_points);
    let driver = thread::spawn(move || {
        let mut shutdown_sent = false;
        loop {
            match driver_events_rx.recv_timeout(Duration::from_secs(10)) {
                Ok(FloodDriverEvent::DecisionPoint(request)) => {
                    let elapsed_us = match request.id {
                        1 => RETIREMENT_LOOP_TIME_US,
                        2 => EXPIRY_AFTER_US - 1,
                        3 => EXPIRY_AFTER_US + 1,
                        4 => EXPIRY_AFTER_US + 1_001,
                        id => EXPIRY_AFTER_US + 1_001 + id.saturating_sub(4) * 1_000,
                    };
                    driver_elapsed.store(elapsed_us, Ordering::SeqCst);
                    driver_decisions
                        .lock()
                        .expect("decision point recorder is not poisoned")
                        .push(DecisionPointRecord {
                            id: request.id,
                            point: request.point,
                            output_idx: request.output_idx,
                            elapsed_us,
                        });
                    request
                        .acknowledged
                        .send(())
                        .expect("core is waiting for this decision point's loop time");
                    if request.id >= 6 && !shutdown_sent {
                        shutdown
                            .send(Message::Shutdown)
                            .expect("stop bounded request-flood scenario");
                        shutdown_sent = true;
                    }
                }
                Ok(FloodDriverEvent::CursorSubmitted(_decision_id)) if !shutdown_sent => {
                    shutdown
                        .send(Message::Shutdown)
                        .expect("stop after the cursor deadline commit");
                    shutdown_sent = true;
                }
                Ok(FloodDriverEvent::CursorSubmitted(_)) => {}
                Ok(FloodDriverEvent::Stop) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if !shutdown_sent => {
                    shutdown
                        .send(Message::Shutdown)
                        .expect("stop a flood scenario that missed its decision rendezvous");
                    shutdown_sent = true;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    let mut drm_peer = drm_peers[0].peer.try_clone().expect("clone DRM peer");
    drm_peer
        .write_all(&[1])
        .expect("signal flood flip readiness");
    run_core(
        poll,
        receiver,
        sender.clone_handle(),
        &mut fixture.state,
        &mut fixture.backend,
        [],
        &ClientIdAllocator::new(),
        AuthState::new(None),
        ResetPolicy::NoReset,
        None,
    )
    .expect("run_core services the cursor deadline during request flood");
    observer_active.store(false, Ordering::SeqCst);
    driver_events
        .send(FloodDriverEvent::Stop)
        .expect("stop the decision-point driver");
    driver.join().expect("join controlled-clock flood driver");
    let all_records = live_records(&fixture);
    let records = &all_records[before_scenario..];
    let decisions = decision_points
        .lock()
        .expect("decision point recorder is not poisoned")
        .clone();
    let submissions = cursor_submissions
        .lock()
        .expect("cursor submission recorder is not poisoned")[before_scenario_submissions..]
        .to_vec();
    assert_eq!(
        submissions.len(),
        1,
        "the flood produces exactly one cursor-only submission at a decision point; decisions={decisions:?}, submissions={submissions:?}",
    );
    let first_submission = submissions[0];
    let submitted_decision = decisions
        .iter()
        .find(|decision| decision.id == first_submission.decision_id)
        .expect("every cursor-only submission maps to an observed decision point");
    assert!(
        submitted_decision.elapsed_us > RETIREMENT_LOOP_TIME_US + EXPIRY_AFTER_US,
        "no cursor-only commit is associated with a pre-deadline decision: decisions={decisions:?}, submissions={submissions:?}",
    );
    let first_expired_decision = decisions
        .iter()
        .find(|decision| decision.elapsed_us > RETIREMENT_LOOP_TIME_US + EXPIRY_AFTER_US)
        .expect("the scenario observes a production decision point after retirement +2 ms");
    let pending_cursor = {
        let output = &fixture.backend.platform.outputs[0];
        fixture
            .backend
            .platform
            .for_tests_atomic_cursor_pending_position(output.key.device_key, output.output.crtc)
    };
    assert!(
        records.iter().any(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
        }),
        "the 2 ms decision expires while unrelated requests remain queued; threshold={EXPIRY_AFTER_US} us, decisions={decisions:?}, marks={:?}, backlog={}, kinds={:?}, flips={}, scene_pending={}, cursor_pending={:?}",
        fixture.backend.pstate.pending_cursor_decisions,
        fixture.backend.pstate.request_backlog,
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        fixture.recorders.page_flip_events.queued_flip_count(),
        fixture.backend.scene.has_pending_page_flips(),
        pending_cursor,
    );
    assert!(fixture.backend.pstate.request_backlog);
    assert_eq!(
        decisions.first().map(|decision| decision.point),
        Some(crate::kms::render::backend::kms::state::CursorDecisionPoint::Composition),
        "the flip iteration reaches its composition-tail decision first",
    );
    assert!(
        decisions.iter().any(|decision| {
            decision.point
                == crate::kms::render::backend::kms::state::CursorDecisionPoint::BeforeBlock
                && decision.elapsed_us < RETIREMENT_LOOP_TIME_US + EXPIRY_AFTER_US
        }),
        "before_block is also observed before the 2 ms deadline: {decisions:?}",
    );
    assert!(
        decisions[..decisions
            .iter()
            .position(|decision| decision.id == first_expired_decision.id)
            .expect("first expired decision belongs to the recorded sequence")]
            .iter()
            .all(|decision| decision.elapsed_us < RETIREMENT_LOOP_TIME_US + EXPIRY_AFTER_US),
        "no earlier decision point reaches the deadline: {decisions:?}",
    );
    assert_eq!(
        first_submission.decision_id, first_expired_decision.id,
        "cursor-only commit occurs at exactly the first production decision point after retirement +2 ms; decisions={decisions:?}, submissions={submissions:?}",
    );
    assert_eq!(
        first_submission.elapsed_us, first_expired_decision.elapsed_us,
        "the submitted cursor commit uses the timestamp assigned at that exact decision point",
    );
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [AtomicCommitKind::CursorOnly],
        "the expired mark produces exactly one cursor-only commit"
    );
    if fixture.backend.scene.has_pending_page_flips()
        || fixture.backend.platform.any_cursor_commit_in_flight()
    {
        fixture.retire_output(0, 500);
    }
    Backend::maybe_composite(&mut fixture.backend).expect("settle flood cursor retirement");
    assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(
        !fixture
            .backend
            .platform
            .has_eligible_atomic_cursor_intent_for_output(0)
    );
    teardown(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: bypass the held-direct lane gate; a cursor-only commit would steal the successor's vblank.
fn atomic_cursor_only_held_direct_successor_queued_beyond_budget_carries_cursor() {
    let injected_now = std::time::Instant::now();
    let clock_cell = std::rc::Rc::new(std::cell::Cell::new(injected_now));
    let clock_reader = std::rc::Rc::clone(&clock_cell);
    let clock: std::rc::Rc<dyn Fn() -> std::time::Instant> =
        std::rc::Rc::new(move || clock_reader.get());
    let Some(mut fixture) =
        AtomicCursorFixture::new_with_clock(clock).expect("build direct cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = setup_cursor_only_direct_present(&mut fixture);
    let _present_id =
        super::round_b_producers::admit_held_direct_cursorless(&mut fixture, &direct, 24_300, 410);
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());
    assert!(fixture.backend.pstate.scanout_m2.pending.is_none());
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_none());
    let _cursor = fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 164, 148);
    assert!(
        fixture
            .backend
            .platform
            .has_eligible_atomic_cursor_intent_for_output(0)
    );
    let drm_peers = make_devices_pollable(&mut fixture);

    let window = ResourceId(direct.window_xid);
    fixture.state.resources.create_window(
        CLIENT,
        yserver_protocol::x11::CreateWindowRequest {
            depth: 24,
            window,
            parent: ROOT_WINDOW,
            x: 0,
            y: 0,
            width: direct.width,
            height: direct.height,
            class: 1,
            visual: yserver_core::resources::ROOT_VISUAL,
            ..Default::default()
        },
    );
    fixture
        .state
        .resources
        .window_mut(window)
        .expect("direct client window")
        .host_xid = WindowHandle::from_raw(direct.window_xid);
    let _ = fixture.state.resources.map_window(window);
    let source = ResourceId(0x0100_3102);
    fixture.state.resources.create_pixmap(
        CLIENT,
        CreatePixmapRequest {
            depth: 24,
            pixmap: source,
            drawable: window,
            width: direct.width,
            height: direct.height,
        },
    );
    fixture
        .state
        .resources
        .window_mut(ROOT_WINDOW)
        .expect("core root resource")
        .host_xid = WindowHandle::from_raw(fixture.backend.window_id());
    assert!(fixture.state.resources.set_pixmap_host_xid(
        source,
        PixmapHandle::from_raw(direct.source_xid).expect("direct pixmap host handle"),
    ));
    fixture.state.present_event_selections.insert(
        0x0100_7efe,
        PresentEventSelection {
            owner: CLIENT,
            window,
            event_mask: x11present::EVENT_MASK_COMPLETE_NOTIFY,
        },
    );

    let client = install_client(&mut fixture.state, CLIENT);
    let before_scenario = live_records(&fixture).len();
    queue_output_flip(&fixture, 0, 401);
    let (client, _, _) = run_core_pass(
        &mut fixture,
        client,
        &drm_peers[0].peer,
        1,
        direct.window_xid,
        Some(source.0),
        false,
        32,
        None,
    );
    let all_records = live_records(&fixture);
    let records = &all_records[before_scenario..];
    assert!(
        records.iter().all(|record| {
            record.kind != AtomicCommitKind::CursorOnly
                || record.result != crate::drm::atomic::tests_support::ScriptedResult::Ok
        }),
        "held-direct cursor lane waits while the successor remains past the request budget; kinds={:?}, backlog={}, mark={:?}, pending_successor={}",
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        fixture.backend.pstate.request_backlog,
        fixture.backend.pstate.pending_cursor_decisions,
        fixture.backend.pstate.scanout_m2.queued_successor.is_some(),
    );
    assert!(
        records.iter().any(|record| {
            record.kind != AtomicCommitKind::CursorOnly
                && carries_cursor_position(&fixture, record)
                && record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
        }),
        "the dispatched direct successor carries the cursor delta"
    );
    assert_eq!(
        records.iter().map(|record| record.kind).collect::<Vec<_>>(),
        [AtomicCommitKind::SceneWithCursor],
        "the scenario accepts one direct successor commit carrying the cursor"
    );
    assert!(
        clock_cell.get().saturating_duration_since(injected_now) < Duration::from_millis(2),
        "held-direct arbitration stays below the 2 ms expiry"
    );
    let _ = client;
    for frame in 500..520 {
        if fixture.backend.pstate.scanout_m2.pending.is_some()
            || fixture.backend.platform.any_cursor_commit_in_flight()
            || fixture.backend.scene.has_pending_page_flips()
        {
            fixture.retire_output(0, frame);
            Backend::maybe_composite(&mut fixture.backend).expect("settle direct-frame teardown");
        }
    }
    let retired_idles = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    assert!(
        !retired_idles.is_empty(),
        "direct Present cleanup drains its retired idle notifications"
    );
    assert!(fixture.backend.pstate.pending_cursor_decisions.is_empty());
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(!fixture.backend.platform.has_eligible_atomic_cursor_intent());
    teardown(fixture);
    drop(drm_peers);
}
