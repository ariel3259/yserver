use super::*;
use std::time::Duration;

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: let generic request mark_dirty calls force ordinary scene wakes while tearing.
fn task6_present_and_xtest_motion_keep_cursor_compose_cap_under_run_core_load() {
    use std::{
        collections::VecDeque,
        io::{self, Read},
        os::fd::{AsFd as _, AsRawFd as _, OwnedFd},
        rc::Rc,
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };
    use yserver_core::{
        backend::Backend,
        core_loop::{
            Message, ResetPolicy, auth::AuthState, channel, poll_tokens::ClientIdAllocator,
            run_core,
        },
        server::{BackendCapabilities, ServerState},
    };
    use yserver_protocol::x11::{
        ClientId, CreatePixmapRequest, RequestHeader, ResourceId, SequenceNumber,
    };

    const WINDOW: ResourceId = ResourceId(0x0100_7d80);
    const PIXMAP: ResourceId = ResourceId(0x0100_7d81);
    const PIXMAP_ALTERNATE: ResourceId = ResourceId(0x0100_7d82);

    type SharedFlips =
        Arc<Mutex<VecDeque<crate::kms::render::platform::flip_events::KernelPageFlip>>>;

    struct RunCoreFlipSource {
        flips: SharedFlips,
        drained: Arc<AtomicUsize>,
        drained_changed: Arc<Condvar>,
    }

    impl crate::kms::render::platform::flip_events::PageFlipEventSource for RunCoreFlipSource {
        fn drain_events(
            &self,
            device: &crate::drm::Device,
        ) -> io::Result<(
            Vec<crate::kms::render::platform::flip_events::KernelPageFlip>,
            Vec<crate::kms::render::platform::flip_events::KernelCrtcSequence>,
        )> {
            let fd = device.as_fd().as_raw_fd();
            loop {
                let mut poll_fd = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: `poll_fd` is one initialized record for this readiness check.
                let ready = unsafe { libc::poll(&mut poll_fd, 1, 0) };
                if ready <= 0 || poll_fd.revents & libc::POLLIN == 0 {
                    break;
                }
                let mut wake_bytes = [0_u8; 64];
                // SAFETY: `wake_bytes` is writable for its full length; the test DRM socket is
                // nonblocking and readable only after the driver has queued flip records.
                let read =
                    unsafe { libc::read(fd, wake_bytes.as_mut_ptr().cast(), wake_bytes.len()) };
                if read <= 0 {
                    break;
                }
            }
            let flips: Vec<_> = self.flips.lock().expect("flip queue").drain(..).collect();
            if !flips.is_empty() {
                self.drained.fetch_add(flips.len(), Ordering::SeqCst);
                self.drained_changed.notify_all();
            }
            Ok((flips, Vec::new()))
        }

        fn drain_ready_events(
            &self,
            device: &crate::drm::Device,
        ) -> io::Result<
            Option<(
                Vec<crate::kms::render::platform::flip_events::KernelPageFlip>,
                Vec<crate::kms::render::platform::flip_events::KernelCrtcSequence>,
            )>,
        > {
            if self.flips.lock().expect("flip queue").is_empty() {
                return Ok(None);
            }
            self.drain_events(device).map(Some)
        }
    }

    struct SharedCommitRecorder {
        kinds: Mutex<Vec<AtomicCommitKind>>,
        changed: Condvar,
    }

    struct RunCoreCommitExecutor(Arc<SharedCommitRecorder>);

    impl crate::drm::atomic::AtomicCommitExecutor for RunCoreCommitExecutor {
        fn atomic_commit(
            &self,
            _device: &crate::drm::Device,
            _flags: ::drm::control::AtomicCommitFlags,
            kind: AtomicCommitKind,
            _request: crate::drm::atomic::AtomicCommitRequest,
            _events: &[crate::drm::atomic::AtomicCommitEvent],
        ) -> io::Result<()> {
            self.0.kinds.lock().expect("commit records").push(kind);
            self.0.changed.notify_all();
            Ok(())
        }
    }

    let mut backend = live_scene_backend();
    let recorder = recording_sink(&mut backend, 0);
    let (outputs, modes) = Backend::randr_outputs_and_modes(&mut backend);
    let (screen_width, screen_height) = Backend::fb_dimensions(&backend);
    let mut state = ServerState::with_randr_outputs_and_modes(
        screen_width,
        screen_height,
        outputs,
        modes,
        BackendCapabilities::from_backend(&backend),
    );
    let stage_host = materialize_test_cow_stage(&mut backend, &mut state, ClientId(1), WINDOW);
    let pixmap = Backend::create_pixmap(&mut backend, None, 24, screen_width, screen_height)
        .expect("create Present source pixmap");
    Backend::fill_rectangle(
        &mut backend,
        None,
        pixmap.as_raw(),
        0xFF40_6080,
        0,
        0,
        screen_width,
        screen_height,
    )
    .expect("paint Present source pixmap");
    let alternate_pixmap =
        Backend::create_pixmap(&mut backend, None, 24, screen_width, screen_height)
            .expect("create alternate Present source pixmap");
    Backend::fill_rectangle(
        &mut backend,
        None,
        alternate_pixmap.as_raw(),
        0xFF80_4020,
        0,
        0,
        screen_width,
        screen_height,
    )
    .expect("paint alternate Present source pixmap");
    state.resources.create_pixmap(
        ClientId(1),
        CreatePixmapRequest {
            depth: 24,
            pixmap: PIXMAP,
            drawable: WINDOW,
            width: screen_width,
            height: screen_height,
        },
    );
    assert!(state.resources.set_pixmap_host_xid(PIXMAP, pixmap));
    state.resources.create_pixmap(
        ClientId(1),
        CreatePixmapRequest {
            depth: 24,
            pixmap: PIXMAP_ALTERNATE,
            drawable: WINDOW,
            width: screen_width,
            height: screen_height,
        },
    );
    assert!(
        state
            .resources
            .set_pixmap_host_xid(PIXMAP_ALTERNATE, alternate_pixmap)
    );
    enter_tearing_without_direct_scanout_for_state_ids(
        &mut backend,
        &mut state,
        WINDOW.0,
        stage_host,
        stage_host,
        16_000,
    );
    recorder.clear_records();
    assert!(backend.pstate.tearing.is_on());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert!(!backend.scene.has_pending_page_flips());

    let refresh_interval = backend.pstate.tearing.cursor_compose_interval(&backend.platform);
    let (device_stream, event_peer) =
        std::os::unix::net::UnixStream::pair().expect("DRM test socket");
    device_stream
        .set_nonblocking(true)
        .expect("nonblocking test DRM endpoint");
    let device = crate::drm::Device::from_inherited_kms_fd(
        OwnedFd::from(device_stream),
        "run_core tearing test DRM socket",
    );
    backend.platform.devices[0].device = Rc::new(device);

    let flips: SharedFlips = Arc::new(Mutex::new(VecDeque::new()));
    let drained = Arc::new(AtomicUsize::new(0));
    let drained_changed = Arc::new(Condvar::new());
    backend.platform.devices[0].page_flip_event_source = Rc::new(RunCoreFlipSource {
        flips: Arc::clone(&flips),
        drained: Arc::clone(&drained),
        drained_changed: Arc::clone(&drained_changed),
    });
    backend.platform.devices[0].test_page_flip_event_source = None;

    let commits = Arc::new(SharedCommitRecorder {
        kinds: Mutex::new(Vec::new()),
        changed: Condvar::new(),
    });
    backend.platform.devices[0].atomic_commit_sink = Rc::new(AtomicCommitSink::new(Rc::new(
        RunCoreCommitExecutor(Arc::clone(&commits)),
    )));
    backend.platform.devices[0]
        .atomic_commit_sink
        .set_tearing_on(true);
    backend.pstate.tearing.last_cursor_compose = None;
    backend.pstate.tearing.deferred_cursor_wake = None;
    recorder.clear_records();

    let mpv_peer = install_test_core_client(&mut state);
    let _xtest_peer = install_test_core_client_as(&mut state, ClientId(2));
    let mut endstate_peer = install_test_core_client_as(&mut state, ClientId(3));
    state
        .clients
        .get_mut(&ClientId(1).0)
        .expect("Present client")
        .event_masks
        .insert(WINDOW, 0x0000_0040); // PointerMotionMask

    let (poll, sender, receiver) = channel().expect("core loop channel");
    let request_sender = sender.bind();
    let reaction_sender = sender.bind();
    let shutdown_sender = sender.clone_handle();
    let client_sequence = Arc::new(Mutex::new(1_u16));
    let reaction_sequence = Arc::clone(&client_sequence);
    let motion_notifications = Arc::new(AtomicUsize::new(0));
    let reactions = Arc::new(AtomicUsize::new(0));
    let reader_notifications = Arc::clone(&motion_notifications);
    let reader_reactions = Arc::clone(&reactions);
    let reader_stop = Arc::new(AtomicBool::new(false));
    let reader_stop_thread = Arc::clone(&reader_stop);
    mpv_peer
        .set_read_timeout(Some(Duration::from_millis(20)))
        .expect("set synthetic mpv event timeout");
    let mpv_reader = thread::spawn(move || {
        let mut peer = mpv_peer;
        let mut event = [0_u8; 32];
        loop {
            match peer.read_exact(&mut event) {
                Ok(()) if event[0] & 0x7f == 6 => {
                    let mut sequence = reaction_sequence.lock().expect("mpv sequence");
                    reaction_sender
                        .send(Message::Request {
                            id: ClientId(1),
                            sequence: SequenceNumber(*sequence),
                            accepted_at: None,
                            header: RequestHeader {
                                opcode: 127,
                                data: 0,
                                length_units: 1,
                            },
                            body: Vec::new(),
                            attached_fd: None,
                        })
                        .expect("queue mpv response to MotionNotify");
                    *sequence = (*sequence).wrapping_add(1);
                    reader_reactions.fetch_add(1, Ordering::SeqCst);
                    reader_notifications.fetch_add(1, Ordering::SeqCst);
                }
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    if reader_stop_thread.load(Ordering::SeqCst) {
                        return;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return,
                Err(error) => panic!("read synthetic mpv events: {error}"),
            }
        }
    });

    let motion_duration = refresh_interval * 7;
    let producer_commits = Arc::clone(&commits);
    let producer_drained = Arc::clone(&drained);
    let producer_drained_changed = Arc::clone(&drained_changed);
    let expected_motion_notifications = Arc::clone(&motion_notifications);
    let producer_reader_stop = Arc::clone(&reader_stop);
    let producer_client_sequence = Arc::clone(&client_sequence);
    let run_started = Instant::now();
    let producer = thread::spawn(move || {
        let client_id = ClientId(1);
        let xtest_client_id = ClientId(2);
        let started = Instant::now();
        let end = started + motion_duration;
        let present_interval = Duration::from_nanos(1_000_000_000 / 130);
        let mut next_present = started;
        let mut xtest_sequence = 1_u16;
        let mut sent_motion = 0_usize;
        let mut sent_presents = 0_usize;
        while Instant::now() < end {
            let step = i16::try_from(sent_motion).expect("test pointer step fits");
            let x = 100 + step.rem_euclid(i16::try_from(screen_width - 100).unwrap());
            let y = 100
                + step
                    .wrapping_mul(7)
                    .rem_euclid(i16::try_from(screen_height - 100).unwrap());
            let mut body = vec![0_u8; 32];
            body[0] = 6; // MotionNotify, as sent by xdotool through XTEST.
            body[20..22].copy_from_slice(&x.to_le_bytes());
            body[22..24].copy_from_slice(&y.to_le_bytes());
            request_sender
                .send(Message::Request {
                    id: xtest_client_id,
                    sequence: SequenceNumber(xtest_sequence),
                    accepted_at: None,
                    header: RequestHeader {
                        opcode: 146,
                        data: 2,
                        length_units: 8,
                    },
                    body,
                    attached_fd: None,
                })
                .expect("queue real XTEST FakeInput motion request");
            xtest_sequence = xtest_sequence.wrapping_add(1);
            sent_motion += 1;

            let now = Instant::now();
            if now >= next_present {
                let mut present_body = vec![0_u8; 68];
                present_body[0..4].copy_from_slice(&WINDOW.0.to_le_bytes());
                let pixmap = [PIXMAP, PIXMAP_ALTERNATE][sent_presents & 1];
                present_body[4..8].copy_from_slice(&pixmap.0.to_le_bytes());
                present_body[8..12]
                    .copy_from_slice(&u32::try_from(sent_presents + 1).unwrap().to_le_bytes());
                present_body[36..40].copy_from_slice(
                    &yserver_core::present_scheduler::PRESENT_OPTION_ASYNC.to_le_bytes(),
                );
                let mut sequence = producer_client_sequence.lock().expect("Present sequence");
                request_sender
                    .send(Message::Request {
                        id: client_id,
                        sequence: SequenceNumber(*sequence),
                        accepted_at: None,
                        header: RequestHeader {
                            opcode: 145,
                            data: yserver_protocol::x11::present::PIXMAP,
                            length_units: 18,
                        },
                        body: present_body,
                        attached_fd: None,
                    })
                    .expect("queue real PresentPixmap request");
                *sequence = (*sequence).wrapping_add(1);
                sent_presents += 1;
                next_present += present_interval;
            }
            // Target ~333 Hz, within xdotool's hardware-run range and with
            // enough margin for the loaded repetition requirement.
            thread::sleep(Duration::from_millis(3));
        }
        let motion_elapsed = Instant::now().saturating_duration_since(started);

        let motion_drain_deadline = Instant::now() + Duration::from_secs(2);
        while expected_motion_notifications.load(Ordering::SeqCst) < sent_motion
            && Instant::now() < motion_drain_deadline
        {
            thread::sleep(Duration::from_millis(2));
        }
        let observed_motion = expected_motion_notifications.load(Ordering::SeqCst);

        // Keep the core running through the last cursor deadline, then retire
        // every commit so the end-state checks see an empty transaction stream.
        thread::sleep(refresh_interval * 2);
        let drain_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let accepted = producer_commits.kinds.lock().expect("commit records").len();
            if producer_drained.load(Ordering::SeqCst) >= accepted {
                break;
            }
            if Instant::now() >= drain_deadline {
                break;
            }
            let guard = producer_commits.kinds.lock().expect("commit records");
            let (_guard, _) = producer_drained_changed
                .wait_timeout(guard, Duration::from_millis(5))
                .expect("flip drain wait");
        }
        // A source drain increments `drained` before run_core retires the
        // returned events. Round-trip through the real core after the last
        // drain so shutdown cannot race the callback that clears scene flips.
        request_sender
            .send(Message::Request {
                id: ClientId(3),
                sequence: SequenceNumber(1),
                accepted_at: None,
                header: RequestHeader {
                    opcode: 43, // GetInputFocus is a state-free core round-trip.
                    data: 0,
                    length_units: 1,
                },
                body: Vec::new(),
                attached_fd: None,
            })
            .expect("queue end-state core round-trip");
        let mut barrier_reply = [0_u8; 32];
        endstate_peer
            .read_exact(&mut barrier_reply)
            .expect("read end-state core round-trip reply");
        assert_eq!(barrier_reply[0], 1, "end-state barrier gets an X11 reply");
        producer_reader_stop.store(true, Ordering::SeqCst);
        let _ = shutdown_sender.send(Message::Shutdown);
        (sent_motion, sent_presents, observed_motion, motion_elapsed)
    });

    let event_flips = Arc::clone(&flips);
    let event_commits = Arc::clone(&commits);
    let event_peer = event_peer;
    let event_stop = Arc::new(AtomicBool::new(false));
    let event_stop_thread = Arc::clone(&event_stop);
    let crtc = backend.platform.outputs[0].output.crtc;
    let event_thread = thread::spawn(move || {
        use std::io::Write;
        let mut peer = event_peer;
        let mut sent_events = 0_usize;
        loop {
            let mut kinds = event_commits.kinds.lock().expect("commit records");
            while kinds.len() == sent_events && !event_stop_thread.load(Ordering::SeqCst) {
                let (next, _) = event_commits
                    .changed
                    .wait_timeout(kinds, Duration::from_millis(20))
                    .expect("commit notification wait");
                kinds = next;
            }
            if kinds.len() == sent_events && event_stop_thread.load(Ordering::SeqCst) {
                return;
            }
            let newly_accepted = kinds.len() - sent_events;
            sent_events = kinds.len();
            drop(kinds);
            for index in 0..newly_accepted {
                let frame = u32::try_from(sent_events - newly_accepted + index + 1)
                    .expect("test frame fits");
                event_flips.lock().expect("flip queue").push_back(
                    crate::kms::render::platform::flip_events::KernelPageFlip {
                        crtc,
                        frame,
                        ust: Duration::ZERO,
                        event_at_monotonic: crate::drm::atomic::monotonic_timestamp(),
                        event_at_boottime: crate::drm::atomic::boottime_timestamp(),
                    },
                );
                peer.write_all(&[1]).expect("signal fake DRM flip event");
            }
        }
    });

    let build_calls_before = backend.telemetry.lifetime.build_scene_calls;
    run_core(
        poll,
        receiver,
        sender.clone_handle(),
        &mut state,
        &mut backend,
        [],
        &ClientIdAllocator::new(),
        AuthState::new(None),
        ResetPolicy::NoReset,
        None,
    )
    .expect("run_core drives PresentPixmap, XTEST motion, MotionNotify and fake CRTC retirements");
    let run_elapsed = run_started.elapsed();
    let (sent_motion, sent_presents, observed_motion, motion_elapsed) =
        producer.join().expect("mixed Present/motion producer");
    mpv_reader
        .join()
        .expect("MotionNotify client response thread");
    event_stop.store(true, Ordering::SeqCst);
    commits.changed.notify_all();
    event_thread.join().expect("fake DRM event producer");

    let accepted_kinds = commits.kinds.lock().expect("commit records").clone();
    let refresh_windows = run_elapsed
        .as_nanos()
        .div_ceil(refresh_interval.as_nanos())
        .max(1);
    let build_calls = backend
        .telemetry
        .lifetime
        .build_scene_calls
        .saturating_sub(build_calls_before);
    assert!(
        motion_elapsed >= refresh_interval * 5,
        "the run_core request stream must span at least five refresh intervals"
    );
    let motion_rate = f64::from(u32::try_from(sent_motion).unwrap()) / motion_elapsed.as_secs_f64();
    let present_rate =
        f64::from(u32::try_from(sent_presents).unwrap()) / motion_elapsed.as_secs_f64();
    assert!(
        (250.0..=1_000.0).contains(&motion_rate),
        "XTEST motion rate was {motion_rate:.1}/s"
    );
    assert!(
        (110.0..=150.0).contains(&present_rate),
        "PresentPixmap rate was {present_rate:.1}/s"
    );
    assert!(
        sent_presents >= 8,
        "the client must submit repeated PresentPixmap frames"
    );
    assert_eq!(
        observed_motion, sent_motion,
        "every XTEST motion reaches the fullscreen client"
    );
    assert_eq!(reactions.load(Ordering::SeqCst), observed_motion);
    assert_eq!(
        accepted_kinds,
        vec![AtomicCommitKind::Async; accepted_kinds.len()],
        "every accepted composed frame remains async while tearing is On"
    );
    assert!(
        !accepted_kinds.is_empty(),
        "client frames and pointer motion must compose"
    );
    assert!(
        accepted_kinds.len() as u128 <= sent_presents as u128 + refresh_windows,
        "{} async composed flips exceed {} client frames + {} refresh intervals over {run_elapsed:?}",
        accepted_kinds.len(),
        sent_presents,
        refresh_windows
    );
    assert!(
        build_calls as u128 <= sent_presents as u128 + refresh_windows,
        "{build_calls} build_scene calls exceed {sent_presents} client frames + {refresh_windows} refresh intervals"
    );
    assert!(backend.pstate.tearing.is_on());
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(!backend.scene.cursor_only_wake_pending());
    assert!(!backend.scene.scene_structure_dirty);
    assert!(!backend.scene.has_pending_page_flips());
    assert_eq!(drained.load(Ordering::SeqCst), accepted_kinds.len());
    assert!(backend.kms_outputs_active);
    assert!(backend.scanout_allowed());
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: ignore the active grouped cursor handoff when applying the compose interval cap.
fn task5_handoff_unflip_starts_cursor_compose_interval() {
    use yserver_core::{backend::Backend, server::ServerState};

    const WINDOW: u32 = 0x7c58;
    let mut backend = live_scene_backend();
    let recorder = recording_sink(&mut backend, 0);
    qualify_test_direct_async(&mut backend, &recorder, WINDOW, 15_050);
    let mut state = ServerState::new();
    let shape = try_direct_present_with_live_storage(
        &mut backend,
        15_149,
        WINDOW,
        800,
        600,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    )
    .expect("hold direct async frame");
    retire_direct_present(&mut backend, &mut state, 74);
    assert!(backend.pstate.scanout_m2.current.is_some());
    assert!(
        backend
            .platform
            .async_ledger
            .allows(crate::drm::async_flip::AsyncStream::Direct, &shape)
    );
    recorder.clear_records();

    // This pending timer is stale while the cursor is hidden on held direct scanout.
    // Keep the previous compose time absent so only the grouped unflip can start
    // the interval used by the following visible cursor motion.
    backend.pstate.tearing.last_cursor_compose = None;
    backend.pstate.tearing.deferred_cursor_wake =
        Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
    Backend::set_cursor_hidden(&mut backend, false);
    assert_eq!(
        backend.pstate.scanout_m2.unflip_reason,
        Some("tearing_cursor_handoff")
    );
    Backend::maybe_composite(&mut backend).expect("submit grouped cursor handoff");
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(recorder.records()[0].kind, AtomicCommitKind::Scene);
    assert!(backend.pstate.scanout_m2.current.is_some());

    let compose_time = backend
        .pstate.tearing
        .last_cursor_compose
        .expect("successful composed unflip starts the cursor interval");
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    let interval = backend.pstate.tearing.cursor_compose_interval(&backend.platform);
    Backend::warp_pointer_root(&mut backend, &mut state, 121, 100);
    assert_eq!(
        backend.pstate.tearing.deferred_cursor_wake,
        Some(compose_time + interval),
        "cursor-only motion during the interval gets one deferred wake"
    );
    Backend::maybe_composite(&mut backend).expect("cursor motion waits behind the unflip");
    assert_eq!(
        recorder.records().len(),
        1,
        "cursor-only motion must not submit another composed frame immediately"
    );

    retire_composed_frame(&mut backend, &mut state, 75);
    assert!(backend.pstate.scanout_m2.current.is_none());
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert_eq!(
        backend.pstate.tearing.deferred_cursor_wake,
        Some(compose_time + interval)
    );

    // Advance the scheduled timer through the production entry without sleeping.
    backend.pstate.tearing.deferred_cursor_wake = Some(std::time::Instant::now());
    Backend::maybe_composite(&mut backend).expect("compose the deferred cursor motion");
    assert_eq!(recorder.records().len(), 2);
    assert_eq!(recorder.records()[1].kind, AtomicCommitKind::Async);
    retire_composed_frame(&mut backend, &mut state, 76);

    assert!(backend.pstate.tearing.is_on());
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(backend.pstate.tearing.last_cursor_compose.is_some());
    assert!(!backend.pstate.scanout_m2.active());
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert!(backend.pstate.scanout_m2.queued_successor.is_none());
    assert!(!backend.scene.has_pending_page_flips());
    assert_direct_queue_drained(&backend);
    assert_commit_sequence(
        &recorder,
        &[AtomicCommitKind::Scene, AtomicCommitKind::Async],
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: make a pending cursor-only deadline defer a new W Copy frame.
fn task5_client_frame_is_never_deferred_by_cursor_cap() {
    use yserver_core::{
        backend::Backend,
        server::{BackendCapabilities, ServerState},
    };
    use yserver_protocol::x11::{ClientId, ResourceId};

    const WINDOW: ResourceId = ResourceId(0x0100_7c60);
    let mut backend = live_scene_backend();
    for device in &mut backend.platform.devices {
        device.async_page_flip = true;
    }
    let recorder = recording_sink(&mut backend, 0);
    let (outputs, modes) = Backend::randr_outputs_and_modes(&mut backend);
    let (screen_width, screen_height) = Backend::fb_dimensions(&backend);
    let mut scene_state = ServerState::with_randr_outputs_and_modes(
        screen_width,
        screen_height,
        outputs,
        modes,
        BackendCapabilities::from_backend(&backend),
    );
    let stage_host =
        materialize_test_cow_stage(&mut backend, &mut scene_state, ClientId(1), WINDOW);
    let (width, height) = Backend::fb_dimensions(&backend);
    let copy = Backend::create_pixmap(&mut backend, None, 24, width, height)
        .expect("create W Copy source");
    let mut state = enter_tearing_without_direct_scanout_for_ids(
        &mut backend,
        WINDOW.0,
        stage_host,
        stage_host,
        15_100,
    );
    recorder.clear_records();

    Backend::warp_pointer_root(&mut backend, &mut state, 100, 100);
    Backend::maybe_composite(&mut backend).expect("compose first cursor motion");
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(recorder.records()[0].kind, AtomicCommitKind::Async);
    retire_composed_frame(&mut backend, &mut state, 72);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );

    let interval = backend.pstate.tearing.cursor_compose_interval(&backend.platform);
    let last_compose = std::time::Instant::now() + std::time::Duration::from_secs(30);
    backend.pstate.tearing.last_cursor_compose = Some(last_compose);
    Backend::warp_pointer_root(&mut backend, &mut state, 101, 100);
    let expected_deferred_wake = last_compose + interval;
    assert_eq!(
        backend.pstate.tearing.deferred_cursor_wake,
        Some(expected_deferred_wake)
    );

    Backend::copy_area(
        &mut backend,
        None,
        copy.as_raw(),
        stage_host,
        0,
        0,
        0,
        0,
        width,
        height,
    )
    .expect("record W Copy damage");
    Backend::note_present_pixmap(&mut backend, copy.as_raw(), stage_host);
    assert!(
        Backend::next_wakeup(&backend)
            .is_some_and(|deadline| deadline <= std::time::Instant::now()),
        "client Copy damage must keep its immediate wake"
    );

    Backend::maybe_composite(&mut backend).expect("compose W Copy immediately");
    assert_eq!(recorder.records().len(), 2);
    assert_eq!(recorder.records()[1].kind, AtomicCommitKind::Async);
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    retire_composed_frame(&mut backend, &mut state, 73);
    assert_commit_sequence(
        &recorder,
        &[AtomicCommitKind::Async, AtomicCommitKind::Async],
    );
    assert!(backend.pstate.tearing.is_on());
    assert!(backend.pstate.tearing.last_cursor_compose.is_some());
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(!backend.scene.has_pending_page_flips());
    assert!(backend.pending_present_batches.is_empty());
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: leave the cursor suppression/deadline policy in place after exit.
fn task5_exit_restores_cursor_policy_per_state() {
    use yserver_core::{
        backend::Backend,
        randr::{CrtcTransform, FIXED_ONE, Filter},
        server::{BackendCapabilities, ServerState},
    };

    // (a) A visible cursor on an untransformed output returns to HW through
    // the next Vsync scene commit.
    let (mut hw, hw_recorder, mut hw_state) = live_tearing_cursor_backend(0x7c70, 15_200);
    let (show_executor, _hide_executor) = install_cursor_plane_with_successful_show(&mut hw);
    seed_pending_tearing_cursor_wake(&mut hw);
    exit_tearing_with_copy_present(&mut hw, 0x7c70, 15_300);
    assert_tearing_off(&hw);
    Backend::mark_dirty(&mut hw);
    Backend::maybe_composite(&mut hw).expect("compose after tearing exit to HW");
    assert_eq!(hw_recorder.records().len(), 1);
    assert_eq!(hw_recorder.records()[0].kind, AtomicCommitKind::Scene);
    retire_composed_frame(&mut hw, &mut hw_state, 74);
    assert_eq!(
        hw.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw
    );
    assert!(hw.platform.any_cursor_plane_visible());
    assert_eq!(show_executor.calls().len(), 1);
    assert_tearing_off(&hw);
    assert!(hw.kms_outputs_active);
    assert_commit_sequence(&hw_recorder, &[AtomicCommitKind::Scene]);

    // (b) A transformed output keeps a visible cursor in the composed scene.
    let (mut sw, sw_recorder, mut sw_state) = live_tearing_cursor_backend(0x7c71, 15_400);
    seed_pending_tearing_cursor_wake(&mut sw);
    let (outputs, modes) = Backend::randr_outputs_and_modes(&mut sw);
    let (screen_width, screen_height) = Backend::fb_dimensions(&sw);
    let mut randr_state = ServerState::with_randr_outputs_and_modes(
        screen_width,
        screen_height,
        outputs,
        modes,
        BackendCapabilities::from_backend(&sw),
    );
    randr_state.randr.outputs[0].current_transform = CrtcTransform::new(
        [2 * FIXED_ONE, 0, 0, 0, 2 * FIXED_ONE, 0, 0, 0, FIXED_ONE],
        Some(Filter::Bilinear),
        Vec::new(),
    )
    .expect("construct test output transform");
    Backend::randr_layout_changed(&mut sw, &mut randr_state);
    exit_tearing_with_copy_present(&mut sw, 0x7c71, 15_500);
    assert_tearing_off(&sw);
    assert!(sw.platform.any_output_transformed());
    sw_recorder.clear_records();
    Backend::mark_dirty(&mut sw);
    Backend::maybe_composite(&mut sw).expect("compose after transformed exit");
    assert_eq!(sw_recorder.records().len(), 1);
    assert_eq!(sw_recorder.records()[0].kind, AtomicCommitKind::Scene);
    retire_composed_frame(&mut sw, &mut sw_state, 75);
    assert_eq!(
        sw.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert!(!sw.platform.any_cursor_plane_visible());
    assert_tearing_off(&sw);
    assert_commit_sequence(&sw_recorder, &[AtomicCommitKind::Scene]);

    // (c) XFIXES-hidden stays Hidden after the same exit.
    let (mut hidden, hidden_recorder, mut hidden_state) =
        live_tearing_cursor_backend(0x7c72, 15_600);
    Backend::set_cursor_hidden(&mut hidden, true);
    seed_pending_tearing_cursor_wake(&mut hidden);
    exit_tearing_with_copy_present(&mut hidden, 0x7c72, 15_700);
    assert_tearing_off(&hidden);
    Backend::mark_dirty(&mut hidden);
    Backend::maybe_composite(&mut hidden).expect("compose after hidden cursor exit");
    assert_eq!(hidden_recorder.records().len(), 1);
    assert_eq!(hidden_recorder.records()[0].kind, AtomicCommitKind::Scene);
    retire_composed_frame(&mut hidden, &mut hidden_state, 76);
    assert_eq!(
        hidden.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(!hidden.platform.any_cursor_plane_visible());
    assert_tearing_off(&hidden);
    assert_commit_sequence(&hidden_recorder, &[AtomicCommitKind::Scene]);

    // (d) DPMS off clears tearing state without submitting a cursor plane
    // commit; resume owns reapplying the cursor policy.
    let (mut dark, dark_recorder, _dark_state) = live_tearing_cursor_backend(0x7c73, 15_800);
    dark.platform.for_tests_accept_output_power_commits();
    seed_pending_tearing_cursor_wake(&mut dark);
    Backend::set_dpms_power(&mut dark, 3).expect("turn test output off");
    assert_tearing_off(&dark);
    assert!(!dark.kms_outputs_active);
    dark_recorder.clear_records();
    Backend::maybe_composite(&mut dark).expect("dark scene pass is gated");
    assert!(dark_recorder.records().is_empty());
    assert_tearing_off(&dark);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: skip exit_tearing_for_window from the Backend destroy entry.
fn task5_tearing_window_destroy_exits_and_restores_cursor_policy() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7c80;
    let (mut backend, recorder, mut state) = live_tearing_cursor_backend(WINDOW, 15_900);
    seed_pending_tearing_cursor_wake(&mut backend);

    Backend::destroy_subwindow(&mut backend, None, WINDOW)
        .expect("destroy tearing owner through Backend entry");
    assert_tearing_off(&backend);
    assert!(!backend.windows.contains_key(&WINDOW));
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("compose after owner destruction");
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(recorder.records()[0].kind, AtomicCommitKind::Scene);
    retire_composed_frame(&mut backend, &mut state, 77);
    assert_tearing_off(&backend);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert!(!backend.platform.any_cursor_plane_visible());
    assert!(backend.kms_outputs_active);
    assert!(!backend.scene.has_pending_page_flips());
    assert_commit_sequence(&recorder, &[AtomicCommitKind::Scene]);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: count EINVAL and non-OS errors as accepted Async commits, count Presents only at submission, or retain tearing state on exit.
fn tearing_summary_counts_async_and_fallbacks() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7a60;
    let mut backend = live_scene_backend();
    let recorder = recording_sink(&mut backend, 0);
    qualify_test_direct_async(&mut backend, &recorder, WINDOW, 700);
    assert!(backend.pstate.tearing.is_on());

    let sink = std::rc::Rc::clone(&backend.platform.devices[0].atomic_commit_sink);
    let before = sink.for_tests_tearing_counts();
    let present_latency_before = sink.for_tests_present_latency_state();
    let mut state = ServerState::new();
    let (width, height) = backend.platform.fb_dimensions();
    let mut expected = Vec::new();

    recorder.script_result(Err(std::io::Error::other("scripted non-OS error")));
    let (candidate, event, _) = prepare_direct_present_for_window(
        &mut backend,
        900,
        WINDOW,
        width,
        height,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    );
    let before_non_os_latency = sink.for_tests_present_latency_state();
    let before_non_os_error = sink.for_tests_tearing_counts();
    let error = Backend::try_present_direct(&mut backend, candidate, event)
        .expect_err("the custom non-OS async error must be returned");
    assert_eq!(error.raw_os_error(), None);
    assert_eq!(
        sink.for_tests_tearing_counts().async_direct,
        before_non_os_error.async_direct,
        "a non-OS error is not an accepted async commit"
    );
    assert_eq!(
        sink.for_tests_present_latency_state().3.len(),
        before_non_os_latency.3.len(),
        "a non-OS error must not add a successful submit sample"
    );
    expected.push(AtomicCommitKind::Async);
    for index in 0..u64::from(crate::kms::render::backend::TEARING_ENTRY_PROBATION - 1) {
        assert!(
            try_direct_present_for_window(
                &mut backend,
                901 + index,
                WINDOW,
                width,
                height,
                yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
            )
            .is_none(),
            "eligible warmup Present {index} rebuilds probation without submitting"
        );
    }
    let before_stream = sink.for_tests_tearing_counts();
    for index in 0..3_u64 {
        let present_id = 800 + index;
        assert!(
            try_direct_present_for_window(
                &mut backend,
                present_id,
                WINDOW,
                width,
                height,
                yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
            )
            .is_some(),
            "async direct Present {present_id} must be accepted"
        );
        assert_eq!(
            recorder.records().last().unwrap().kind,
            AtomicCommitKind::Async
        );
        expected.push(AtomicCommitKind::Async);
        retire_direct_present(&mut backend, &mut state, index as u32 + 30);
    }

    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Ok(()));
    assert!(
        try_direct_present_for_window(
            &mut backend,
            803,
            WINDOW,
            width,
            height,
            yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
        )
        .is_some(),
        "the EINVAL retry must retain the direct Present"
    );
    expected.extend([AtomicCommitKind::Async, AtomicCommitKind::Scene]);
    assert_eq!(
        recorder.records().last().unwrap().kind,
        AtomicCommitKind::Scene
    );
    retire_direct_present(&mut backend, &mut state, 40);

    let after = sink.for_tests_tearing_counts();
    assert_eq!(after.async_direct - before_stream.async_direct, 3);
    assert_eq!(after.async_composed, before.async_composed);
    assert_eq!(
        after.async_einval_direct - before_stream.async_einval_direct,
        1
    );
    assert_eq!(after.async_einval_composed, before.async_einval_composed);
    assert_eq!(after.vsync_while_on - before_stream.vsync_while_on, 1);
    assert_eq!(
        after.client_present_direct - before_stream.client_present_direct,
        4
    );
    assert_eq!(
        after.direct_turnarounds - before_stream.direct_turnarounds,
        3
    );
    assert_eq!(
        after.direct_present_to_flip_events - before_stream.direct_present_to_flip_events,
        3
    );
    assert_eq!(
        after.composed_present_to_flip_events,
        before.composed_present_to_flip_events
    );
    assert_eq!(
        backend
            .platform
            .async_ledger
            .for_tests_consecutive_einval(crate::drm::async_flip::AsyncStream::Direct),
        1
    );

    // Two eligible Presents arrive behind the first in-flight commit. The
    // latest-wins slot replaces 804 with 805, so all three receipts count
    // even though only 804 and 806 reach the recording atomic sink.
    for present_id in [804, 805, 806] {
        assert!(
            try_direct_present_for_window(
                &mut backend,
                present_id,
                WINDOW,
                width,
                height,
                yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
            )
            .is_some(),
            "eligible direct Present {present_id} must be retained"
        );
    }
    assert_eq!(
        recorder.records().len(),
        7,
        "only the in-flight Present submits"
    );
    retire_direct_present(&mut backend, &mut state, 41);
    assert_eq!(
        recorder.records().len(),
        8,
        "the latest successor submits at retirement"
    );
    retire_direct_present(&mut backend, &mut state, 42);
    assert_direct_queue_drained(&backend);
    let after_successors = sink.for_tests_tearing_counts();
    assert_eq!(
        after_successors.client_present_direct - before_stream.client_present_direct,
        7
    );

    // Seed a pending scheduler deadline after all async counters are checked;
    // the real Backend Present entry below exits tearing and must clear it.
    seed_pending_tearing_cursor_wake(&mut backend);
    assert!(backend.pstate.tearing.deferred_cursor_wake.is_some());
    assert!(
        try_direct_present_for_window(&mut backend, 807, WINDOW, width, height, 0).is_some(),
        "the non-Async Present exits tearing and submits its Vsync frame"
    );
    assert_tearing_off(&backend);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
        "exit restores the XFIXES-hidden cursor policy"
    );
    assert!(!backend.platform.any_cursor_plane_visible());
    assert_eq!(backend.platform.tearing_copy_received_at, None);
    assert_eq!(backend.platform.tearing_copy_received_monotonic, None);
    let reset = sink.for_tests_tearing_counts();
    assert_eq!(reset.async_direct, 0);
    assert_eq!(reset.async_composed, 0);
    assert_eq!(reset.async_einval_direct, 0);
    assert_eq!(reset.async_einval_composed, 0);
    assert_eq!(reset.vsync_while_on, 0);
    assert_eq!(reset.client_present_direct, 0);
    assert_eq!(reset.direct_turnarounds, 0);
    assert_eq!(reset.composed_turnarounds, 0);
    assert_eq!(reset.direct_present_to_flip_events, 0);
    assert_eq!(reset.composed_present_to_flip_events, 0);
    retire_direct_present(&mut backend, &mut state, 43);
    assert_direct_queue_drained(&backend);
    assert!(!backend.scene.has_pending_page_flips());

    let records = recorder.records();
    assert_eq!(records.len(), 9);
    assert_eq!(
        records[4].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: std::io::ErrorKind::InvalidInput,
            raw_os_error: Some(libc::EINVAL),
        }
    );
    assert_eq!(
        records[5].result,
        crate::drm::atomic::tests_support::ScriptedResult::Ok
    );
    assert_eq!(
        records[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: std::io::ErrorKind::Other,
            raw_os_error: None,
        }
    );
    expected.extend([AtomicCommitKind::Async, AtomicCommitKind::Async]);
    expected.push(AtomicCommitKind::Scene);
    assert_commit_sequence(&recorder, &expected);
    assert_tearing_off(&backend);
    assert!(backend.pstate.scanout_m2.deferred_successor_skips.is_empty());
    let (eligible_presents, flips, skips, present_to_submit, present_to_flip) =
        sink.for_tests_present_latency_state();
    assert_eq!(
        eligible_presents - present_latency_before.0,
        16,
        "receipt rate includes the Off FIFO Present"
    );
    assert_eq!(
        flips - present_latency_before.1,
        7,
        "each accepted direct or fallback frame has its own event"
    );
    assert_eq!(skips - present_latency_before.2, 0);
    assert_eq!(present_to_submit.len() - present_latency_before.3.len(), 7);
    assert_eq!(present_to_flip.len() - present_latency_before.4.len(), 7);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: retain the Copy receipt timestamp unless the async ioctl succeeds, even after its Vsync fallback succeeds.
fn tearing_copy_receipt_is_consumed_by_successful_vsync_fallback() {
    use yserver_core::{
        backend::Backend,
        server::{BackendCapabilities, ServerState},
    };
    use yserver_protocol::x11::{ClientId, ResourceId};

    const WINDOW: ResourceId = ResourceId(0x0100_7d00);
    let mut backend = live_scene_backend();
    for device in &mut backend.platform.devices {
        device.async_page_flip = true;
    }
    let recorder = recording_sink(&mut backend, 0);
    let (outputs, modes) = Backend::randr_outputs_and_modes(&mut backend);
    let (screen_width, screen_height) = Backend::fb_dimensions(&backend);
    let mut core_state = ServerState::with_randr_outputs_and_modes(
        screen_width,
        screen_height,
        outputs,
        modes,
        BackendCapabilities::from_backend(&backend),
    );
    let stage_host = materialize_test_cow_stage(&mut backend, &mut core_state, ClientId(1), WINDOW);
    let (width, height) = Backend::fb_dimensions(&backend);
    let copy =
        Backend::create_pixmap(&mut backend, None, 24, width, height).expect("create Copy source");
    let mut state = enter_tearing_without_direct_scanout_for_ids(
        &mut backend,
        WINDOW.0,
        stage_host,
        stage_host,
        12_000,
    );
    recorder.clear_records();

    Backend::copy_area(
        &mut backend,
        None,
        copy.as_raw(),
        stage_host,
        0,
        0,
        0,
        0,
        width,
        height,
    )
    .expect("record Copy damage");
    Backend::note_present_pixmap(&mut backend, copy.as_raw(), stage_host);
    assert!(backend.platform.tearing_copy_received_at.is_some());
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Ok(()));

    Backend::maybe_composite(&mut backend).expect("submit Copy damage with Vsync fallback");
    assert_eq!(
        backend.platform.tearing_copy_received_at, None,
        "the accepted Vsync retry consumed the damage receipt"
    );
    assert_eq!(backend.platform.tearing_copy_received_monotonic, None);
    assert_eq!(recorder.records().len(), 2);
    assert_eq!(
        recorder.records()[0].kind,
        AtomicCommitKind::Async,
        "the stream first attempted Async"
    );
    assert_eq!(recorder.records()[1].kind, AtomicCommitKind::Scene);
    retire_composed_frame(&mut backend, &mut state, 80);

    assert!(
        try_direct_present_for_window(&mut backend, 15_501, WINDOW.0, width, height, 0).is_none(),
        "a non-Async Present exits tearing while the visible SW cursor blocks direct scanout"
    );
    assert_tearing_off(&backend);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert!(!backend.scene.has_pending_page_flips());
    assert!(backend.kms_outputs_active);
    assert_commit_sequence(
        &recorder,
        &[AtomicCommitKind::Async, AtomicCommitKind::Scene],
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: measure async turnaround with raw DRM UST instead of CLOCK_MONOTONIC event time.
fn composed_async_turnaround_uses_its_own_commit_flip_timestamp() {
    use std::{
        cell::Cell,
        os::fd::{AsFd as _, AsRawFd as _},
        rc::Rc,
        time::Duration,
    };
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x7c88;
    let (mut backend, recorder, mut state) = live_tearing_cursor_backend(WINDOW, 12_000);
    let accepted_at = Rc::new(Cell::new(
        crate::drm::atomic::monotonic_timestamp().expect("CLOCK_MONOTONIC timestamp"),
    ));
    let clock_value = Rc::clone(&accepted_at);
    let sink = Rc::new(AtomicCommitSink::new_with_monotonic_clock(
        recorder.clone(),
        Rc::new(move || Some(clock_value.get())),
    ));
    sink.set_tearing_on(true);
    backend.platform.devices[0].atomic_commit_sink = Rc::clone(&sink);

    Backend::warp_pointer_root(&mut backend, &mut state, 300, 160);
    Backend::maybe_composite(&mut backend).expect("submit cursor-only composed Async frame");
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(recorder.records()[0].kind, AtomicCommitKind::Async);

    let event_at = accepted_at.get() + Duration::from_micros(150);
    let crtc = backend.platform.outputs[0].output.crtc;
    backend.platform.devices[0]
        .test_page_flip_event_source
        .as_ref()
        .expect("recording page-flip event source")
        .queue_flip_with_timestamps(crtc, 91, Duration::from_micros(1_321_315), event_at);
    let drm_fd = backend.platform.devices[0].device.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(&mut backend, &mut state, drm_fd);

    assert_eq!(sink.for_tests_composed_turnaround_samples(), [150]);
    assert!(!backend.scene.has_pending_page_flips());
    assert!(
        try_direct_present_for_window(&mut backend, 15_001, WINDOW, 800, 600, 0).is_none(),
        "the Backend Present entry exits tearing while the visible SW cursor blocks direct scanout"
    );
    assert_tearing_off(&backend);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(!backend.scene.has_pending_page_flips());
    assert!(backend.kms_outputs_active);
    assert_commit_sequence(&recorder, &[AtomicCommitKind::Async]);
}

fn submit_direct_async_for_clock_test(
    backend: &mut KmsBackend,
    recorder: std::rc::Rc<RecordingAtomicCommitExecutor>,
    window: u32,
    present_id: u64,
) -> (
    std::rc::Rc<AtomicCommitSink>,
    Duration,
    ::drm::control::crtc::Handle,
) {
    use std::{cell::Cell, rc::Rc};
    use yserver_core::backend::Backend;

    let accepted_at = Rc::new(Cell::new(None));
    let accepted_clock = Rc::clone(&accepted_at);
    let sink = Rc::new(AtomicCommitSink::new_with_monotonic_clock(
        recorder.clone(),
        Rc::new(move || {
            let now = crate::drm::atomic::monotonic_timestamp();
            accepted_clock.set(now);
            now
        }),
    ));
    sink.set_tearing_on(true);
    backend.platform.devices[0].atomic_commit_sink = Rc::clone(&sink);

    let (width, height) = backend.platform.fb_dimensions();
    let (candidate, event, _) = prepare_direct_present_for_window(
        backend,
        present_id,
        window,
        width,
        height,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    );
    assert!(
        Backend::try_present_direct(backend, candidate, event)
            .expect("submit direct Async through the Backend Present entry")
    );
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(recorder.records()[0].kind, AtomicCommitKind::Async);
    let accepted_at = accepted_at
        .get()
        .expect("accepted ioctl records CLOCK_MONOTONIC time");
    let crtc = backend.platform.outputs[0].output.crtc;
    (sink, accepted_at, crtc)
}

fn deliver_clock_test_flip(
    backend: &mut KmsBackend,
    state: &mut yserver_core::server::ServerState,
    crtc: ::drm::control::crtc::Handle,
    frame: u32,
    kernel_ust: Duration,
    received_at_monotonic: Duration,
    received_at_boottime: Duration,
) {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    backend.platform.devices[0]
        .test_page_flip_event_source
        .as_ref()
        .expect("recording page-flip event source")
        .queue_flip_with_clock_samples(
            crtc,
            frame,
            kernel_ust,
            Some(received_at_monotonic),
            Some(received_at_boottime),
        );
    let drm_fd = backend.platform.devices[0].device.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(backend, state, drm_fd);
}

fn expected_clock_test_present_latency(
    sink: &AtomicCommitSink,
    crtc: ::drm::control::crtc::Handle,
    kernel_completion_at_monotonic: Duration,
) -> u64 {
    let present_received = sink
        .for_tests_pending_direct_present_received_at(crtc)
        .expect("the accepted direct Present is paired with its flip");
    u64::try_from(
        kernel_completion_at_monotonic
            .saturating_sub(present_received)
            .as_micros(),
    )
    .expect("Present-to-flip latency fits u64 microseconds")
}

fn finish_clock_test_direct_async(
    backend: &mut KmsBackend,
    recorder: &std::rc::Rc<RecordingAtomicCommitExecutor>,
    state: &mut yserver_core::server::ServerState,
    window: u32,
    next_present_id: u64,
) {
    let (width, height) = backend.platform.fb_dimensions();
    assert!(
        try_direct_present_for_window(backend, next_present_id, window, width, height, 0,)
            .is_some()
    );
    assert_tearing_off(backend);
    assert_eq!(
        backend.platform.tearing_copy_received_monotonic, None,
        "tearing exit clears deferred Copy receipt time"
    );
    assert_eq!(
        backend.pstate.tearing.deferred_cursor_wake, None,
        "tearing exit clears deferred cursor wake"
    );
    retire_direct_present(backend, state, 201);
    assert_direct_queue_drained(backend);
    assert!(!backend.scene.has_pending_page_flips());
    assert_tearing_off(backend);
    assert_commit_sequence(
        recorder,
        &[AtomicCommitKind::Async, AtomicCommitKind::Scene],
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: use event receipt for Present-to-flip latency, or kernel completion for turnaround.
fn delayed_flip_delivery_keeps_turnaround_and_present_latency_in_their_clock_domains() {
    use yserver_core::server::ServerState;

    const WINDOW: u32 = 0x7c89;
    let mut backend = live_scene_backend();
    let recorder = recording_sink(&mut backend, 0);
    qualify_test_direct_async(&mut backend, &recorder, WINDOW, 16_000);
    let mut state = ServerState::new();
    let (sink, accepted_at, crtc) =
        submit_direct_async_for_clock_test(&mut backend, recorder.clone(), WINDOW, 16_100);

    let kernel_completion_at = accepted_at + Duration::from_micros(150);
    let received_at_monotonic = accepted_at + Duration::from_millis(20);
    let received_at_boottime = received_at_monotonic + Duration::from_millis(1_300);
    let expected_present_latency =
        expected_clock_test_present_latency(&sink, crtc, kernel_completion_at);
    deliver_clock_test_flip(
        &mut backend,
        &mut state,
        crtc,
        95,
        kernel_completion_at,
        received_at_monotonic,
        received_at_boottime,
    );

    assert_eq!(sink.for_tests_direct_turnaround_samples(), [20_000]);
    assert_eq!(
        sink.for_tests_direct_present_to_flip_samples(),
        [expected_present_latency]
    );
    assert!(!backend.scene.has_pending_page_flips());
    finish_clock_test_direct_async(&mut backend, &recorder, &mut state, WINDOW, 16_101);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: subtract an unconverted BOOTTIME UST from the Present's MONOTONIC receipt.
fn boottime_flip_timestamp_is_converted_before_present_latency_is_sampled() {
    use yserver_core::server::ServerState;

    const WINDOW: u32 = 0x7c8a;
    let mut backend = live_scene_backend();
    let recorder = recording_sink(&mut backend, 0);
    qualify_test_direct_async(&mut backend, &recorder, WINDOW, 16_200);
    let mut state = ServerState::new();
    let (sink, accepted_at, crtc) =
        submit_direct_async_for_clock_test(&mut backend, recorder.clone(), WINDOW, 16_300);

    let suspend_offset = Duration::from_millis(1_300);
    let kernel_completion_at = accepted_at + Duration::from_micros(150);
    let received_at_monotonic = accepted_at + Duration::from_millis(20);
    let received_at_boottime = received_at_monotonic + suspend_offset;
    let kernel_ust_boottime = kernel_completion_at + suspend_offset;
    let expected_present_latency =
        expected_clock_test_present_latency(&sink, crtc, kernel_completion_at);
    deliver_clock_test_flip(
        &mut backend,
        &mut state,
        crtc,
        96,
        kernel_ust_boottime,
        received_at_monotonic,
        received_at_boottime,
    );

    assert_eq!(sink.for_tests_direct_turnaround_samples(), [20_000]);
    assert_eq!(
        sink.for_tests_direct_present_to_flip_samples(),
        [expected_present_latency]
    );
    assert!(!backend.scene.has_pending_page_flips());
    finish_clock_test_direct_async(&mut backend, &recorder, &mut state, WINDOW, 16_301);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: replace an implausible kernel timestamp with event receipt time instead of reporting n/a.
fn implausible_flip_timestamp_is_reported_as_unavailable() {
    use yserver_core::server::ServerState;

    const WINDOW: u32 = 0x7c8b;
    let mut backend = live_scene_backend();
    let recorder = recording_sink(&mut backend, 0);
    qualify_test_direct_async(&mut backend, &recorder, WINDOW, 16_400);
    let mut state = ServerState::new();
    let (sink, accepted_at, crtc) =
        submit_direct_async_for_clock_test(&mut backend, recorder.clone(), WINDOW, 16_500);

    let received_at_monotonic = accepted_at + Duration::from_millis(20);
    let received_at_boottime = received_at_monotonic + Duration::from_millis(1_300);
    deliver_clock_test_flip(
        &mut backend,
        &mut state,
        crtc,
        97,
        Duration::ZERO,
        received_at_monotonic,
        received_at_boottime,
    );

    assert_eq!(sink.for_tests_direct_turnaround_samples(), [20_000]);
    assert!(sink.for_tests_direct_present_to_flip_samples().is_empty());
    assert_eq!(
        sink.for_tests_direct_present_to_flip_summary(),
        ("n/a".to_owned(), "n/a".to_owned(), "n/a".to_owned())
    );
    assert!(!backend.scene.has_pending_page_flips());
    finish_clock_test_direct_async(&mut backend, &recorder, &mut state, WINDOW, 16_501);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: keep a per-CRTC accepted-commit telemetry entry after discarding its old DRM event.
fn discarded_vt_flip_events_do_not_shift_the_next_async_turnaround_match() {
    use std::{
        cell::Cell,
        os::fd::{AsFd as _, AsRawFd as _},
        rc::Rc,
        time::Duration,
    };
    use yserver_core::{backend::Backend, core_loop::input_inventory::InputInventory};

    struct LogicalVtResume;

    impl crate::kms::render::backend::kms::session::VtResumeExecutor for LogicalVtResume {
        fn acquire_master(
            &self,
            _device: &crate::kms::render::platform::KmsDevice,
        ) -> std::io::Result<()> {
            Ok(())
        }

        fn release_master(
            &self,
            _device: &crate::kms::render::platform::KmsDevice,
        ) -> std::io::Result<()> {
            Ok(())
        }

        fn probe_connector_snapshot(
            &self,
            platform: &crate::kms::render::platform::PlatformBackend,
        ) -> std::io::Result<Vec<crate::kms::render::platform::ConnectorSnapshot>> {
            Ok(platform
                .outputs
                .iter()
                .map(|output| crate::kms::render::platform::ConnectorSnapshot {
                    key: output.key.clone(),
                    modes: vec![output.output.picked.clone()],
                    mm_width: 0,
                    mm_height: 0,
                    edid: Vec::new(),
                    connector_type: "test".to_owned(),
                })
                .collect())
        }
    }

    const WINDOW: u32 = 0x7d90;
    let (mut backend, recorder, mut state) = live_tearing_cursor_backend(WINDOW, 16_100);
    backend.for_tests_set_vt_resume_executor(Rc::new(LogicalVtResume));
    backend.platform.for_tests_accept_output_power_commits();

    let accepted_at = Rc::new(Cell::new(Duration::from_millis(100)));
    let clock = Rc::clone(&accepted_at);
    let sink = Rc::new(AtomicCommitSink::new_with_monotonic_clock(
        recorder.clone(),
        Rc::new(move || Some(clock.get())),
    ));
    backend.platform.devices[0].atomic_commit_sink = Rc::clone(&sink);
    sink.set_tearing_on(true);
    backend.pstate.tearing.last_cursor_compose = None;
    Backend::warp_pointer_root(&mut backend, &mut state, 300, 180);
    Backend::maybe_composite(&mut backend).expect("submit first composed Async commit");
    assert_commit_sequence(&recorder, &[AtomicCommitKind::Async]);
    assert!(backend.scene.has_pending_page_flips());

    let crtc = backend.platform.outputs[0].output.crtc;
    let event_source = Rc::clone(
        backend.platform.devices[0]
            .test_page_flip_event_source
            .as_ref()
            .expect("recording page-flip source"),
    );
    event_source.queue_flip_at(crtc, 92, Duration::from_millis(100));
    Backend::finish_vt_release(&mut backend, &mut state, &InputInventory::default());
    assert_tearing_off(&backend);
    assert_eq!(
        event_source.queued_flip_count(),
        0,
        "VT discard consumed the old event"
    );
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.scanout_allowed());

    Backend::on_vt_acquire(&mut backend, &mut state);
    assert_tearing_off(&backend);
    assert_eq!(backend.pstate.vt_state, crate::vt::state::VtState::Active);
    assert!(backend.scanout_allowed());
    damage_root_and_composite(&mut backend);
    assert!(backend.scene.has_pending_page_flips());
    retire_composed_frame(&mut backend, &mut state, 93);

    state = enter_tearing_without_direct_scanout(&mut backend, WINDOW, 16_200);
    assert!(backend.pstate.tearing.is_on());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    recorder.clear_records();
    accepted_at.set(Duration::from_millis(200));
    backend.pstate.tearing.last_cursor_compose = None;
    Backend::warp_pointer_root(&mut backend, &mut state, 340, 220);
    Backend::maybe_composite(&mut backend).expect("submit resumed composed Async commit");
    assert_commit_sequence(&recorder, &[AtomicCommitKind::Async]);
    assert!(backend.scene.has_pending_page_flips());

    let event_at = accepted_at.get() + Duration::from_micros(175);
    event_source.queue_flip_at(crtc, 94, event_at);
    let drm_fd = backend.platform.devices[0].device.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(&mut backend, &mut state, drm_fd);

    assert_eq!(sink.for_tests_composed_turnaround_samples(), [175]);
    assert_commit_sequence(&recorder, &[AtomicCommitKind::Async]);
    assert!(backend.pstate.tearing.is_on());
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.scene.scene_structure_dirty);
    assert_eq!(event_source.queued_flip_count(), 0);
    assert!(backend.scanout_allowed());
    assert!(backend.kms_outputs_active);
}
