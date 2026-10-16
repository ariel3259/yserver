use super::support::AtomicCursorFixture;

#[test]
// Mutation: rejecting valid sub-64 cursor caps or accepting zero/overflow query results breaks these allocations.
fn atomic_cursor_only_cursor_cap_dimensions_use_valid_values_and_per_axis_fallbacks() {
    let rows = [
        (Some(32), Some(24), 32, 24),
        (None, Some(24), 64, 24),
        (Some(0), Some(u64::MAX), 64, 64),
    ];
    for (cursor_width, cursor_height, expected_width, expected_height) in rows {
        let fixture = AtomicCursorFixture::without_scene_with_cursor_capabilities(
            cursor_width,
            cursor_height,
        )
        .expect("build recording Backend cursor fixture");
        let records = fixture.recorders.cursor_buffer_ledger.records();
        let creates: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                crate::kms::cursor_plane::ops::CursorBufferRecord::Create {
                    width, height, ..
                } => Some((*width, *height)),
                _ => None,
            })
            .collect();
        assert_eq!(creates, vec![(expected_width, expected_height)]);
        let framebuffers: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                crate::kms::cursor_plane::ops::CursorBufferRecord::AddFb {
                    width,
                    height,
                    format,
                    ..
                } => Some((*width, *height, *format)),
                _ => None,
            })
            .collect();
        assert_eq!(
            framebuffers,
            vec![(
                expected_width,
                expected_height,
                u32::from_be_bytes(*b"AR24")
            )],
        );
        fixture.assert_cursor_quiescent();
        let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: publish before AddFB2, omit AddFB2-failure unmap, omit map-failure destroy, or retain partial first allocations.
