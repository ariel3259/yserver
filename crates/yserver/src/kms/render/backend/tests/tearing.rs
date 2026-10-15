use super::*;

fn clone_test_drm_output(output: &crate::platform::drm::Output) -> crate::platform::drm::Output {
    crate::platform::drm::Output {
        connector: output.connector,
        connector_name: output.connector_name.clone(),
        encoder: output.encoder,
        crtc: output.crtc,
        plane: output.plane,
        mode: output.mode,
        picked: output.picked.clone(),
        plane_fb_id_prop: output.plane_fb_id_prop,
        plane_crtc_id_prop: output.plane_crtc_id_prop,
        plane_src_x_prop: output.plane_src_x_prop,
        plane_src_y_prop: output.plane_src_y_prop,
        plane_src_w_prop: output.plane_src_w_prop,
        plane_src_h_prop: output.plane_src_h_prop,
        plane_crtc_x_prop: output.plane_crtc_x_prop,
        plane_crtc_y_prop: output.plane_crtc_y_prop,
        plane_crtc_w_prop: output.plane_crtc_w_prop,
        plane_crtc_h_prop: output.plane_crtc_h_prop,
        plane_in_fence_fd_prop: output.plane_in_fence_fd_prop,
        crtc_out_fence_ptr_prop: output.crtc_out_fence_ptr_prop,
        scanout_modifiers: output.scanout_modifiers.clone(),
        mm_width: output.mm_width,
        mm_height: output.mm_height,
        edid: output.edid.clone(),
        connector_type: output.connector_type.clone(),
        modes: output.modes.clone(),
    }
}

fn pollable_test_device() -> (
    std::rc::Rc<crate::drm::Device>,
    std::os::unix::net::UnixStream,
) {
    let (device_fd, peer) = std::os::unix::net::UnixStream::pair().expect("test socket pair");
    let device = crate::drm::Device::from_inherited_kms_fd(
        std::os::fd::OwnedFd::from(device_fd),
        "<pollable test fd>",
    );
    (std::rc::Rc::new(device), peer)
}

fn query_capabilities_in_core(
    state: &mut yserver_core::server::ServerState,
    backend: &mut KmsBackend,
    targets: &[u32],
) -> Vec<u32> {
    use std::{
        collections::{HashMap, HashSet, VecDeque},
        io::Read,
        sync::{Arc, Mutex, atomic::AtomicU16},
        time::Duration,
    };
    use yserver_core::{
        core_loop::{
            CoreSender, Message, ResetPolicy, auth::AuthState, channel,
            poll_tokens::ClientIdAllocator, run_core,
        },
        server::ClientState,
        transport::Transport,
    };
    use yserver_protocol::x11::{
        ClientByteOrder, ClientId, RequestHeader, SequenceNumber, present as x11present,
    };

    let client_id = ClientId(1);
    let (transport, mut peer) = Transport::pair().expect("client socket pair");
    peer.set_read_timeout(Some(Duration::from_secs(3)))
        .expect("reply timeout");
    state.clients.insert(
        client_id.0,
        ClientState {
            writer: Arc::new(Mutex::new(transport)),
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
            reader_control: None,
            is_local: true,
            fd_passing: true,
        },
    );

    let (poll, sender, receiver) = channel().expect("core channel");
    let request_sender = sender.bind();
    let shutdown_sender = sender.clone_handle();
    let targets = targets.to_vec();
    struct ShutdownOnDrop(CoreSender);
    impl Drop for ShutdownOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(Message::Shutdown);
        }
    }
    let driver = std::thread::spawn(move || {
        let _shutdown_on_drop = ShutdownOnDrop(shutdown_sender);
        let mut replies = Vec::with_capacity(targets.len());
        for (index, target) in targets.into_iter().enumerate() {
            request_sender
                .send(Message::Request {
                    id: client_id,
                    sequence: SequenceNumber(u16::try_from(index + 1).expect("sequence fits")),
                    accepted_at: None,
                    header: RequestHeader {
                        opcode: 145,
                        data: x11present::QUERY_CAPABILITIES,
                        length_units: 2,
                    },
                    body: target.to_le_bytes().to_vec(),
                    attached_fd: None,
                })
                .expect("queue Present::QueryCapabilities");

            let mut reply = [0; 32];
            peer.read_exact(&mut reply)
                .expect("read Present::QueryCapabilities reply");
            assert_eq!(reply[0], 1, "reply marker");
            replies.push(u32::from_le_bytes(reply[8..12].try_into().unwrap()));
        }
        replies
    });

    run_core(
        poll,
        receiver,
        sender.clone_handle(),
        state,
        backend,
        [],
        &ClientIdAllocator::new(),
        AuthState::new(None),
        ResetPolicy::NoReset,
        None,
    )
    .expect("run core through Present::QueryCapabilities and shutdown");
    driver.join().expect("query driver thread")
}

