use super::support::{
    accepted_live_records, assert_no_cursor_resources, create_argb_cursor, finish_fixture,
    retire_pending_output,
};
use crate::{
    drm::{CursorDeviceCaps, atomic::AtomicCommitKind},
    kms::cursor_plane::{
        ops::{CursorBufferIo, recording::RecordingCursorBufferIo},
        topology::{
            CursorPathReason, CursorPlaneTestPlane, CursorPlaneTestSnapshot, CursorStatePath,
        },
    },
};
use std::{
    collections::HashSet,
    os::fd::{AsFd as _, AsRawFd as _},
    rc::Rc,
};
use yserver_core::backend::Backend;

const CURSOR_PROPERTIES: [&str; 12] = [
    "FB_ID",
    "CRTC_ID",
    "CRTC_X",
    "CRTC_Y",
    "CRTC_W",
    "CRTC_H",
    "SRC_X",
    "SRC_Y",
    "SRC_W",
    "SRC_H",
    "HOTSPOT_X",
    "HOTSPOT_Y",
];

#[test]
#[ignore = "needs lavapipe Vulkan scene composition"]
// Mutation: route a non-qualified snapshot to a hardware cursor path; the
// static path, permanent latch, and zero hardware cursor-commit assertions fail.
fn non_atomic_device_snapshots_use_software_cursor_without_changing_x_state() {
    use super::super::commit_identity::recording_scene::RecordingSceneFixture;

    let without_hotspot: Vec<_> = CURSOR_PROPERTIES
        .into_iter()
        .filter(|name| !name.starts_with("HOTSPOT_"))
        .map(str::to_owned)
        .collect();
    let with_property_missing: Vec<_> = CURSOR_PROPERTIES
        .into_iter()
        .filter(|name| *name != "CRTC_W")
        .map(str::to_owned)
        .collect();
    let full_properties: Vec<_> = CURSOR_PROPERTIES.into_iter().map(str::to_owned).collect();
    let scenarios = [
        (
            "old paravirtualized guest rejects hotspot capability",
            Some(without_hotspot),
            CursorDeviceCaps::new(false, Some("virtio_gpu".to_owned())),
            CursorPathReason::HotspotSupportMissing,
        ),
        (
            "driver name query failed",
            Some(full_properties.clone()),
            CursorDeviceCaps::new(false, None),
            CursorPathReason::HotspotSupportMissing,
        ),
        (
            "required plane property is missing",
            Some(with_property_missing),
            CursorDeviceCaps::new(false, Some("amdgpu".to_owned())),
            CursorPathReason::MissingProperty("CRTC_W"),
        ),
        (
            "device exposes no cursor plane",
            None,
            CursorDeviceCaps::new(false, Some("apple-dcp".to_owned())),
            CursorPathReason::NoCursorPlane,
        ),
    ];

    let mut results = Vec::new();
    for (scenario_index, (name, property_names, caps, expected_reason)) in
        scenarios.into_iter().enumerate()
    {
        let Some(mut scene) = RecordingSceneFixture::new(1).expect("software cursor scene fixture")
        else {
            assert_eq!(
                scenario_index, 0,
                "scene fixture became unavailable mid-test"
            );
            return;
        };
        let events = &mut scene.events;
        let crtc = events.backend.platform.outputs[0].output.crtc;
        let crtc_raw = u32::from(crtc);
        let device_key = events.backend.platform.devices[0].key;
        let device_fd = events.backend.platform.devices[0]
            .device
            .as_fd()
            .as_raw_fd();
        let planes = property_names.map_or_else(Vec::new, |property_names| {
            vec![CursorPlaneTestPlane {
                handle: 21,
                possible_crtcs: vec![crtc_raw],
                property_names,
            }]
        });
        let snapshot = CursorPlaneTestSnapshot {
            crtcs: vec![crtc_raw],
            planes,
            cursor_caps: Some(caps),
        };
        let (io, ledger) = RecordingCursorBufferIo::new(1);
        let buffer_io: Rc<dyn CursorBufferIo> = Rc::new(io);
        events
            .backend
            .platform
            .for_tests_install_cursor_plane_from_snapshot(0, snapshot, buffer_io, 64, 64);

        let device = &events.backend.platform.devices[0];
        let plane = device
            .cursor
            .plane
            .as_ref()
            .expect("installed cursor topology");
        let path_ok = device.cursor.state_path() == CursorStatePath::Software
            && plane.state_path().reason == expected_reason;
        let latch_ok = events
            .backend
            .platform
            .for_tests_cursor_permanently_disabled(device_key);
        if !path_ok {
            let mut delivered = HashSet::new();
            finish_fixture(events, &mut delivered);
            assert_no_cursor_resources(&events.backend, Some(&ledger));
            scene.finish_after_normal_drain();
            results.push((name, path_ok, latch_ok, false, false, false));
            continue;
        }
        let no_hardware_plane = !events.backend.platform.cursor_plane_available()
            && !events.backend.platform.cursor_plane_visible_for_output(0);

        let cursor = create_argb_cursor(
            &mut events.backend,
            (5, 4),
            [0x12, 0x34, 0x56, 0xff],
            (2, 1),
        );
        let cursor_xid = cursor.as_raw();
        let root = events.backend.core.window_id;
        Backend::define_cursor(&mut events.backend, None, root, cursor_xid)
            .expect("define root cursor through Backend");
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 40, 50);
        let expected_image = Backend::get_active_cursor_image(&events.backend)
            .expect("GetCursorImage retains the selected cursor");
        let initial_x_state_ok = expected_image.host_xid == cursor_xid
            && (expected_image.width, expected_image.height) == (5, 4)
            && (expected_image.hot_x, expected_image.hot_y) == (2, 1)
            && !events.backend.effective_cursor_hidden();

        Backend::set_cursor_hidden(&mut events.backend, true);
        let hidden_image = Backend::get_active_cursor_image(&events.backend)
            .expect("GetCursorImage remains available while hidden");
        let hide_ok =
            events.backend.effective_cursor_hidden() && hidden_image.host_xid == cursor_xid;
        Backend::set_cursor_hidden(&mut events.backend, false);
        let show_ok = !events.backend.effective_cursor_hidden()
            && events.backend.sprite_key() == Some((expected_image.serial.into(), (2, 1)));

        let root_width = events.backend.platform.fb_w;
        let root_height = events.backend.platform.fb_h;
        Backend::clear_area(
            &mut events.backend,
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
        .expect("damage root to compose the visible software cursor");
        let recorder = &events.recorders[&device_fd];
        Backend::maybe_composite(&mut events.backend)
            .expect("compose the visible software cursor through Backend");
        let records = accepted_live_records(recorder);
        let scene_composed = records
            .iter()
            .any(|record| record.kind == AtomicCommitKind::Scene);
        let no_hardware_cursor_commits = records
            .iter()
            .all(|record| record.kind != AtomicCommitKind::CursorOnly);
        let no_cursor_buffers = ledger.records().is_empty();
        let after_image = Backend::get_active_cursor_image(&events.backend)
            .expect("GetCursorImage remains available after software composition");
        let cursor_image_unchanged = after_image.host_xid == expected_image.host_xid
            && after_image.serial == expected_image.serial
            && after_image.width == expected_image.width
            && after_image.height == expected_image.height
            && after_image.hot_x == expected_image.hot_x
            && after_image.hot_y == expected_image.hot_y
            && after_image.x == expected_image.x
            && after_image.y == expected_image.y
            && after_image.bgra_bytes == expected_image.bgra_bytes;

        let mut delivered = HashSet::new();
        retire_pending_output(
            events,
            &mut delivered,
            0,
            u32::try_from(scenario_index + 1).expect("test frame fits u32"),
        );
        Backend::define_cursor(&mut events.backend, None, root, 0x7f10)
            .expect("restore fixture root cursor");
        Backend::free_cursor(&mut events.backend, None, cursor_xid)
            .expect("free test cursor through Backend");
        finish_fixture(events, &mut delivered);
        assert_no_cursor_resources(&events.backend, Some(&ledger));
        scene.finish_after_normal_drain();
        results.push((
            name,
            path_ok,
            latch_ok,
            no_hardware_plane,
            initial_x_state_ok && hide_ok && show_ok && cursor_image_unchanged,
            scene_composed && no_hardware_cursor_commits && no_cursor_buffers,
        ));
    }

    for (name, path_ok, latch_ok, no_hardware_plane, x_state_ok, software_scene_ok) in results {
        assert!(path_ok, "{name}: static path and reason");
        assert!(
            latch_ok,
            "{name}: static decision sets the existing permanent software latch"
        );
        assert!(
            no_hardware_plane,
            "{name}: no KMS cursor plane is available or visible"
        );
        assert!(
            x_state_ok,
            "{name}: GetCursorImage, hotspot, and hide/show state"
        );
        assert!(
            software_scene_ok,
            "{name}: visible cursor composes without cursor commits or cursor buffers"
        );
    }
}