fn atomic_cursor_only_failed_sprite_allocation_preserves_prior_image() {
    use crate::kms::cursor_plane::ops::CursorBufferOperation;
    use yserver_core::backend::{AnyHandle, Backend};

    let cases = [
        (CursorBufferOperation::AddFb, 0, "addfb"),
        (CursorBufferOperation::Create, 0, "create"),
        (CursorBufferOperation::Map, 0, "map"),
        (CursorBufferOperation::Create, 1, "second-initial-create"),
    ];
    for (operation, successes_before_failure, label) in cases {
        let Some(mut fixture) =
            AtomicCursorFixture::new_with_cursor_capabilities(Some(32), Some(24))
                .expect("build allocation-failure cursor fixture")
        else {
            return;
        };
        let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        let initial_records = ledger.records().len();
        ledger.fail_after(operation, successes_before_failure, libc::ENOMEM);

        let source = Backend::create_pixmap(&mut fixture.backend, None, 32, 1, 1)
            .expect("create depth-32 cursor pixmap through Backend");
        let pixels = [0x41, 0x52, 0x63, 0xff];
        Backend::put_image(
            &mut fixture.backend,
            None,
            source.as_raw(),
            32,
            1,
            1,
            0,
            0,
            &pixels,
        )
        .expect("upload complete cursor source through Backend");
        let picture = Backend::render_create_picture(
            &mut fixture.backend,
            None,
            AnyHandle::Pixmap(source),
            0,
            0,
            &[],
        )
        .expect("create source picture through Backend")
        .expect("picture handle");
        let cursor = Backend::render_create_cursor(&mut fixture.backend, None, picture, 0, 0)
            .expect("create ARGB cursor through Backend")
            .expect("cursor handle");
        let root = fixture.backend.core.window_id;
        Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
            .expect("define cursor through Backend");
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 80, 90);
        let root = fixture.backend.core.window_id;
        let root_width = fixture.backend.platform.fb_w;
        let root_height = fixture.backend.platform.fb_h;
        Backend::clear_area(
            &mut fixture.backend,
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
        .expect("damage root through Backend");
        Backend::maybe_composite(&mut fixture.backend)
            .expect("submit cursor frame through Backend");
        fixture.retire_output(0, 1);

        let version = fixture.backend.cursor_records[&cursor.as_raw()].version;
        let plane = fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("initialized cursor plane");
        let pool_versions = plane.for_tests_sprite_pool_versions();
        assert_eq!(
            pool_versions.0,
            Some(version),
            "{label}: wanted image is retained"
        );
        assert_eq!(
            fixture
                .backend
                .platform
                .cursor_plane_uploaded_version_for_output(0),
            None,
            "{label}: Deferred is not a successful upload",
        );
        assert!(
            fixture.recorders.legacy_cursor.records().is_empty(),
            "{label}: Deferred does not bind or replace the existing image",
        );
        assert!(
            !fixture.backend.platform.cursor_plane_visible_for_output(0),
            "{label}: Deferred leaves the current hardware binding unchanged",
        );
        assert_eq!(
            ledger.end_state(),
            (1, 1, 1),
            "{label}: only the pre-existing legacy backing remains live",
        );
        let rollback: Vec<_> = ledger.records()[initial_records..].to_vec();
        match label {
            "create" => assert!(rollback.is_empty(), "failed create acquires nothing"),
            "map" => assert!(matches!(
                rollback.as_slice(),
                [
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Create { .. },
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Destroy { .. }
                ]
            )),
            "addfb" => assert!(matches!(
                rollback.as_slice(),
                [
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Create { .. },
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Map { .. },
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Unmap { .. },
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Destroy { .. }
                ]
            )),
            "second-initial-create" => assert!(matches!(
                rollback.as_slice(),
                [
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Create { .. },
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Map { .. },
                    crate::kms::cursor_plane::ops::CursorBufferRecord::AddFb { .. },
                    crate::kms::cursor_plane::ops::CursorBufferRecord::RemoveFb { .. },
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Unmap { .. },
                    crate::kms::cursor_plane::ops::CursorBufferRecord::Destroy { .. }
                ]
            )),
            _ => unreachable!("known allocation failure case"),
        }
        assert_eq!(
            pool_versions,
            (Some(version), None),
            "{label}: wanted image remains retryable while no candidate is published",
        );

        Backend::maybe_composite(&mut fixture.backend)
            .expect("Backend scheduler retries deferred cursor staging");
        assert!(
            fixture.backend.scene.has_pending_page_flips(),
            "{label}: retry is carried by a real Backend scene frame",
        );
        fixture.retire_output(0, 2);
        assert_eq!(
            fixture
                .backend
                .platform
                .cursor_plane_uploaded_version_for_output(0),
            Some(version),
            "{label}: retry stages and uploads the retained wanted record",
        );
        assert_eq!(
            fixture.backend.platform.devices[0]
                .cursor
                .plane
                .as_ref()
                .expect("cursor plane")
                .for_tests_sprite_pool_versions(),
            (Some(version), Some(version)),
        );

        Backend::set_cursor_hidden(&mut fixture.backend, true);
        Backend::clear_area(
            &mut fixture.backend,
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
        .expect("damage root for Backend cursor hide");
        Backend::maybe_composite(&mut fixture.backend).expect("submit cursor hide frame");
        fixture.retire_output(0, 3);
        fixture.assert_cursor_quiescent();
        assert_eq!(
            fixture.backend.platform.devices[0]
                .cursor
                .plane
                .as_ref()
                .expect("cursor plane")
                .for_tests_sprite_pool_versions(),
            (Some(version), None),
            "{label}: hide cancels the staged candidate lease",
        );
        assert_eq!(
            fixture
                .recorders
                .atomic
                .records()
                .into_iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            vec![
                crate::drm::atomic::AtomicCommitKind::Scene,
                crate::drm::atomic::AtomicCommitKind::Scene,
                crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            ],
            "{label}: accepted order is original scene, successful retry, then hide",
        );
        let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(
            resources.end_state(),
            (0, 0, 0),
            "{label}: teardown releases every resource"
        );
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omitting the per-device full-size sprite pool leaves fewer than two staged allocations.
fn atomic_cursor_only_full_buffer_argb_upload_and_cleanup() {
    use yserver_core::backend::{AnyHandle, Backend};

    let Some(mut fixture) = AtomicCursorFixture::new_with_cursor_capabilities(Some(32), Some(24))
        .expect("build cursor fixture with sub-64 driver caps")
    else {
        return;
    };
    let initial_creates = fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .iter()
        .filter(|record| {
            matches!(
                record,
                crate::kms::cursor_plane::ops::CursorBufferRecord::Create { .. }
            )
        })
        .count();
    assert_eq!(
        initial_creates, 1,
        "the legacy full-size cursor backing is initialized once"
    );

    let root = fixture.backend.core.window_id;
    let create_argb_cursor =
        |fixture: &mut AtomicCursorFixture, width: u16, height: u16, bytes: &[u8]| {
            let source = Backend::create_pixmap(&mut fixture.backend, None, 32, width, height)
                .expect("create depth-32 cursor pixmap through Backend");
            Backend::put_image(
                &mut fixture.backend,
                None,
                source.as_raw(),
                32,
                width,
                height,
                0,
                0,
                bytes,
            )
            .expect("upload exact ARGB source pixels through Backend");
            let picture = Backend::render_create_picture(
                &mut fixture.backend,
                None,
                AnyHandle::Pixmap(source),
                0,
                0,
                &[],
            )
            .expect("create source picture through Backend")
            .expect("picture handle");
            let cursor = Backend::render_create_cursor(&mut fixture.backend, None, picture, 0, 0)
                .expect("create ARGB cursor through Backend")
                .expect("ARGB cursor handle");
            Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
                .expect("define ARGB cursor through Backend");
            cursor.as_raw()
        };
    let first_pixels = [
        0x11, 0x22, 0x33, 0xff, 0x44, 0x55, 0x66, 0xff, 0x77, 0x88, 0x99, 0xff, 0xaa, 0xbb, 0xcc,
        0xff,
    ];
    let first_cursor = create_argb_cursor(&mut fixture, 2, 2, &first_pixels);
    assert_eq!(
        fixture.backend.cursor_records[&first_cursor].bgra_bytes,
        first_pixels
    );
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    let width = fixture.backend.platform.fb_w;
    let height = fixture.backend.platform.fb_h;
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
    .expect("damage root through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor image frame");
    fixture.retire_output(0, 1);

    let first_create = fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .into_iter()
        .filter_map(|record| match record {
            crate::kms::cursor_plane::ops::CursorBufferRecord::Create {
                device_fd, handle, ..
            } if handle != 1 => Some((device_fd, handle)),
            _ => None,
        })
        .next()
        .expect("first full-size sprite allocation");
    let first_storage = fixture
        .recorders
        .cursor_buffer_ledger
        .mapped_bytes(first_create.0, first_create.1)
        .expect("first sprite mapping remains owned by its pool");
    assert_eq!(&first_storage[..8], &first_pixels[..8]);
    assert!(first_storage[8..256].iter().all(|byte| *byte == 0));
    assert_eq!(&first_storage[256..264], &first_pixels[8..]);
    assert!(first_storage[264..].iter().all(|byte| *byte == 0));

    let smaller_pixels = [0xde, 0xad, 0xbe, 0xff];
    let smaller_cursor = create_argb_cursor(&mut fixture, 1, 1, &smaller_pixels);
    assert_eq!(
        fixture.backend.cursor_records[&smaller_cursor].bgra_bytes, smaller_pixels,
        "well-formed ARGB readback keeps exactly width*height*4 bytes",
    );
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
    .expect("damage root for second cursor image through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit smaller cursor image frame");
    fixture.retire_output(0, 2);
    let sprite_creates: Vec<_> = fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .into_iter()
        .filter_map(|record| match record {
            crate::kms::cursor_plane::ops::CursorBufferRecord::Create {
                device_fd,
                handle,
                width,
                height,
            } if handle != 1 => Some((device_fd, handle, width, height)),
            _ => None,
        })
        .collect();
    assert_eq!(
        sprite_creates,
        vec![
            (first_create.0, first_create.1, 32, 24),
            (first_create.0, first_create.1 + 1, 32, 24),
        ],
        "reported sub-64 dimensions size complete candidate buffers",
    );
    let add_fb_records: Vec<_> = fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .into_iter()
        .filter_map(|record| match record {
            crate::kms::cursor_plane::ops::CursorBufferRecord::AddFb {
                device_fd,
                handle,
                width,
                height,
                format,
                ..
            } if handle != 1 => Some((device_fd, handle, width, height, format)),
            _ => None,
        })
        .collect();
    assert_eq!(add_fb_records.len(), 2);
    assert!(add_fb_records.iter().all(|record| {
        (record.0, record.2, record.3, record.4)
            == (first_create.0, 32, 24, u32::from_be_bytes(*b"AR24"))
    }));
    let smaller_storage = fixture
        .recorders
        .cursor_buffer_ledger
        .mapped_bytes(first_create.0, sprite_creates[1].1)
        .expect("second full-size sprite mapping remains live");
    assert_eq!(&smaller_storage[..4], &smaller_pixels);
    assert!(smaller_storage[4..].iter().all(|byte| *byte == 0));

    let reused_pixels = [0x12, 0x34, 0x56, 0xff];
    let reused_cursor = create_argb_cursor(&mut fixture, 1, 1, &reused_pixels);
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
    .expect("damage root for reused cursor slot through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit reused cursor slot frame");
    fixture.retire_output(0, 3);
    let reused_storage = fixture
        .recorders
        .cursor_buffer_ledger
        .mapped_bytes(first_create.0, sprite_creates[0].1)
        .expect("reused sprite mapping remains live");
    assert_eq!(&reused_storage[..4], &reused_pixels);
    assert!(
        reused_storage[4..].iter().all(|byte| *byte == 0),
        "smaller replacement clears pixels from the previously staged larger sprite",
    );

    let creates = fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .iter()
        .filter(|record| {
            matches!(
                record,
                crate::kms::cursor_plane::ops::CursorBufferRecord::Create { .. }
            )
        })
        .count();
    assert_eq!(
        creates - initial_creates,
        2,
        "pool starts with two framebuffers per device"
    );
    assert_eq!(
        fixture.recorders.atomic.records()[0].kind,
        crate::drm::atomic::AtomicCommitKind::Scene,
        "the upload is reached through the accepted Backend scene frame",
    );

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
    .expect("damage root for cursor hide through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor hide frame");
    fixture.retire_output(0, 4);
    for frame in 1..=32 {
        if !fixture.backend.scene.has_pending_page_flips() {
            break;
        }
        fixture.retire_output(0, frame);
    }
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .for_tests_sprite_pool_versions(),
        (
            Some(fixture.backend.cursor_records[&reused_cursor].version),
            None
        ),
        "hide cancels the final staged lease after all output obligations retire",
    );
    let atomic_kinds: Vec<_> = fixture
        .recorders
        .atomic
        .records()
        .into_iter()
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        atomic_kinds,
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "the accepted frame sequence is initial scene then cursor hide",
    );

    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: replace the short-ZPixmap silent return in drawing.rs with BadLength; this must expose that exact X11 error.
fn atomic_cursor_only_malformed_cursor_source_stops_at_request_validation() {
    use std::{
        collections::{HashMap, HashSet, VecDeque},
        io::{Read, Write},
        os::unix::net::UnixStream,
        sync::{Arc, Mutex, atomic::AtomicU16},
        time::Duration,
    };
    use yserver_core::{
        backend::{AnyHandle, Backend},
        core_loop::{
            CoreSender, Message, ResetPolicy, auth::AuthState, channel, client_reader,
            poll_tokens::ClientIdAllocator, run_core,
        },
        server::ClientState,
        transport::Transport,
    };
    use yserver_protocol::x11::{ClientByteOrder, ClientId, ResourceId};

    fn wire_request(opcode: u8, data: u8, body: &[u8]) -> Vec<u8> {
        let byte_len = body.len() + 4;
        assert_eq!(byte_len % 4, 0, "X11 request is four-byte aligned");
        let length_units = u16::try_from(byte_len / 4).expect("request length fits");
        let mut wire = vec![opcode, data];
        wire.extend_from_slice(&length_units.to_le_bytes());
        wire.extend_from_slice(body);
        assert_eq!(wire.len(), usize::from(length_units) * 4);
        wire
    }

    fn read_response(peer: &mut UnixStream) -> Vec<u8> {
        let mut packet = vec![0; 32];
        peer.read_exact(&mut packet).expect("read X11 response");
        if packet[0] == 1 {
            let extra_words = u32::from_le_bytes(packet[4..8].try_into().unwrap());
            let extra_bytes = usize::try_from(extra_words).expect("reply length fits") * 4;
            let start = packet.len();
            packet.resize(start + extra_bytes, 0);
            peer.read_exact(&mut packet[start..])
                .expect("read variable GetImage reply data");
        }
        packet
    }

    fn put_image_body(drawable: u32, gc: u32, width: u16, height: u16, pixels: &[u8]) -> Vec<u8> {
        let mut body = Vec::with_capacity(20 + pixels.len());
        body.extend_from_slice(&drawable.to_le_bytes());
        body.extend_from_slice(&gc.to_le_bytes());
        body.extend_from_slice(&width.to_le_bytes());
        body.extend_from_slice(&height.to_le_bytes());
        body.extend_from_slice(&0_i16.to_le_bytes());
        body.extend_from_slice(&0_i16.to_le_bytes());
        body.extend_from_slice(&[0, 32, 0, 0]);
        body.extend_from_slice(pixels);
        body
    }

    let mut backend = crate::kms::render::backend::KmsBackend::for_tests_with_vk()
        .expect("build lavapipe backend for core-loop request validation");
    if !backend
        .platform
        .gpu
        .vk
        .as_ref()
        .is_some_and(|vk| vk.supports_sync_fd())
    {
        eprintln!("skipping: Vulkan device lacks SYNC_FD semaphore export; cursor test needs it");
        return;
    }
    backend
        .platform
        .for_tests_initialize_cursor_planes_with_capabilities(Some(32), Some(24));
    let recorders = std::rc::Rc::clone(
        backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("test backend owns recording services"),
    );
    let (drm_fd, _drm_peer) = UnixStream::pair().expect("pollable synthetic DRM fd");
    let pollable_device = std::rc::Rc::new(crate::drm::Device::from_inherited_kms_fd(
        std::os::fd::OwnedFd::from(drm_fd),
        "<atomic cursor core-loop test fd>",
    ));
    recorders.page_flip_events.register_device(&pollable_device);
    backend.platform.devices[0].device = pollable_device;
    let mut fixture = AtomicCursorFixture {
        backend,
        recorders,
        state: yserver_core::server::ServerState::new(),
    };
    let client_id = ClientId(1);
    let pixmap_id = ResourceId(0x1001);
    let gc_id = ResourceId(0x1002);
    let pool_records_before = fixture.recorders.cursor_buffer_ledger.records();
    let legacy_record = pool_records_before
        .iter()
        .find_map(|record| match record {
            crate::kms::cursor_plane::ops::CursorBufferRecord::Create {
                device_fd, handle, ..
            } => Some((*device_fd, *handle)),
            _ => None,
        })
        .expect("legacy cursor backing exists");
    let legacy_pixels_before = fixture
        .recorders
        .cursor_buffer_ledger
        .mapped_bytes(legacy_record.0, legacy_record.1)
        .expect("legacy cursor mapping is live");
    assert_eq!(fixture.backend.effective_cursor_xid, None);

    let (transport, mut peer) = Transport::pair().expect("client socket pair");
    let writer = transport
        .try_clone()
        .expect("clone client transport for core writer");
    peer.set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set reply timeout");
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

    let (poll, core_sender, receiver) = channel().expect("core channel");
    client_reader::spawn(
        client_id,
        transport,
        ClientByteOrder::LittleEndian,
        0,
        reader_control_rx,
        core_sender.bind(),
    )
    .expect("spawn reader for the synthetic X11 client");
    let shutdown = core_sender.clone_handle();
    let root = yserver_core::resources::ROOT_WINDOW;
    let known_pixels: Vec<u8> = [
        0x11, 0x22, 0x33, 0xff, 0x44, 0x55, 0x66, 0xff, 0x77, 0x88, 0x99, 0xff, 0xaa, 0xbb, 0xcc,
        0xff,
    ]
    .into_iter()
    .collect();
    let short_pixels: Vec<u8> = [
        0xe1, 0xe2, 0xe3, 0xff, 0xd1, 0xd2, 0xd3, 0xff, 0xc1, 0xc2, 0xc3, 0xff,
    ]
    .into_iter()
    .collect();
    let control_pixels = known_pixels.clone();
    let driver = std::thread::spawn(move || {
        struct ShutdownOnDrop(CoreSender);
        impl Drop for ShutdownOnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(Message::Shutdown);
            }
        }
        let _shutdown_on_drop = ShutdownOnDrop(shutdown);

        let mut wire_requests = Vec::new();
        let mut create_pixmap = Vec::new();
        create_pixmap.extend_from_slice(&pixmap_id.0.to_le_bytes());
        create_pixmap.extend_from_slice(&root.0.to_le_bytes());
        create_pixmap.extend_from_slice(&2_u16.to_le_bytes());
        create_pixmap.extend_from_slice(&2_u16.to_le_bytes());
        wire_requests.extend_from_slice(&wire_request(53, 32, &create_pixmap));

        let mut create_gc = Vec::new();
        create_gc.extend_from_slice(&gc_id.0.to_le_bytes());
        create_gc.extend_from_slice(&pixmap_id.0.to_le_bytes());
        create_gc.extend_from_slice(&0_u32.to_le_bytes());
        wire_requests.extend_from_slice(&wire_request(55, 0, &create_gc));

        let valid = wire_request(
            72,
            2,
            &put_image_body(pixmap_id.0, gc_id.0, 2, 2, &control_pixels),
        );
        assert_eq!(valid.len(), 40, "valid 2x2 ZPixmap is 10 request units");
        wire_requests.extend_from_slice(&valid);

        let mut get_image = Vec::new();
        get_image.extend_from_slice(&pixmap_id.0.to_le_bytes());
        get_image.extend_from_slice(&0_i16.to_le_bytes());
        get_image.extend_from_slice(&0_i16.to_le_bytes());
        get_image.extend_from_slice(&2_u16.to_le_bytes());
        get_image.extend_from_slice(&2_u16.to_le_bytes());
        get_image.extend_from_slice(&u32::MAX.to_le_bytes());
        wire_requests.extend_from_slice(&wire_request(73, 2, &get_image));

        let short = wire_request(
            72,
            2,
            &put_image_body(pixmap_id.0, gc_id.0, 2, 2, &short_pixels),
        );
        assert_eq!(short.len(), 36, "short PutImage is exactly 9 request units");
        assert_eq!(
            short.len(),
            4 + 20 + 12,
            "header, fixed body, and payload match"
        );
        wire_requests.extend_from_slice(&short);

        let zero_size = wire_request(72, 2, &put_image_body(pixmap_id.0, gc_id.0, 0, 2, &[]));
        assert_eq!(zero_size.len(), 24, "zero-size PutImage is 6 request units");
        wire_requests.extend_from_slice(&zero_size);
        wire_requests.extend_from_slice(&wire_request(73, 2, &get_image));
        wire_requests.extend_from_slice(&wire_request(43, 0, &[]));
        peer.write_all(&wire_requests)
            .expect("send raw X11 request stream including the 9-unit short PutImage");

        let mut packets = Vec::new();
        loop {
            let packet = read_response(&mut peer);
            let sequence = u16::from_le_bytes([packet[2], packet[3]]);
            let is_focus_reply = packet[0] == 1 && sequence == 8;
            packets.push(packet);
            if is_focus_reply {
                break;
            }
        }
        packets
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
    .expect("run core through valid, short, zero-size PutImage and sync");
    let packets = driver.join().expect("X11 request driver");
    let _ = reader_control.send(yserver_core::server::ReaderControl::Shutdown);
    let get_image_reply = |sequence: u16| {
        packets
            .iter()
            .find(|packet| packet[0] == 1 && u16::from_le_bytes([packet[2], packet[3]]) == sequence)
            .unwrap_or_else(|| panic!("missing GetImage reply for sequence {sequence}"))
    };
    let valid_reply = get_image_reply(4);
    assert_eq!(
        &valid_reply[32..48],
        known_pixels,
        "valid PutImage reaches Backend and is readable"
    );
    let unchanged_reply = get_image_reply(7);
    assert_eq!(
        &unchanged_reply[32..48],
        known_pixels,
        "short and zero-size requests leave the source pixmap unchanged",
    );
    let short_request_errors: Vec<_> = packets
        .iter()
        .filter(|packet| {
            packet[0] == 0 && u16::from_le_bytes([packet[2], packet[3]]) == 5 && packet[10] == 72
        })
        .map(|packet| (packet[1], packet[10]))
        .collect();
    assert!(
        short_request_errors.is_empty(),
        "short request sequence 5 opcode 72 must follow the existing silent length guard; observed {short_request_errors:?}",
    );
    assert_eq!(
        fixture.backend.effective_cursor_xid, None,
        "invalid source update does not alter the current cursor"
    );
    assert_eq!(
        fixture.recorders.cursor_buffer_ledger.records(),
        pool_records_before,
        "validation cases do not allocate or stage a cursor sprite",
    );
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .mapped_bytes(legacy_record.0, legacy_record.1)
            .expect("legacy cursor mapping remains live"),
        legacy_pixels_before,
        "validation cases do not write the legacy cursor backing",
    );

    let host_pixmap = fixture
        .state
        .resources
        .pixmap(pixmap_id)
        .and_then(|pixmap| pixmap.host_xid)
        .expect("core-created pixmap has a Backend drawable");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(host_pixmap),
        0,
        0,
        &[],
    )
    .expect("create picture for unchanged pixmap")
    .expect("picture handle");
    let cursor = Backend::render_create_cursor(&mut fixture.backend, None, picture, 0, 0)
        .expect("read unchanged pixmap as a RENDER cursor")
        .expect("cursor handle");
    assert_eq!(
        fixture.backend.cursor_records[&cursor.as_raw()].bgra_bytes,
        known_pixels,
        "normal RENDER cursor readback preserves every valid source pixel",
    );
    assert_eq!(
        fixture.backend.effective_cursor_xid, None,
        "readback alone does not define a cursor"
    );

    fixture.assert_cursor_quiescent();
    let validation_resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(
        validation_resources.end_state(),
        (0, 0, 0),
        "core-loop validation fixture releases every cursor allocation",
    );

    let Some(mut admission) = AtomicCursorFixture::new_with_cursor_capabilities(Some(32), Some(24))
        .expect("build oversized-cursor admission fixture")
    else {
        return;
    };
    let admission_records_before = admission.recorders.cursor_buffer_ledger.records();

    // This well-formed Backend source exceeds the 32x24 cursor cap; admission falls back to SW before pool upload.
    let oversized = Backend::create_pixmap(&mut admission.backend, None, 32, 33, 1)
        .expect("create oversized valid cursor pixmap through Backend");
    let oversized_pixels = vec![0x5a; 33 * 4];
    Backend::put_image(
        &mut admission.backend,
        None,
        oversized.as_raw(),
        32,
        33,
        1,
        0,
        0,
        &oversized_pixels,
    )
    .expect("upload oversized but well-formed ARGB source through Backend");
    let oversized_picture = Backend::render_create_picture(
        &mut admission.backend,
        None,
        AnyHandle::Pixmap(oversized),
        0,
        0,
        &[],
    )
    .expect("create oversized source picture")
    .expect("oversized source picture handle");
    let oversized_cursor =
        Backend::render_create_cursor(&mut admission.backend, None, oversized_picture, 0, 0)
            .expect("create well-formed oversized cursor")
            .expect("oversized cursor handle");
    Backend::set_cursor_hidden(&mut admission.backend, false);
    let root = admission.backend.core.window_id;
    Backend::define_cursor(
        &mut admission.backend,
        None,
        root,
        oversized_cursor.as_raw(),
    )
    .expect("define oversized cursor through Backend");
    assert_eq!(
        admission.backend.effective_cursor_xid,
        Some(oversized_cursor.as_raw()),
        "Backend definition selects the oversized cursor",
    );
    Backend::warp_pointer_root(&mut admission.backend, &mut admission.state, 80, 90);
    let root = admission.backend.core.window_id;
    let width = admission.backend.platform.fb_w;
    let height = admission.backend.platform.fb_h;
    Backend::clear_area(
        &mut admission.backend,
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
    .expect("damage scene for oversized cursor admission");
    Backend::maybe_composite(&mut admission.backend).expect("compose oversized SW cursor");
    admission.retire_output(0, 1);
    assert_eq!(
        admission.backend.scene.cursor_mode(),
        yserver_render::kms::render::scene::CursorPlaneMode::Sw,
        "well-formed source above the driver cap uses software cursor fallback",
    );
    assert_eq!(
        admission.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("cursor plane")
            .for_tests_sprite_pool_versions(),
        (None, None),
        "oversized source is rejected before sprite-pool upload",
    );
    assert_eq!(
        admission.recorders.cursor_buffer_ledger.records(),
        admission_records_before,
        "oversized admission does not allocate cursor sprite buffers",
    );

    Backend::set_cursor_hidden(&mut admission.backend, true);
    Backend::clear_area(
        &mut admission.backend,
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
    .expect("damage root for oversized cursor hide");
    Backend::maybe_composite(&mut admission.backend).expect("submit cursor hide through Backend");
    admission.retire_output(0, 2);
    admission.assert_cursor_quiescent();
    let resources = std::rc::Rc::clone(&admission.recorders.cursor_buffer_ledger);
    drop(admission);
    assert_eq!(
        resources.end_state(),
        (0, 0, 0),
        "test teardown releases cursor allocations"
    );
}

#[test]
// Mutation: reordering caps in the shared production KMS-open body must break this recorded trace.
fn atomic_cursor_only_shared_kms_open_body_orders_caps_before_driver_query() {
    use crate::drm::client_caps::{
        DeviceClientCapability, DeviceControl, DeviceControlCall, RecordingDeviceControl,
    };
    use std::sync::Arc;

    let control = Arc::new(RecordingDeviceControl::new(
        [Ok(()), Ok(()), Ok(())],
        Ok("amdgpu".to_owned()),
    ));
    let dependency: Arc<dyn DeviceControl + Send + Sync> = control.clone();
    let device = crate::drm::Device::for_tests_with_control(dependency)
        .expect("accepted client capabilities open the test KMS device");

    assert_eq!(
        control.calls(),
        vec![
            DeviceControlCall::SetClientCapability(DeviceClientCapability::UniversalPlanes),
            DeviceControlCall::SetClientCapability(DeviceClientCapability::Atomic),
            DeviceControlCall::SetClientCapability(DeviceClientCapability::CursorPlaneHotspot),
            DeviceControlCall::DriverName,
        ],
    );
    let caps = device
        .cursor_caps()
        .expect("test KMS device negotiated caps");
    assert!(caps.cursor_plane_hotspot_cap_accepted());
    assert_eq!(caps.driver_name(), "amdgpu");
}

#[test]
// Mutation: making hotspot-cap rejection fatal must fail this physical-driver startup assertion.
fn atomic_cursor_only_hotspot_rejection_is_cached_without_failing_open() {
    use crate::drm::client_caps::{DeviceControl, RecordingDeviceControl};
    use std::sync::Arc;

    let control = Arc::new(RecordingDeviceControl::new(
        [Ok(()), Ok(()), Err(libc::EOPNOTSUPP)],
        Ok("amdgpu".to_owned()),
    ));
    let dependency: Arc<dyn DeviceControl + Send + Sync> = control;
    let device = crate::drm::Device::for_tests_with_control(dependency)
        .expect("physical driver hotspot rejection is non-fatal");

    let caps = device
        .cursor_caps()
        .expect("test KMS device negotiated caps");
    assert!(!caps.cursor_plane_hotspot_cap_accepted());
    assert_eq!(caps.driver_name(), "amdgpu");
}

#[test]
// Mutation: renegotiating an inherited KMS fd must fail the no-call and empty-cache assertions.
fn atomic_cursor_only_inherited_fd_reuses_caps_without_negotiation() {
    use crate::drm::client_caps::{DeviceControl, RecordingDeviceControl};
    use std::{
        os::{fd::OwnedFd, unix::net::UnixStream},
        sync::Arc,
    };

    let control = Arc::new(RecordingDeviceControl::new(
        [Ok(()), Ok(()), Ok(())],
        Ok("amdgpu".to_owned()),
    ));
    let dependency: Arc<dyn DeviceControl + Send + Sync> = control.clone();
    let (stream, peer) = UnixStream::pair().expect("create synthetic inherited fd");
    let fd = OwnedFd::from(stream);
    let device = crate::drm::Device::from_inherited_kms_fd_with_control(
        fd,
        "<test inherited fd>",
        dependency,
    );
    drop(peer);

    assert!(device.cursor_caps().is_none());
    assert!(control.calls().is_empty());
}

#[test]
// Mutation: negotiating on a render node must fail the empty call and cache assertions.
fn atomic_cursor_only_render_node_skips_kms_capability_negotiation() {
    use crate::drm::client_caps::{DeviceControl, RecordingDeviceControl};
    use std::sync::Arc;

    let control = Arc::new(RecordingDeviceControl::new(
        [Ok(()), Ok(()), Ok(())],
        Ok("render-only".to_owned()),
    ));
    let dependency: Arc<dyn DeviceControl + Send + Sync> = control.clone();
    let device = crate::drm::Device::open_render_node_with_control("/dev/null", dependency)
        .expect("open synthetic render node");

    assert!(device.cursor_caps().is_none());
    assert!(control.calls().is_empty());
}

#[test]
// Mutation: dropping the cache from AtomicCursorFixture must fail the platform visibility assertions.
fn atomic_cursor_only_backend_fixture_exposes_device_caps() {
    let fixture = AtomicCursorFixture::without_scene().expect("build backend fixture");
    let device = &fixture.backend.platform.devices[0].device;
    let caps = device
        .cursor_caps()
        .expect("backend fixture device retains negotiated cursor caps");
    assert!(caps.cursor_plane_hotspot_cap_accepted());
    assert_eq!(caps.driver_name(), "atomic-cursor-fixture");
    fixture.assert_cursor_quiescent();
}

fn clone_drm_output(output: &crate::platform::drm::Output) -> crate::platform::drm::Output {
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

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: bypass the injected atomic executor; the accepted commit sequence must fail.
// Mutation: omit legacy position recording; the post-retirement bind/move sequence must fail.
fn atomic_cursor_only_trace_covers_backend_motion_and_retirement() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    let recorders = std::rc::Rc::clone(&fixture.recorders);
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    let root = fixture.backend.core.window_id;
    let width = fixture.backend.platform.fb_w;
    let height = fixture.backend.platform.fb_h;
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
    .expect("damage root through Backend");

    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor show frame");
    assert!(
        fixture.backend.scene.has_pending_page_flips(),
        "Backend composition must own the cursor show until its event retires",
    );
    let first = recorders.atomic.records();
    assert_eq!(
        first.len(),
        1,
        "show frame must use the selected atomic executor"
    );
    assert_eq!(
        first[0].kind,
        crate::drm::atomic::AtomicCommitKind::Scene,
        "the first scene frame establishes cursor ownership",
    );
    assert_eq!(
        first[0].result,
        crate::drm::atomic::tests_support::ScriptedResult::Ok,
        "the scene request was accepted",
    );
    assert_eq!(first[0].events.len(), 1);
    assert!(
        recorders.legacy_cursor.records().is_empty(),
        "legacy bind happens only after the accepted show frame retires",
    );

    fixture.retire_output(0, 1);
    let legacy_after_show = recorders.legacy_cursor.records();
    assert_eq!(legacy_after_show.len(), 2);
    assert_eq!(
        legacy_after_show
            .iter()
            .map(|record| record.operation)
            .collect::<Vec<_>>(),
        vec![
            crate::kms::cursor_plane::ops::LegacyCursorOperation::Bind,
            crate::kms::cursor_plane::ops::LegacyCursorOperation::Move,
        ],
    );
    assert!(legacy_after_show[0].framebuffer.is_some());
    assert!(legacy_after_show[1].position.is_some());

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 180, 140);
    let latest = fixture
        .backend
        .platform
        .cursor_plane_atomic_positions_for_outputs(&[0]);
    assert_eq!(
        latest.len(),
        1,
        "Backend motion must retain one cursor intent"
    );
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor-only motion");
    let after_motion = recorders.atomic.records();
    assert_eq!(
        after_motion
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
        ],
        "accepted scene and motion requests must stay ordered",
    );
    assert!(
        after_motion.iter().all(|record| {
            record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
        })
    );
    for (property, value) in [
        (latest[0].crtc_x, i64::from(latest[0].x) as u64),
        (latest[0].crtc_y, i64::from(latest[0].y) as u64),
    ] {
        assert!(after_motion[1].properties.iter().any(|update| {
            update.object == latest[0].plane.into()
                && update.property == property
                && update.value == value
        }));
    }
    assert_eq!(after_motion[1].events.len(), 1);
    assert!(
        fixture
            .backend
            .platform
            .for_tests_atomic_cursor_commit_is_in_flight(latest[0].device_key, latest[0].crtc,)
    );
    fixture.retire_output(0, 2);
    fixture.assert_cursor_quiescent();

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor hide frame");
    assert!(fixture.backend.scene.has_pending_page_flips());
    fixture.retire_output(0, 3);
    fixture.assert_cursor_quiescent();
    assert_eq!(
        recorders
            .atomic
            .records()
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "the accepted commit sequence is show, motion, hide",
    );
    assert_eq!(
        recorders
            .legacy_cursor
            .records()
            .last()
            .map(|record| record.operation),
        Some(crate::kms::cursor_plane::ops::LegacyCursorOperation::Hide),
    );

    let ledger = std::rc::Rc::clone(&recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(
        ledger.end_state(),
        (0, 0, 0),
        "cursor resources must be released"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: remove the scene-flip cursor blocker so motion submits CursorOnly before scene retirement.
fn atomic_cursor_only_motion_waits_for_scene_retirement_and_carries_latest_position() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    fixture.define_visible_cursor();
    let root = fixture.backend.core.window_id;
    let width = fixture.backend.platform.fb_w;
    let height = fixture.backend.platform.fb_h;
    let records_before = fixture.recorders.atomic.records().len();
    let damage_root = |fixture: &mut AtomicCursorFixture| {
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
        .expect("damage root through Backend");
    };

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor scene");
    assert!(
        fixture.backend.scene.has_pending_page_flips(),
        "Backend composition owns the scene until its page flip retires",
    );
    assert_eq!(
        fixture.recorders.atomic.records()[records_before].kind,
        crate::drm::atomic::AtomicCommitKind::Scene,
    );
    fixture.retire_output(0, 1);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));

    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("submit scene binding the eligible cursor plane");
    let binding_scene = fixture.recorders.atomic.records();
    assert_eq!(binding_scene.len(), records_before + 2);
    assert_eq!(
        binding_scene[records_before + 1].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    fixture.retire_output(0, 2);

    // Begin the tested scene flip after an accepted Backend frame has
    // established HW cursor ownership on the real test output.
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit scene before motion");
    assert!(fixture.backend.scene.has_pending_page_flips());
    assert_eq!(fixture.recorders.atomic.records().len(), records_before + 3,);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 210, 95);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("run Backend scheduling while the scene flip is pending");
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        records_before + 3,
        "motion cannot submit a cursor-only commit before scene retirement",
    );

    fixture.retire_output(0, 3);
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("submit scene with the latest cursor position");

    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), records_before + 4);
    assert_eq!(
        records[records_before + 2].kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    let hardware_scene = &records[records_before + 3];
    assert_eq!(
        hardware_scene.kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
    );
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let properties = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("fixture cursor plane")
        .properties_for(crtc)
        .expect("eligible cursor plane properties");
    for (property, value) in [
        (properties.crtc_x, 210_i64 as u64),
        (properties.crtc_y, 95_i64 as u64),
    ] {
        assert!(
            hardware_scene.properties.iter().any(|update| {
                update.object == properties.plane.into()
                    && update.property == property
                    && update.value == value
            }),
            "accepted scene carries latest cursor property {property:?}={value}",
        );
    }
    fixture.retire_output(0, 4);

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit cursor hide scene");
    assert!(fixture.backend.scene.has_pending_page_flips());
    fixture.retire_output(0, 5);
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture.recorders.atomic.records()[records_before..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "the exact accepted sequence is initial scene, binding scene, pending scene, latest-position scene, and hide",
    );

    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
// Mutation: ignore the selected hotplug source; the queued edge must fail to arm Backend's deadline.
fn atomic_cursor_only_hotplug_source_arms_backend_debounce() {
    use std::{cell::Cell, time::Duration};
    use yserver_core::backend::Backend;

    let mut fixture = AtomicCursorFixture::without_scene().expect("build backend fixture");
    let base = std::time::Instant::now();
    let elapsed_ms = std::rc::Rc::new(Cell::new(0_u64));
    let clock_ms = std::rc::Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(std::rc::Rc::new(move || {
            base + Duration::from_millis(clock_ms.get())
        }));
    assert!(Backend::next_wakeup(&fixture.backend).is_none());

    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    assert!(Backend::next_wakeup(&fixture.backend).is_none());

    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue pollable recording hotplug edge");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    let deadline = Backend::next_wakeup(&fixture.backend).expect("Backend advertises debounce");
    assert_eq!(deadline, base + Duration::from_millis(150));

    fixture.assert_cursor_quiescent();
    assert!(
        fixture.recorders.atomic.records().is_empty(),
        "an empty synthetic connector rescan must not submit atomic commits",
    );

    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: bypass the recording CRTC discovery service; Backend enable must fail before its modeset record.
// Mutation: skip final Backend disable; output/resource quiescence must fail.
fn atomic_cursor_only_randr_modeset_uses_injected_backend_services() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build backend fixture") else {
        return;
    };
    fixture
        .backend
        .initialize_provider_output_sources()
        .expect("automatically attach the selected renderer to the synthetic KMS sink");
    let selected: std::rc::Rc<dyn crate::drm::io::ModeBlobIo> =
        std::rc::Rc::clone(&fixture.backend.platform.services.mode_blobs);
    let recording: std::rc::Rc<dyn crate::drm::io::ModeBlobIo> =
        fixture.recorders.mode_blobs.clone();
    assert!(
        std::rc::Rc::ptr_eq(&selected, &recording),
        "for_tests must select the recording mode-blob operations",
    );
    assert_eq!(
        crate::drm::io::ModeBlobIo::for_tests_record_count(fixture.recorders.mode_blobs.as_ref()),
        Some(0),
        "for_tests must select the recording mode-blob operations",
    );

    let output = clone_drm_output(&fixture.backend.platform.outputs[0].output);
    let connector = output.connector_name.clone();
    let mode = yserver_core::backend::ModeSpec {
        width: output.picked.width,
        height: output.picked.height,
        vrefresh: output.picked.vrefresh,
    };
    let output_id = fixture
        .backend
        .randr_outputs_and_modes()
        .0
        .into_iter()
        .find(|candidate| candidate.name == connector)
        .expect("synthetic RANDR output")
        .output_id;
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.recorders.crtc_output_discovery.queue_output(output);

    let attempt = Backend::apply_crtc_config(
        &mut fixture.backend,
        output_id,
        &connector,
        Some(mode),
        13,
        0,
    );
    assert!(attempt.expect("recorded CRTC discovery and modeset succeed"));
    assert_eq!(fixture.recorders.crtc_output_discovery.calls(), 1);
    assert_eq!(fixture.backend.platform.outputs[0].x, 13);
    assert_eq!(fixture.recorders.mode_blobs.live_count(), 0);
    assert!(fixture.recorders.mode_blobs.records().is_empty());
    assert!(
        fixture.recorders.atomic.records().is_empty(),
        "the fake KMS fd must stop at property lookup before atomic submission",
    );
    assert_eq!(
        fixture
            .recorders
            .output_power
            .records()
            .iter()
            .map(|record| record.enabled)
            .collect::<Vec<_>>(),
        vec![false, true, true],
        "the accepted config records quiesce, modeset, and re-light in order",
    );
    assert!(
        Backend::apply_crtc_config(&mut fixture.backend, output_id, &connector, None, 0, 0,)
            .expect("Backend RandR teardown disables the output"),
    );
    assert!(
        fixture.backend.platform.outputs.is_empty(),
        "Backend RandR disable must remove the active output before quiescence",
    );
    assert_eq!(
        fixture
            .recorders
            .output_power
            .records()
            .iter()
            .map(|record| record.enabled)
            .collect::<Vec<_>>(),
        vec![false, true, true, false],
        "Backend teardown records the final output-off operation",
    );
    fixture.assert_cursor_quiescent();

    let blobs = std::rc::Rc::clone(&fixture.recorders.mode_blobs);
    let cursor_resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(blobs.live_count(), 0);
    assert_eq!(cursor_resources.end_state(), (0, 0, 0));
}