#[test]
fn present_capabilities_follow_the_windows_output_device() {
    use std::rc::Rc;
    use yserver_core::{
        backend::Backend,
        resources::{ROOT_VISUAL, ROOT_WINDOW},
        server::{BackendCapabilities, ServerState},
    };
    use yserver_protocol::x11::{ClientId, CreateWindowRequest, ResourceId};

    // Mutations killed: route every CRTC through the primary KMS device, or
    // hard-code `async_flip: true`; the latter also makes target 0 advertise Async.
    let mut backend = KmsBackend::for_tests();
    let (device_a, _device_a_peer) = pollable_test_device();
    backend.platform.devices[0].device = device_a;
    let device_a_key = backend.platform.devices[0].key;
    let device_b_key = crate::platform::drm::DrmDeviceKey {
        major: 226,
        minor: 99,
    };
    let commits_a =
        Rc::new(crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default());
    backend.platform.devices[0].atomic_commit_sink =
        Rc::new(crate::drm::atomic::AtomicCommitSink::new(commits_a.clone()));
    let (device_b, _device_b_peer) = pollable_test_device();
    let commits_b =
        Rc::new(crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default());
    backend
        .platform
        .devices
        .push(crate::kms::render::platform::KmsDevice {
            key: device_b_key,
            device: device_b,
            atomic_commit_sink: Rc::new(crate::drm::atomic::AtomicCommitSink::new(
                commits_b.clone(),
            )),
            page_flip_event_source: Rc::new(
                crate::kms::render::platform::flip_events::DrmPageFlipEventSource,
            ),
            #[cfg(test)]
            test_page_flip_event_source: None,
            cursor: crate::kms::render::platform::KmsCursorState::new(),
            async_page_flip: false,
        });

    backend.platform.devices[0].async_page_flip = true;
    let mut output_b = clone_test_drm_output(&backend.platform.outputs[0].output);
    output_b.connector_name = "test-b".to_owned();
    let route_b = crate::kms::scanout_route::ScanoutRoute::new(
        crate::kms::render::platform::RenderDeviceId::UnverifiedFallback,
        device_b_key,
        crate::kms::scanout_route::RenderKmsRelationship::Unknown,
    );
    backend
        .platform
        .outputs
        .push(crate::kms::backend::ActiveOutput::new(
            route_b,
            output_b,
            crate::drm::Swapchain::empty_for_tests(),
            800,
            0,
        ));

    let (outputs, modes) = Backend::randr_outputs_and_modes(&mut backend);
    let (width, height) = Backend::fb_dimensions(&backend);
    let mut state = ServerState::with_randr_outputs_and_modes(
        width,
        height,
        outputs,
        modes,
        BackendCapabilities::from_backend(&backend),
    );
    let window_a = ResourceId(0x0100_0001);
    let window_b = ResourceId(0x0100_0002);
    for (window, x) in [(window_a, 10), (window_b, 810)] {
        state.resources.create_window(
            ClientId(1),
            CreateWindowRequest {
                depth: 24,
                window,
                parent: ROOT_WINDOW,
                x,
                y: 10,
                width: 100,
                height: 100,
                class: 1,
                visual: ROOT_VISUAL,
                ..Default::default()
            },
        );
    }
    let crtc_a = state.randr.outputs[0].crtc_id;
    let crtc_b = state.randr.outputs[1].crtc_id;
    assert_ne!(device_a_key, device_b_key);
    assert_ne!(crtc_a, crtc_b);

    let first = query_capabilities_in_core(
        &mut state,
        &mut backend,
        &[window_a.0, window_b.0, crtc_a, crtc_b, 0],
    );
    assert_eq!(
        first.iter().map(|caps| caps & 0x1 != 0).collect::<Vec<_>>(),
        [true, false, true, false, false],
        "windows resolve to their output device and CRTC targets resolve directly",
    );
    assert!(first.iter().all(|caps| caps & 0x8 == 0));

    backend.platform.devices[0].async_page_flip = false;
    backend.platform.devices[1].async_page_flip = true;
    let swapped = query_capabilities_in_core(
        &mut state,
        &mut backend,
        &[window_a.0, window_b.0, crtc_a, crtc_b, 0],
    );
    assert_eq!(
        swapped
            .iter()
            .map(|caps| caps & 0x1 != 0)
            .collect::<Vec<_>>(),
        [false, true, false, true, false],
        "advertised Async follows the owning device when flags swap",
    );
    assert!(swapped.iter().all(|caps| caps & 0x8 == 0));

    assert!(state.present_pending_msc.is_empty());
    assert!(state.present_window_msc.is_empty());
    assert!(backend.pending_present_batches.is_empty());
    assert!(backend.pending_present_source_waits.is_empty());
    assert!(backend.retained_present_wakes.is_empty());
    assert!(backend.pending_completed_events_on_shutdown.is_empty());
    assert_eq!(backend.vt_state, crate::vt::state::VtState::Active);
    assert!(commits_a.records().is_empty());
    assert!(commits_b.records().is_empty());
}