#[test]
// Mutation: drain the shared recording queue without matching the DRM device; the colliding CRTC event must remain queued.
fn atomic_cursor_only_flip_events_are_scoped_to_the_ready_device() {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    let mut fixture = AtomicCursorFixture::without_scene().expect("build backend fixture");
    let primary = std::rc::Rc::clone(&fixture.backend.platform.devices[0].device);
    let second = std::rc::Rc::new(
        crate::drm::Device::for_tests().expect("open second synthetic DRM device"),
    );
    let second_key = crate::platform::drm::DrmDeviceKey { major: 0, minor: 1 };
    let source = std::rc::Rc::clone(&fixture.recorders.page_flip_events);
    source.register_device(&second);
    fixture
        .backend
        .platform
        .devices
        .push(crate::kms::render::platform::KmsDevice {
            key: second_key,
            device: std::rc::Rc::clone(&second),
            async_page_flip: false,
            atomic_commit_sink: std::rc::Rc::new(crate::drm::atomic::AtomicCommitSink::new(
                std::rc::Rc::clone(&fixture.backend.platform.services.atomic),
            )),
            page_flip_event_source: source.clone(),
            cursor_buffer_io: std::rc::Rc::clone(&fixture.backend.platform.services.cursor_buffers),
            legacy_cursor_io: std::rc::Rc::clone(&fixture.backend.platform.services.legacy_cursor),
            test_page_flip_event_source: Some(source.clone()),
            cursor: crate::kms::render::platform::KmsCursorState::new(),
        });

    let colliding_crtc = ::drm::control::from_u32(1).expect("synthetic CRTC handle");
    source.queue_flip_for_device(&primary, colliding_crtc, 7);
    source.queue_flip_for_device(&second, colliding_crtc, 8);
    let primary_fd = primary.as_fd().as_raw_fd();
    Backend::on_page_flip_ready(&mut fixture.backend, &mut fixture.state, primary_fd);

    assert_eq!(source.queued_flip_count_for_device(&primary), 0);
    assert_eq!(
        source.queued_flip_count_for_device(&second),
        1,
        "draining one DRM fd cannot consume the other card's colliding CRTC event",
    );
    Backend::on_page_flip_ready(
        &mut fixture.backend,
        &mut fixture.state,
        second.as_fd().as_raw_fd(),
    );
    assert_eq!(source.queued_flip_count(), 0);
    assert!(fixture.recorders.atomic.records().is_empty());
    assert!(!fixture.backend.scene.has_pending_page_flips());
    assert!(fixture.backend.pstate.scanout_m2.pending.is_none());
    assert!(fixture.backend.pstate.scanout_m2.current.is_none());
    assert!(fixture.backend.pstate.scanout_m2.queued_successor.is_none());
    fixture.assert_cursor_quiescent();

    let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

const ATOMIC_CURSOR_PROPERTIES: [&str; 10] = [
    "FB_ID", "CRTC_ID", "CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H", "SRC_X", "SRC_Y", "SRC_W", "SRC_H",
];

fn cursor_topology_snapshot(
    driver_name: &str,
    hotspot_cap_accepted: bool,
    crtcs: &[u32],
    planes: &[(u32, Vec<u32>, Vec<&str>)],
) -> crate::kms::cursor_plane::CursorPlaneTestSnapshot {
    crate::kms::cursor_plane::CursorPlaneTestSnapshot {
        crtcs: crtcs.to_vec(),
        planes: planes
            .iter()
            .map(|(handle, possible_crtcs, properties)| {
                crate::kms::cursor_plane::CursorPlaneTestPlane {
                    handle: *handle,
                    possible_crtcs: possible_crtcs.clone(),
                    property_names: properties.iter().map(|name| (*name).to_owned()).collect(),
                }
            })
            .collect(),
        driver_name: driver_name.to_owned(),
        hotspot_cap_accepted,
    }
}

fn connector_snapshot_for_output(
    output: &crate::kms::backend::ActiveOutput,
) -> crate::kms::render::platform::ConnectorSnapshot {
    crate::kms::render::platform::ConnectorSnapshot {
        key: output.key.clone(),
        modes: vec![crate::platform::drm::Mode::default()],
        mm_width: 0,
        mm_height: 0,
        edid: Vec::new(),
        connector_type: "test".to_owned(),
    }
}

fn schedule_rescan_clock(
    backend: &mut crate::kms::render::backend::KmsBackend,
) -> (std::time::Instant, std::rc::Rc<std::cell::Cell<u64>>) {
    use std::{cell::Cell, time::Duration};
    let base = std::time::Instant::now();
    let elapsed_ms = std::rc::Rc::new(Cell::new(0_u64));
    let clock_ms = std::rc::Rc::clone(&elapsed_ms);
    backend.for_tests_set_monotonic_clock(std::rc::Rc::new(move || {
        base + Duration::from_millis(clock_ms.get())
    }));
    (base, elapsed_ms)
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit software cursor drawing or eligible cursor-plane X/Y fields;
// the accepted footprint and HW-scene assertions must fail.
fn atomic_cursor_only_eligibility_matrix() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor scene fixture") else {
        return;
    };
    let device_key = fixture.backend.platform.devices[0].key;
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    assert_eq!(crtc, ::drm::control::from_u32(1).expect("fixture CRTC"));
    fixture.define_visible_cursor();
    let (base, elapsed_ms) = schedule_rescan_clock(&mut fixture.backend);
    let properties: Vec<_> = ATOMIC_CURSOR_PROPERTIES.to_vec();
    let mut rows = Vec::new();
    rows.push((
        "no plane".to_owned(),
        cursor_topology_snapshot("amdgpu", true, &[1], &[]),
        false,
    ));
    rows.push((
        "unassigned CRTC".to_owned(),
        cursor_topology_snapshot("amdgpu", true, &[1], &[(10, vec![2], properties.clone())]),
        false,
    ));
    for missing in ATOMIC_CURSOR_PROPERTIES {
        let without = properties
            .iter()
            .copied()
            .filter(|name| *name != missing)
            .collect();
        rows.push((
            format!("missing {missing}"),
            cursor_topology_snapshot("amdgpu", true, &[1], &[(10, vec![1], without)]),
            false,
        ));
    }
    let mut virtual_properties = properties.clone();
    virtual_properties.extend(["HOTSPOT_X", "HOTSPOT_Y"]);
    for driver in ["virtio_gpu", "vmwgfx", "qxl", "vboxvideo"] {
        rows.push((
            format!("{driver} rejected hotspot cap"),
            cursor_topology_snapshot(
                driver,
                false,
                &[1],
                &[(10, vec![1], virtual_properties.clone())],
            ),
            false,
        ));
    }
    for missing in ["HOTSPOT_X", "HOTSPOT_Y"] {
        let without: Vec<_> = virtual_properties
            .iter()
            .copied()
            .filter(|name| *name != missing)
            .collect();
        rows.push((
            format!("accepted virtual cap without {missing}"),
            cursor_topology_snapshot("virtio_gpu", true, &[1], &[(10, vec![1], without)]),
            false,
        ));
    }
    rows.push((
        "virtual plane with accepted cap and both hotspots".to_owned(),
        cursor_topology_snapshot(
            "virtio_gpu",
            true,
            &[1],
            &[(10, vec![1], virtual_properties.clone())],
        ),
        true,
    ));
    rows.push((
        "maximum matching retains exclusive plane preference".to_owned(),
        cursor_topology_snapshot(
            "amdgpu",
            false,
            &[1, 2, 3],
            &[
                (21, vec![1, 2], properties.clone()),
                (22, vec![1, 3], properties.clone()),
                (23, vec![1, 3], properties.clone()),
                (24, vec![1], properties.clone()),
            ],
        ),
        true,
    ));
    rows.push((
        "physical plane with rejected cap and ordinary properties".to_owned(),
        cursor_topology_snapshot("amdgpu", false, &[1], &[(10, vec![1], properties.clone())]),
        true,
    ));

    for (row_index, (name, snapshot, eligible)) in rows.into_iter().enumerate() {
        let candidate_planes: Vec<::drm::control::plane::Handle> = snapshot
            .planes
            .iter()
            .map(|plane| ::drm::control::from_u32(plane.handle).expect("test plane"))
            .collect();
        assert!(
            fixture
                .backend
                .platform
                .for_tests_queue_cursor_discovery_snapshot(device_key, snapshot)
        );
        let calls_before = fixture
            .backend
            .platform
            .for_tests_cursor_discovery_calls(device_key)
            .expect("fixture discovery recorder");
        assert!(Backend::reprobe_connectors(&mut fixture.backend, &mut fixture.state).is_err());
        assert_eq!(
            fixture
                .backend
                .platform
                .for_tests_cursor_discovery_calls(device_key),
            Some(calls_before),
            "{name}: ordinary reprobe must not enumerate planes",
        );
        fixture
            .recorders
            .connector_snapshot
            .queue_snapshot(vec![connector_snapshot_for_output(
                &fixture.backend.platform.outputs[0],
            )]);
        fixture
            .recorders
            .hotplug
            .queue_change()
            .expect("queue hotplug edge");
        Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
        let deadline = fixture
            .backend
            .pstate
            .hotplug_rescan_deadline
            .expect("Backend arms its debounce deadline");
        assert!(Backend::next_wakeup(&fixture.backend).is_some());
        assert_eq!(
            deadline,
            base + std::time::Duration::from_millis(row_index as u64 * 150 + 150)
        );
        elapsed_ms.set(row_index as u64 * 150 + 149);
        Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
        assert_eq!(
            fixture
                .backend
                .platform
                .for_tests_cursor_discovery_calls(device_key),
            Some(calls_before),
            "{name}: no plane enumeration before the debounce deadline",
        );
        elapsed_ms.set(row_index as u64 * 150 + 150);
        Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
        assert_eq!(
            fixture
                .backend
                .platform
                .for_tests_cursor_discovery_calls(device_key),
            Some(calls_before + 1),
            "{name}: consumed hotplug must rediscover once",
        );

        assert_eq!(
            fixture.backend.platform.devices[0]
                .cursor
                .plane
                .as_ref()
                .and_then(|plane| plane.properties_for(crtc)),
            eligible.then(|| {
                fixture.backend.platform.devices[0]
                    .cursor
                    .plane
                    .as_ref()
                    .expect("cursor plane")
                    .properties_for(crtc)
                    .expect("eligible plane properties")
            }),
            "{name}: discovered property map",
        );
        assert_eq!(
            fixture
                .backend
                .platform
                .cursor_plane_available_for_output(0),
            eligible,
            "{name}: output-local availability",
        );
        assert_eq!(
            fixture
                .backend
                .platform
                .cursor_plane_fits_for_output(0, 1, 1),
            eligible,
            "{name}: output-local image fit",
        );
        if name == "maximum matching retains exclusive plane preference" {
            let plane = fixture.backend.platform.devices[0]
                .cursor
                .plane
                .as_ref()
                .expect("cursor plane");
            let assignments: Vec<_> = [1_u32, 2, 3]
                .into_iter()
                .map(|raw| {
                    plane
                        .properties_for(::drm::control::from_u32(raw).expect("test CRTC"))
                        .expect("maximum matching covers each CRTC")
                        .plane
                })
                .collect();
            assert_eq!(
                assignments
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                3
            );
            assert_eq!(
                assignments[0],
                ::drm::control::from_u32(24).expect("exclusive plane")
            );
        }
        let records_before = fixture.recorders.atomic.records().len();
        let legacy_records_before = fixture.recorders.legacy_cursor.records().len();
        let root = fixture.backend.core.window_id;
        let (width, height) = (fixture.backend.platform.fb_w, fixture.backend.platform.fb_h);
        Backend::set_cursor_hidden(&mut fixture.backend, false);
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 120, 110);
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
        .expect("damage root through Backend");
        Backend::maybe_composite(&mut fixture.backend).expect("compose eligibility row");
        let records = fixture.recorders.atomic.records();
        let accepted_scene = records[records_before..]
            .iter()
            .rev()
            .find(|record| {
                matches!(
                    record.kind,
                    crate::drm::atomic::AtomicCommitKind::Scene
                        | crate::drm::atomic::AtomicCommitKind::SceneWithCursor
                )
            })
            .unwrap_or_else(|| panic!("{name}: accepted scene record missing"));
        assert_eq!(
            accepted_scene.result,
            crate::drm::atomic::tests_support::ScriptedResult::Ok,
            "{name}: the scene carrying the software cursor must be accepted",
        );
        assert_eq!(
            accepted_scene.kind,
            crate::drm::atomic::AtomicCommitKind::Scene,
            "{name}: the first accepted frame establishes cursor ownership",
        );
        if !eligible {
            assert!(
                !accepted_scene.properties.iter().any(|update| {
                    candidate_planes
                        .iter()
                        .any(|plane| update.object == (*plane).into())
                }),
                "{name}: ineligible CRTC must not receive cursor-plane properties",
            );
        }
        let first_frame = u32::try_from(row_index * 2 + 1).expect("frame index fits");
        fixture.retire_output(0, first_frame);
        if eligible {
            assert!(
                fixture.backend.platform.cursor_plane_visible_for_output(0),
                "{name}: Backend retirement binds the eligible hardware plane",
            );
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
            .expect("damage eligible hardware cursor through Backend");
            Backend::maybe_composite(&mut fixture.backend)
                .expect("compose the active hardware cursor");
            let records = fixture.recorders.atomic.records();
            let hardware_scene = records[records_before + 1..]
                .iter()
                .rev()
                .find(|record| {
                    matches!(
                        record.kind,
                        crate::drm::atomic::AtomicCommitKind::Scene
                            | crate::drm::atomic::AtomicCommitKind::SceneWithCursor
                    )
                })
                .expect("eligible hardware scene record");
            assert_eq!(
                hardware_scene.kind,
                crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
                "{name}: accepted scene uses the eligible output-local cursor plane",
            );
            let properties = fixture.backend.platform.devices[0]
                .cursor
                .plane
                .as_ref()
                .expect("eligible cursor plane")
                .properties_for(crtc)
                .expect("eligible CRTC properties");
            for property in [properties.crtc_x, properties.crtc_y] {
                assert!(
                    hardware_scene.properties.iter().any(|update| {
                        update.object == properties.plane.into() && update.property == property
                    }),
                    "{name}: accepted hardware scene omitted required property {property:?}",
                );
            }
            assert!(
                fixture.recorders.legacy_cursor.records()[legacy_records_before..]
                    .iter()
                    .any(|record| {
                        record.operation
                            == crate::kms::cursor_plane::ops::LegacyCursorOperation::Bind
                            && record.framebuffer.is_some()
                    }),
                "{name}: Backend retirement binds a real eligible cursor framebuffer",
            );
            fixture.retire_output(0, first_frame + 1);
        } else {
            assert!(
                fixture
                    .backend
                    .scene
                    .for_tests_cursor_footprint_was_presented(0),
                "{name}: accepted scene paints the software cursor on an ineligible output",
            );
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
        .expect("damage cursor hide through Backend");
        Backend::maybe_composite(&mut fixture.backend).expect("compose cursor hide");
        if fixture.backend.scene.has_pending_page_flips() {
            fixture.retire_output(0, first_frame + 1);
        }
        let expected_scene_kind = if eligible {
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor
        } else {
            crate::drm::atomic::AtomicCommitKind::Scene
        };
        let mut expected_scene_sequence = vec![crate::drm::atomic::AtomicCommitKind::Scene];
        if eligible {
            expected_scene_sequence.push(crate::drm::atomic::AtomicCommitKind::SceneWithCursor);
        }
        expected_scene_sequence.push(expected_scene_kind);
        assert_eq!(
            fixture.recorders.atomic.records()[records_before..]
                .iter()
                .map(|record| record.kind)
                .collect::<Vec<_>>(),
            expected_scene_sequence,
            "{name}: exact show/hide scene commit sequence",
        );
        fixture.assert_cursor_quiescent();
    }

    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: omit an ineligible output's software draw or add a draw on the eligible peer; raw scanout pixels must fail.
fn atomic_cursor_only_composed_cursor_pixels_follow_output_local_eligibility() {
    use std::time::Duration;
    use yserver_core::backend::Backend;

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_two_outputs().expect("build two-output cursor scene fixture")
    else {
        return;
    };
    assert_eq!(fixture.backend.platform.outputs.len(), 2);
    assert_eq!(fixture.backend.platform.outputs[0].x, 0);
    assert_eq!(fixture.backend.platform.outputs[1].x, 800);
    let device_key = fixture.backend.platform.devices[0].key;
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let mut ineligible_properties = ATOMIC_CURSOR_PROPERTIES.to_vec();
    ineligible_properties.retain(|property| *property != "CRTC_Y");
    let snapshot = cursor_topology_snapshot(
        "amdgpu",
        false,
        &[1, 2],
        &[
            (10, vec![1], ineligible_properties),
            (11, vec![2], ATOMIC_CURSOR_PROPERTIES.to_vec()),
        ],
    );
    assert!(
        fixture
            .backend
            .platform
            .for_tests_queue_cursor_discovery_snapshot(device_key, snapshot)
    );
    let calls_before = fixture
        .backend
        .platform
        .for_tests_cursor_discovery_calls(device_key)
        .expect("cursor discovery recorder");
    fixture.recorders.connector_snapshot.queue_snapshot(vec![
        connector_snapshot_for_output(&fixture.backend.platform.outputs[0]),
        connector_snapshot_for_output(&fixture.backend.platform.outputs[1]),
    ]);
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue topology hotplug edge");
    let (base, elapsed_ms) = schedule_rescan_clock(&mut fixture.backend);
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture.backend.pstate.hotplug_rescan_deadline,
        Some(base + Duration::from_millis(150)),
        "Backend advertises the real topology debounce deadline",
    );
    assert!(Backend::next_wakeup(&fixture.backend).is_some());
    elapsed_ms.set(149);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_cursor_discovery_calls(device_key),
        Some(calls_before),
        "no plane enumeration occurs before the debounce deadline",
    );
    elapsed_ms.set(150);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_cursor_discovery_calls(device_key),
        Some(calls_before + 1),
        "the consumed edge rediscovered this device once",
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_fits_for_output(1, 2, 1)
    );

    let root = fixture.backend.core.window_id;
    let source =
        Backend::create_pixmap(&mut fixture.backend, None, 1, 2, 1).expect("create cursor bitmap");
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        1,
        2,
        1,
        0,
        0,
        &[3, 0, 0, 0],
    )
    .expect("paint two visible cursor pixels");
    let cursor = Backend::create_cursor(
        &mut fixture.backend,
        None,
        source,
        None,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        0,
        0,
    )
    .expect("create a cursor that spans the CRTC boundary");
    assert_eq!(
        (
            fixture.backend.cursor_records[&cursor.as_raw()].width,
            fixture.backend.cursor_records[&cursor.as_raw()].height,
        ),
        (2, 1),
        "the visible cursor spans one pixel on either output",
    );
    Backend::define_cursor(&mut fixture.backend, None, root, cursor.as_raw())
        .expect("define the two-pixel cursor");
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    assert!(!fixture.backend.effective_cursor_hidden());
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 799, 110);
    Backend::fill_rectangle(
        &mut fixture.backend,
        None,
        root,
        0x0011_2233,
        0,
        0,
        1600,
        600,
    )
    .expect("paint a uniform scene background through Backend");
    Backend::clear_area(
        &mut fixture.backend,
        None,
        root,
        0,
        None,
        0,
        0,
        1600,
        600,
        (0, 0),
    )
    .expect("damage both outputs through Backend");

    let records_before = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend).expect("compose the initial cursor frame");
    assert!(fixture.backend.scene.has_pending_page_flips());
    fixture.retire_output(0, 101);
    fixture.retire_output(1, 102);
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(1)
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(
        fixture.backend.platform.cursor_plane_visible_for_output(1),
        "eligible peer owns the cursor on its hardware plane",
    );

    Backend::clear_area(
        &mut fixture.backend,
        None,
        root,
        0,
        None,
        0,
        0,
        1600,
        600,
        (0, 0),
    )
    .expect("damage the active cursor frame through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("compose both output-local cursor paths");
    fixture.retire_output(0, 103);
    fixture.retire_output(1, 104);

    let read_pixel = |backend: &mut crate::kms::render::backend::KmsBackend, x: i32| -> [u8; 4] {
        let bytes = crate::kms::render::backend::read_scanout_region(
            backend,
            ash::vk::Rect2D {
                offset: ash::vk::Offset2D { x, y: 110 },
                extent: ash::vk::Extent2D {
                    width: 1,
                    height: 1,
                },
            },
            crate::kms::render::backend::ScanoutReadSelection::PermissiveDump,
        )
        .expect("read the raw composed scene output");
        bytes.try_into().expect("one BGRA pixel")
    };
    let background = read_pixel(&mut fixture.backend, 798);
    let software_cursor = read_pixel(&mut fixture.backend, 799);
    let hardware_peer_composed_pixel = read_pixel(&mut fixture.backend, 800);
    assert_ne!(
        &software_cursor[..3],
        &background[..3],
        "ineligible CRTC scanout contains the software cursor pixel",
    );
    assert_eq!(
        &hardware_peer_composed_pixel[..3],
        &background[..3],
        "eligible peer scanout omits the cursor pixel carried by its hardware plane",
    );
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(1));
    assert!(
        fixture
            .recorders
            .legacy_cursor
            .records()
            .iter()
            .any(|record| {
                record.operation == crate::kms::cursor_plane::ops::LegacyCursorOperation::Bind
                    && record.framebuffer.is_some()
            })
    );

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::clear_area(
        &mut fixture.backend,
        None,
        root,
        0,
        None,
        0,
        0,
        1600,
        600,
        (0, 0),
    )
    .expect("damage cursor hide through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("compose cursor hide");
    fixture.retire_output(0, 105);
    fixture.retire_output(1, 106);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(1));
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture.recorders.atomic.records()[records_before..]
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
        ],
        "exact show, active, and hide commit sequence for both CRTCs",
    );
    assert!(
        fixture.recorders.atomic.records()[records_before..]
            .iter()
            .all(|record| record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok)
    );

    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
// Mutation: suppress retryable cursor initialization after the CRTC returns;
// the second Backend enable must fail to create an eligible plane.
fn atomic_cursor_only_headless_enable_disable_enable_retries_initialization() {
    use std::time::Duration;
    use yserver_core::backend::Backend;

    let Some((mut fixture, key, output)) =
        AtomicCursorFixture::headless_for_lifecycle().expect("build headless lifecycle fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let device_key = key.device_key;
    let crtc = output.crtc;
    let factory = std::rc::Rc::clone(&fixture.recorders.cursor_plane_factory);
    factory.queue_error(libc::EIO);
    factory.queue_snapshot(cursor_topology_snapshot(
        "amdgpu",
        false,
        &[crtc.into()],
        &[(10, vec![crtc.into()], ATOMIC_CURSOR_PROPERTIES.to_vec())],
    ));

    assert!(fixture.backend.platform.outputs.is_empty());
    assert_eq!(
        factory.calls(),
        0,
        "headless construction defers cursor init"
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_cursor_initialization_state(device_key),
        Some((true, false)),
    );
    let (base, elapsed_ms) = schedule_rescan_clock(&mut fixture.backend);
    fixture.recorders.connector_snapshot.queue_snapshot(vec![
        crate::kms::render::platform::ConnectorSnapshot {
            key: key.clone(),
            modes: output.modes.clone(),
            mm_width: output.mm_width,
            mm_height: output.mm_height,
            edid: output.edid.clone(),
            connector_type: output.connector_type.clone(),
        },
    ]);
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue initial connected connector edge");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    let deadline = fixture
        .backend
        .pstate
        .hotplug_rescan_deadline
        .expect("Backend arms the hotplug debounce deadline");
    assert_eq!(deadline, base + Duration::from_millis(150));
    assert!(Backend::next_wakeup(&fixture.backend).is_some());
    assert!(Backend::reprobe_connectors(&mut fixture.backend, &mut fixture.state).is_err());
    assert_eq!(
        factory.calls(),
        0,
        "lightweight reprobe does not initialize planes"
    );
    elapsed_ms.set(149);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        factory.calls(),
        0,
        "cursor init waits for the rescan deadline"
    );
    elapsed_ms.set(150);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert_eq!(fixture.backend.platform.outputs.len(), 0);
    assert_eq!(factory.calls(), 0, "connected-Off discovery stays headless");

    let (outputs, _) = Backend::randr_outputs_and_modes(&mut fixture.backend);
    let output_id = outputs
        .iter()
        .find(|candidate| candidate.name == key.connector_name)
        .expect("connected headless connector is in RANDR registry")
        .output_id;
    let mode = yserver_core::backend::ModeSpec {
        width: output.picked.width,
        height: output.picked.height,
        vrefresh: output.picked.vrefresh,
    };

    fixture
        .recorders
        .crtc_output_discovery
        .queue_output(clone_drm_output(&output));
    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            output_id,
            &key.connector_name,
            Some(mode),
            0,
            0,
        )
        .expect("first explicit output activation")
    );
    assert_eq!(fixture.backend.platform.outputs.len(), 1);
    assert_eq!(
        factory.calls(),
        1,
        "first active CRTC attempts initialization"
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_cursor_initialization_state(device_key),
        Some((false, true)),
        "transient first failure is retained as an initialization retry",
    );
    let active_epoch = fixture.backend.platform.devices[0]
        .cursor
        .cursor_topology_epoch;

    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            output_id,
            &key.connector_name,
            None,
            0,
            0,
        )
        .expect("disable the only active output")
    );
    assert!(fixture.backend.platform.outputs.is_empty());
    assert_eq!(factory.calls(), 1, "zero-output topology does not retry");
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .cursor_topology_epoch,
        active_epoch + 1,
    );

    fixture
        .recorders
        .crtc_output_discovery
        .queue_output(clone_drm_output(&output));
    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            output_id,
            &key.connector_name,
            Some(mode),
            0,
            0,
        )
        .expect("reactivate output and retry cursor initialization")
    );
    assert_eq!(fixture.backend.platform.outputs.len(), 1);
    assert_eq!(
        factory.calls(),
        2,
        "active topology retries initialization once"
    );
    assert!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .and_then(|plane| plane.properties_for(crtc))
            .is_some()
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_cursor_initialization_state(device_key),
        Some((false, false)),
    );

    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            output_id,
            &key.connector_name,
            None,
            0,
            0,
        )
        .expect("Backend teardown disables the last output")
    );
    assert!(fixture.backend.platform.outputs.is_empty());
    fixture.assert_cursor_quiescent();
    let output_power_records = fixture.recorders.output_power.records();
    let synthetic_fb = ::drm::control::from_u32(0xF001).expect("synthetic framebuffer");
    assert_eq!(
        output_power_records
            .iter()
            .map(|record| (
                record.enabled,
                record.connector,
                record.crtc,
                record.framebuffer,
            ))
            .collect::<Vec<_>>(),
        vec![
            (true, output.connector, crtc, Some(synthetic_fb)),
            (true, output.connector, crtc, Some(synthetic_fb)),
            (false, output.connector, crtc, None),
            (true, output.connector, crtc, Some(synthetic_fb)),
            (true, output.connector, crtc, Some(synthetic_fb)),
            (false, output.connector, crtc, None),
        ],
        "accepted modeset and DPMS commits must follow enable/disable/enable/disable order",
    );
    assert!(fixture.recorders.atomic.records().is_empty());
    assert_eq!(fixture.recorders.crtc_output_discovery.calls(), 2);
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
// Mutation: share raw-handle caches between cards, refresh every card's
// applied topology, or skip the final Backend teardown; the peer/end-state
// assertions fail.
fn atomic_cursor_only_topology_rechecks_only_affected_device() {
    use std::time::Duration;
    use yserver_core::backend::Backend;

    let mut fixture = AtomicCursorFixture::without_scene().expect("build backend fixture");
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let first_key = fixture.backend.platform.devices[0].key;
    let second_key = crate::platform::drm::DrmDeviceKey { major: 0, minor: 1 };
    let second_device = std::rc::Rc::new(crate::drm::Device::for_tests().expect("second DRM fd"));
    let events = std::rc::Rc::clone(&fixture.recorders.page_flip_events);
    events.register_device(&second_device);
    let first_peer = {
        let route = fixture.backend.platform.outputs[0].scanout_route;
        let mut output = clone_drm_output(&fixture.backend.platform.outputs[0].output);
        output.connector_name = "card-a-peer".to_owned();
        output.connector = ::drm::control::from_u32(2).expect("peer connector");
        output.encoder = ::drm::control::from_u32(2).expect("peer encoder");
        output.crtc = ::drm::control::from_u32(2).expect("peer CRTC");
        output.plane = ::drm::control::from_u32(2).expect("peer plane");
        crate::kms::backend::ActiveOutput::new(
            route,
            output,
            crate::drm::Swapchain::empty_for_tests(),
            800,
            0,
        )
    };
    fixture.backend.platform.outputs.push(first_peer);
    let second_route = crate::kms::scanout_route::ScanoutRoute::new(
        crate::kms::render::platform::RenderDeviceId::UnverifiedFallback,
        second_key,
        crate::kms::scanout_route::RenderKmsRelationship::Unknown,
    );
    let second_output = {
        let mut output = clone_drm_output(&fixture.backend.platform.outputs[0].output);
        output.connector_name = "card-b".to_owned();
        crate::kms::backend::ActiveOutput::new(
            second_route,
            output,
            crate::drm::Swapchain::empty_for_tests(),
            800,
            0,
        )
    };
    fixture.backend.platform.outputs.push(second_output);
    fixture
        .backend
        .platform
        .devices
        .push(crate::kms::render::platform::KmsDevice {
            key: second_key,
            device: std::rc::Rc::clone(&second_device),
            async_page_flip: false,
            atomic_commit_sink: std::rc::Rc::new(crate::drm::atomic::AtomicCommitSink::new(
                std::rc::Rc::clone(&fixture.backend.platform.services.atomic),
            )),
            page_flip_event_source: events.clone(),
            cursor_buffer_io: std::rc::Rc::clone(&fixture.backend.platform.services.cursor_buffers),
            legacy_cursor_io: std::rc::Rc::clone(&fixture.backend.platform.services.legacy_cursor),
            test_page_flip_event_source: Some(events),
            cursor: crate::kms::render::platform::KmsCursorState::new(),
        });
    for _ in 0..2 {
        fixture.backend.platform.scanout_pools.push(None);
        fixture.backend.platform.bo_generations.push(Vec::new());
        fixture.backend.platform.first_pageflip_logged.push(false);
    }
    fixture
        .backend
        .platform
        .for_tests_initialize_cursor_planes();
    let colliding_crtc = fixture.backend.platform.outputs[0].output.crtc;
    assert_eq!(
        colliding_crtc,
        fixture.backend.platform.outputs[2].output.crtc
    );
    let first_epoch = fixture.backend.platform.devices[0]
        .cursor
        .cursor_topology_epoch;
    let second_epoch = fixture.backend.platform.devices[1]
        .cursor
        .cursor_topology_epoch;

    let (base, elapsed_ms) = schedule_rescan_clock(&mut fixture.backend);
    assert!(
        fixture
            .backend
            .platform
            .for_tests_queue_cursor_discovery_snapshot(
                first_key,
                cursor_topology_snapshot(
                    "amdgpu",
                    false,
                    &[1, 2],
                    &[(20, vec![2], ATOMIC_CURSOR_PROPERTIES.to_vec())],
                ),
            )
    );
    fixture.recorders.connector_snapshot.queue_snapshot(vec![
        connector_snapshot_for_output(&fixture.backend.platform.outputs[0]),
        connector_snapshot_for_output(&fixture.backend.platform.outputs[1]),
        connector_snapshot_for_output(&fixture.backend.platform.outputs[2]),
    ]);
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue edge");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    let deadline = Backend::next_wakeup(&fixture.backend).expect("real debounce deadline");
    assert_eq!(deadline, base + Duration::from_millis(150));
    elapsed_ms.set(150);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);

    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .cursor_topology_epoch,
        first_epoch + 1
    );
    assert_eq!(
        fixture.backend.platform.devices[1]
            .cursor
            .cursor_topology_epoch,
        second_epoch
    );
    assert_eq!(fixture.backend.platform.devices[0].key, first_key);
    assert_eq!(fixture.backend.platform.devices[1].key, second_key);
    assert!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .is_some_and(|plane| plane.properties_for(colliding_crtc).is_none())
    );
    assert!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .is_some_and(|plane| plane
                .properties_for(::drm::control::from_u32(2).unwrap())
                .is_some())
    );
    assert!(
        fixture.backend.platform.devices[1]
            .cursor
            .plane
            .as_ref()
            .is_some_and(|plane| plane.properties_for(colliding_crtc).is_some())
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_plane_available_for_output(0)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(1)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_fits_for_output(1, 1, 1)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_available_for_output(2)
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_fits_for_output(2, 1, 1)
    );

    let stable_first_epoch = fixture.backend.platform.devices[0]
        .cursor
        .cursor_topology_epoch;
    let stable_second_epoch = fixture.backend.platform.devices[1]
        .cursor
        .cursor_topology_epoch;
    assert!(
        fixture
            .backend
            .platform
            .for_tests_queue_cursor_discovery_snapshot(
                first_key,
                cursor_topology_snapshot(
                    "amdgpu",
                    false,
                    &[1, 2],
                    &[(20, vec![2], ATOMIC_CURSOR_PROPERTIES.to_vec())],
                ),
            )
    );
    fixture.recorders.connector_snapshot.queue_snapshot(vec![
        connector_snapshot_for_output(&fixture.backend.platform.outputs[0]),
        connector_snapshot_for_output(&fixture.backend.platform.outputs[1]),
        connector_snapshot_for_output(&fixture.backend.platform.outputs[2]),
    ]);
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue unchanged-topology edge");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture.backend.pstate.hotplug_rescan_deadline,
        Some(base + Duration::from_millis(300))
    );
    elapsed_ms.set(299);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    elapsed_ms.set(300);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .cursor_topology_epoch,
        stable_first_epoch,
        "unchanged connector/plane topology does not advance its epoch",
    );
    assert_eq!(
        fixture.backend.platform.devices[1]
            .cursor
            .cursor_topology_epoch,
        stable_second_epoch,
        "unchanged peer topology does not advance its epoch",
    );

    fixture
        .recorders
        .connector_snapshot
        .queue_snapshot(Vec::new());
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue final disconnected topology edge");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    let teardown_deadline = fixture
        .backend
        .pstate
        .hotplug_rescan_deadline
        .expect("Backend arms final teardown debounce");
    assert_eq!(teardown_deadline, base + Duration::from_millis(450));
    elapsed_ms.set(449);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert_eq!(fixture.backend.platform.outputs.len(), 3);
    elapsed_ms.set(450);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert!(fixture.backend.platform.outputs.is_empty());
    fixture.assert_cursor_quiescent();
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
