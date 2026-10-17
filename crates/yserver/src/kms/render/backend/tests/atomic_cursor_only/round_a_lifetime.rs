use super::{round_a::damage_cursor_root, support::AtomicCursorFixture};

fn argb_cursor(
    fixture: &mut AtomicCursorFixture,
    pixel: [u8; 4],
    hot_x: u16,
    hot_y: u16,
) -> yserver_core::backend::CursorHandle {
    use yserver_core::backend::{AnyHandle, Backend};

    let source = Backend::create_pixmap(&mut fixture.backend, None, 32, 1, 1)
        .expect("create cursor source through Backend");
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        32,
        1,
        1,
        0,
        0,
        &pixel,
    )
    .expect("write cursor pixels through Backend");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(source),
        0,
        0,
        &[],
    )
    .expect("create cursor picture through Backend")
    .expect("cursor picture");
    Backend::render_create_cursor(&mut fixture.backend, None, picture, hot_x, hot_y)
        .expect("create cursor through Backend")
        .expect("cursor handle")
}

fn framebuffer_storage(fixture: &AtomicCursorFixture, framebuffer: u32) -> (i32, u32, Vec<u8>) {
    use crate::kms::cursor_plane::ops::CursorBufferRecord;

    let (device_fd, handle) = fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .into_iter()
        .find_map(|record| match record {
            CursorBufferRecord::AddFb {
                device_fd,
                handle,
                fb,
                ..
            } if fb == framebuffer => Some((device_fd, handle)),
            _ => None,
        })
        .expect("cursor framebuffer has an AddFB2 record");
    let bytes = fixture
        .recorders
        .cursor_buffer_ledger
        .mapped_bytes(device_fd, handle)
        .expect("cursor framebuffer mapping remains live");
    (device_fd, handle, bytes)
}

fn framebuffer_storage_for_device(
    fixture: &AtomicCursorFixture,
    device_key: crate::platform::drm::DrmDeviceKey,
    framebuffer: u32,
) -> (i32, u32, Vec<u8>) {
    use crate::kms::cursor_plane::ops::CursorBufferRecord;
    use std::os::fd::{AsFd as _, AsRawFd as _};

    let device_fd = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .expect("output device")
        .device
        .as_fd()
        .as_raw_fd();
    let handle = fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .into_iter()
        .find_map(|record| match record {
            CursorBufferRecord::AddFb {
                device_fd: recorded_fd,
                handle,
                fb,
                ..
            } if recorded_fd == device_fd && fb == framebuffer => Some(handle),
            _ => None,
        })
        .expect("device-scoped cursor framebuffer has an AddFB2 record");
    let bytes = fixture
        .recorders
        .cursor_buffer_ledger
        .mapped_bytes(device_fd, handle)
        .expect("device-scoped cursor framebuffer mapping remains live");
    (device_fd, handle, bytes)
}

fn remove_fb_count(fixture: &AtomicCursorFixture, framebuffer: u32) -> usize {
    fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .iter()
        .filter(|record| {
            matches!(
                record,
                crate::kms::cursor_plane::ops::CursorBufferRecord::RemoveFb { fb, .. }
                    if *fb == framebuffer
            )
        })
        .count()
}

fn cursor_framebuffer_trace(fixture: &AtomicCursorFixture) -> Vec<(u32, u32)> {
    use crate::drm::atomic::tests_support::ScriptedResult;

    let targets: Vec<_> = fixture
        .backend
        .platform
        .outputs
        .iter()
        .filter_map(|layout| {
            let device = fixture
                .backend
                .platform
                .device_for_key(layout.key.device_key)?;
            let properties = device
                .cursor
                .plane
                .as_ref()?
                .properties_for(layout.output.crtc)?;
            Some((
                properties.plane.into(),
                properties.fb_id,
                u32::from(layout.output.crtc),
            ))
        })
        .collect();
    fixture
        .recorders
        .atomic
        .records()
        .into_iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .flat_map(|record| record.properties)
        .filter_map(|update| {
            targets
                .iter()
                .find(|(plane, fb_id, _)| update.object == *plane && update.property == *fb_id)
                .map(|(_, _, crtc)| (*crtc, u32::try_from(update.value).expect("FB id fits u32")))
        })
        .collect()
}

fn cursor_framebuffer_trace_by_device(
    fixture: &AtomicCursorFixture,
) -> Vec<(crate::platform::drm::DrmDeviceKey, u32, u32)> {
    use crate::drm::atomic::tests_support::ScriptedResult;
    use std::os::fd::{AsFd as _, AsRawFd as _};

    let targets: Vec<_> = fixture
        .backend
        .platform
        .outputs
        .iter()
        .filter_map(|layout| {
            let device = fixture
                .backend
                .platform
                .device_for_key(layout.key.device_key)?;
            let properties = device
                .cursor
                .plane
                .as_ref()?
                .properties_for(layout.output.crtc)?;
            Some((
                device.device.as_fd().as_raw_fd(),
                layout.key.device_key,
                properties.plane.into(),
                properties.fb_id,
                u32::from(layout.output.crtc),
            ))
        })
        .collect();
    fixture
        .recorders
        .atomic
        .records()
        .into_iter()
        .filter(|record| record.result == ScriptedResult::Ok)
        .flat_map(|record| {
            let device_fd = record.device_fd;
            let targets = &targets;
            record.properties.into_iter().filter_map(move |update| {
                targets
                    .iter()
                    .find(|(fd, _, plane, fb_id, _)| {
                        *fd == device_fd && update.object == *plane && update.property == *fb_id
                    })
                    .map(|(_, device_key, _, _, crtc)| {
                        (
                            *device_key,
                            *crtc,
                            u32::try_from(update.value).expect("FB id fits u32"),
                        )
                    })
            })
        })
        .collect()
}

fn cursor_binding(
    fixture: &AtomicCursorFixture,
    output_idx: usize,
) -> crate::kms::render::platform::cursor_state::CursorBinding {
    let layout = &fixture.backend.platform.outputs[output_idx];
    fixture
        .backend
        .platform
        .device_for_key(layout.key.device_key)
        .expect("output device")
        .cursor
        .crtc_states
        .get(&layout.output.crtc)
        .and_then(|state| state.applied)
        .expect("accepted cursor binding")
}

fn settle_initial_two_output_cursor(fixture: &mut AtomicCursorFixture) {
    use yserver_core::backend::Backend;

    let root = fixture.backend.core.window_id;
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial two-output cursor frame");
    fixture.retire_output(0, 1);
    fixture.retire_output(1, 2);
    fixture.retire_output(0, 3);
    fixture.retire_output(1, 4);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(1));
    assert_eq!(fixture.backend.core.window_id, root);
}

fn disable_and_drain(fixture: &mut AtomicCursorFixture, first_frame: u32) {
    use yserver_core::backend::Backend;

    Backend::set_cursor_hidden(&mut fixture.backend, true);
    damage_cursor_root(fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit Backend cursor hide");
    for frame_offset in 0..32 {
        let pending_outputs: Vec<_> = (0..fixture.backend.platform.outputs.len())
            .filter(|output_idx| {
                fixture.backend.scene.has_pending_page_flip(*output_idx)
                    || fixture
                        .backend
                        .platform
                        .cursor_atomic_event_pending_for_output(*output_idx)
            })
            .collect();
        if pending_outputs.is_empty() {
            break;
        }
        for output_idx in pending_outputs {
            fixture.retire_output(output_idx, first_frame + frame_offset);
        }
        Backend::maybe_composite(&mut fixture.backend).expect("retry Backend cursor hide");
    }
    Backend::set_dpms_power(&mut fixture.backend, 3)
        .expect("Backend DPMS entry disables outputs and detaches cursor planes");
    assert!(
        !fixture.backend.kms_outputs_active,
        "Backend disabled outputs"
    );
    fixture.assert_cursor_quiescent();
}

fn drive_animation_to_next_frame(fixture: &mut AtomicCursorFixture, animation: u32) -> u64 {
    use yserver_core::backend::Backend;

    let prior = fixture.backend.cursor_records[&animation].version;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut frame = 100;
    while fixture.backend.cursor_records[&animation].version == prior {
        damage_cursor_root(fixture);
        Backend::maybe_composite(&mut fixture.backend).expect("tick animation through Backend");
        if fixture.backend.scene.has_pending_page_flip(0)
            && !fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(0)
        {
            fixture.retire_output(0, frame);
            frame += 1;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Backend animation tick did not advance before the test deadline",
        );
    }
    fixture.backend.cursor_records[&animation].version
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: release a lease at acceptance or the first event, rewrite a live slot, or exceed the per-device pool bound.
fn atomic_cursor_only_shared_fb_survives_staggered_replacement() {
    use crate::kms::cursor_plane::ops::CursorBufferRecord;
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_overlapping_outputs()
        .expect("build overlapping two-output cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let root = fixture.backend.core.window_id;
    let first_pixels = [0x11, 0x22, 0x33, 0xff];
    let first_cursor = argb_cursor(&mut fixture, first_pixels, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, first_cursor.as_raw())
        .expect("define F0 through Backend");
    settle_initial_two_output_cursor(&mut fixture);

    let first_a = cursor_binding(&fixture, 0);
    let first_b = cursor_binding(&fixture, 1);
    assert_eq!(
        first_a.lease, first_b.lease,
        "both outputs share F0's lease"
    );
    assert_eq!(
        first_a.framebuffer, first_b.framebuffer,
        "both outputs bind F0"
    );
    let f0 = first_a.framebuffer;
    let (device_fd, handle, f0_bytes) = framebuffer_storage(&fixture, f0);
    assert_eq!(&f0_bytes[..4], &first_pixels);
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .records()
            .iter()
            .filter(
                |record| matches!(record, CursorBufferRecord::Create { handle, .. } if *handle != 1)
            )
            .count(),
        2,
        "the per-device sprite pool starts with two buffers",
    );
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("sprite pool")
            .sprite_user_count(first_a.lease),
        Some(2),
        "F0 is owned by both on-screen CRTCs",
    );

    // A gets a real scene successor. B's recorded page-flip event is withheld,
    // leaving B's scanout slot in flight so its successor is skipped.
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit staggered scene successor");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    fixture.retire_output(0, 5);
    assert!(fixture.backend.scene.has_pending_page_flip(1));

    let replacement_pixels = [0x44, 0x55, 0x66, 0xff];
    let replacement = argb_cursor(&mut fixture, replacement_pixels, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("define F1 through Backend");
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit A replacement and skip B");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.scene.has_pending_page_flip(1));

    let replaced_a = cursor_binding(&fixture, 0);
    let held_b = cursor_binding(&fixture, 1);
    assert_ne!(
        replaced_a.lease, first_a.lease,
        "A accepts F1 at its real scene retirement",
    );
    assert_eq!(held_b.lease, first_b.lease, "B still scans out F0");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    assert_eq!(
        fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[0].output.crtc]
            .in_flight
            .as_ref()
            .and_then(|commit| commit.prior)
            .map(|prior| prior.lease),
        Some(first_a.lease),
        "the accepted shared frame keeps A's prior binding through its event",
    );
    let before_a_event = framebuffer_storage(&fixture, f0);
    assert_eq!(before_a_event.0, device_fd);
    assert_eq!(before_a_event.1, handle);
    assert_eq!(
        before_a_event.2, f0_bytes,
        "accepted replacement leaves F0 bytes intact"
    );
    assert_eq!(
        remove_fb_count(&fixture, f0),
        0,
        "F0 is not removed on acceptance"
    );
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("sprite pool")
            .sprite_user_count(first_a.lease),
        Some(2),
        "A's pending scene event and B's binding both retain F0",
    );

    fixture.retire_output(0, 6);
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("sprite pool")
            .sprite_user_count(first_a.lease),
        Some(1),
        "A's replacing event releases only A's old F0 reference",
    );
    assert_eq!(framebuffer_storage(&fixture, f0).2, f0_bytes);
    assert_eq!(
        remove_fb_count(&fixture, f0),
        0,
        "B still owns F0 after A retires"
    );

    fixture.retire_output(1, 8);
    let mut b_replacement_accepted = false;
    for frame in 9..24 {
        Backend::maybe_composite(&mut fixture.backend).expect("retry B after its withheld event");
        let state = &fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[1].output.crtc];
        if state
            .applied
            .is_some_and(|binding| binding.lease == replaced_a.lease)
            && state.in_flight.is_some()
        {
            b_replacement_accepted = true;
            break;
        }
        if fixture.backend.scene.has_pending_page_flip(1)
            || fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(1)
        {
            fixture.retire_output(1, frame);
        }
    }
    assert!(
        b_replacement_accepted,
        "B accepts its replacement after the held scene retires"
    );
    fixture.retire_output(1, 24);
    assert_eq!(cursor_binding(&fixture, 1).lease, replaced_a.lease);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("sprite pool")
            .sprite_user_count(first_a.lease),
        Some(0),
        "B's final replacing event releases the last F0 reference",
    );
    assert_eq!(framebuffer_storage(&fixture, f0).2, f0_bytes);
    assert_eq!(
        remove_fb_count(&fixture, f0),
        0,
        "reuse keeps the pool allocation live"
    );

    disable_and_drain(&mut fixture, 10);
    assert_eq!(
        cursor_framebuffer_trace(&fixture),
        vec![
            (1, first_a.framebuffer),
            (2, first_b.framebuffer),
            (1, replaced_a.framebuffer),
            (2, replaced_a.framebuffer),
            (1, 0),
            (2, 0),
            (1, 0),
            (2, 0),
        ],
        "F0 remains the visible binding on B until its final event, then both outputs detach",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: key shared sprite leases by raw KMS handles without their DRM device.
fn atomic_cursor_only_identical_handles_are_scoped_to_each_device() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_colliding_device_handles()
        .expect("build one two-device cursor backend with colliding handles")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();

    let first_pixels = [0x1a, 0x2b, 0x3c, 0xff];
    let first_cursor = argb_cursor(&mut fixture, first_pixels, 0, 0);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, first_cursor.as_raw())
        .expect("define cursor through Backend");
    settle_initial_two_output_cursor(&mut fixture);

    let first_binding = cursor_binding(&fixture, 0);
    let second_binding = cursor_binding(&fixture, 1);
    let first_key = fixture.backend.platform.outputs[0].key.device_key;
    let second_key = fixture.backend.platform.outputs[1].key.device_key;
    assert_ne!(
        first_key, second_key,
        "outputs belong to separate DRM devices"
    );
    assert_eq!(
        fixture.backend.platform.outputs[0].output.crtc,
        fixture.backend.platform.outputs[1].output.crtc,
        "synthetic CRTC handles collide across the two DRM devices",
    );
    assert_eq!(
        first_binding.properties.plane, second_binding.properties.plane,
        "synthetic cursor plane handles collide across the two DRM devices",
    );
    assert_eq!(
        first_binding.framebuffer, second_binding.framebuffer,
        "synthetic framebuffer handles collide across the two DRM devices",
    );
    assert_eq!(first_binding.lease, second_binding.lease);
    for device_key in [first_key, second_key] {
        let users = fixture
            .backend
            .platform
            .device_for_key(device_key)
            .expect("output device")
            .cursor
            .plane
            .as_ref()
            .expect("device cursor plane")
            .sprite_user_count(first_binding.lease);
        assert_eq!(users, Some(1), "each device owns its own F0 user");
    }
    let first_storage =
        framebuffer_storage_for_device(&fixture, first_key, first_binding.framebuffer);
    let second_storage =
        framebuffer_storage_for_device(&fixture, second_key, second_binding.framebuffer);
    assert_ne!(
        first_storage.0, second_storage.0,
        "device fds remain distinct"
    );
    assert_eq!(
        first_storage.1, second_storage.1,
        "dumb-buffer handles collide"
    );
    assert_eq!(&first_storage.2[..4], &first_pixels);
    assert_eq!(&second_storage.2[..4], &first_pixels);

    // Let the first device accept F1 while the second keeps its scene event
    // pending. The equal raw FB id must still name separate allocations.
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit two-device predecessor");
    fixture.retire_output(0, 5);
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    let replacement_pixels = [0x4d, 0x5e, 0x6f, 0xff];
    let replacement = argb_cursor(&mut fixture, replacement_pixels, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, replacement.as_raw())
        .expect("define replacement through Backend");
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit first-device replacement");
    let first_replacement = cursor_binding(&fixture, 0);
    assert_ne!(first_replacement.framebuffer, first_binding.framebuffer);
    assert_eq!(cursor_binding(&fixture, 1), second_binding);
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    assert_eq!(
        &framebuffer_storage_for_device(&fixture, first_key, first_replacement.framebuffer).2[..4],
        &replacement_pixels,
    );
    assert_eq!(
        &framebuffer_storage_for_device(&fixture, second_key, first_replacement.framebuffer).2[..4],
        &[0; 4],
        "the second device's colliding FB id keeps its own untouched bytes",
    );

    fixture.retire_output(0, 6);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert!(
        !fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    assert_eq!(
        fixture
            .backend
            .platform
            .device_for_key(first_key)
            .unwrap()
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(first_binding.lease),
        Some(0),
        "A's event retires only the first device's F0 user",
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .device_for_key(second_key)
            .unwrap()
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(second_binding.lease),
        Some(1),
        "B's old framebuffer user remains on the second device",
    );

    fixture.retire_output(1, 7);
    let mut second_replacement_event = false;
    for frame in 8..24 {
        Backend::maybe_composite(&mut fixture.backend).expect("retry second-device replacement");
        let binding = cursor_binding(&fixture, 1);
        if binding.lease == first_replacement.lease
            && fixture.backend.scene.has_pending_page_flip(1)
        {
            second_replacement_event = true;
            break;
        }
        if fixture.backend.scene.has_pending_page_flip(1)
            || fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(1)
        {
            fixture.retire_output(1, frame);
        }
    }
    assert!(
        second_replacement_event,
        "second device accepts its own replacement"
    );
    fixture.retire_output(1, 24);
    assert_eq!(cursor_binding(&fixture, 1).lease, first_replacement.lease);

    disable_and_drain(&mut fixture, 25);
    assert_eq!(
        cursor_framebuffer_trace_by_device(&fixture)
            .into_iter()
            .filter(|(key, _, _)| *key == first_key)
            .map(|(_, crtc, fb)| (crtc, fb))
            .collect::<Vec<_>>(),
        vec![
            (1, first_binding.framebuffer),
            (1, first_replacement.framebuffer),
            (1, 0),
            (1, 0),
        ],
    );
    assert_eq!(
        cursor_framebuffer_trace_by_device(&fixture)
            .into_iter()
            .filter(|(key, _, _)| *key == second_key)
            .map(|(_, crtc, fb)| (crtc, fb))
            .collect::<Vec<_>>(),
        vec![
            (1, second_binding.framebuffer),
            (1, first_replacement.framebuffer),
            (1, 0),
            (1, 0),
        ],
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: retain the oldest wanted cursor image after pool pressure instead of the latest animation frame.
fn atomic_cursor_only_pool_pressure_keeps_only_newest_animation() {
    use crate::kms::cursor_plane::ops::CursorBufferRecord;
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_overlapping_outputs()
        .expect("build two-output pressure fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let f0_pixels = [0x10, 0x11, 0x12, 0xff];
    let f0_cursor = argb_cursor(&mut fixture, f0_pixels, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, f0_cursor.as_raw())
        .expect("define F0 through Backend");
    settle_initial_two_output_cursor(&mut fixture);
    let f0_binding_a = cursor_binding(&fixture, 0);
    let f0_binding_b = cursor_binding(&fixture, 1);
    let f0_bytes_initial = framebuffer_storage(&fixture, f0_binding_a.framebuffer).2;
    assert_eq!(&f0_bytes_initial[..4], &f0_pixels);
    assert_eq!(f0_binding_a.lease, f0_binding_b.lease);
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .records()
            .iter()
            .filter(
                |record| matches!(record, CursorBufferRecord::Create { handle, .. } if *handle != 1)
            )
            .count(),
        2,
        "N=2 starts with two sprite buffers before pool pressure",
    );

    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit held B scene commit");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    fixture.retire_output(0, 5);

    let f1_pixels = [0x20, 0x21, 0x22, 0xff];
    let f1_cursor = argb_cursor(&mut fixture, f1_pixels, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, f1_cursor.as_raw())
        .expect("define F1 through Backend");
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit A's F1 replacement");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    let f1_binding = cursor_binding(&fixture, 0);
    assert_eq!(
        f1_binding.lease.version(),
        fixture.backend.cursor_records[&f1_cursor.as_raw()].version
    );
    assert!(
        fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[0].output.crtc]
            .in_flight
            .is_some()
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    fixture.retire_output(0, 6);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    fixture.retire_output(0, 7);
    assert_eq!(
        framebuffer_storage(&fixture, f0_binding_a.framebuffer).2,
        f0_bytes_initial,
        "A's retirement does not rewrite F0 while B still displays it",
    );
    assert!(fixture.backend.scene.has_pending_page_flip(1));

    // Keep an A-only primary scene in flight before defining F2. A's
    // predecessor does not carry a cursor transition, so A remains bound to
    // F1 while B's later F2 event is withheld for the pressure phase.
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("submit A predecessor while B remains in flight");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.scene.has_pending_page_flip(1));

    let f2_pixels = [0x30, 0x31, 0x32, 0xff];
    let f2_cursor = argb_cursor(&mut fixture, f2_pixels, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, f2_cursor.as_raw())
        .expect("define B's F2 replacement through Backend");
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("keep B skipped while its scene is in flight");
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    assert_eq!(cursor_binding(&fixture, 1).lease, f0_binding_b.lease);

    // Deliver B's old scene, then let its real Backend retry accept F2.
    fixture.retire_output(1, 8);
    let mut b_f2_accepted = false;
    for frame in 9..32 {
        Backend::maybe_composite(&mut fixture.backend).expect("retry B's newest cursor scene");
        let state = &fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[1].output.crtc];
        if state.applied.is_some_and(|binding| {
            binding.lease.version() == fixture.backend.cursor_records[&f2_cursor.as_raw()].version
        }) && state.in_flight.is_some()
        {
            b_f2_accepted = true;
            break;
        }
        if fixture.backend.scene.has_pending_page_flip(1)
            || fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(1)
        {
            fixture.retire_output(1, frame);
        }
    }
    assert!(
        b_f2_accepted,
        "B accepts F2 through its Backend scene retry"
    );
    let f2_binding = cursor_binding(&fixture, 1);
    assert_ne!(f2_binding.lease, f1_binding.lease);
    assert_eq!(
        framebuffer_storage(&fixture, f0_binding_a.framebuffer).2,
        f0_bytes_initial,
        "B's accepted replacement still retains its old F0 bytes until the event",
    );
    let initial_sprite_fbs: Vec<_> = fixture
        .recorders
        .cursor_buffer_ledger
        .records()
        .into_iter()
        .filter_map(|record| match record {
            CursorBufferRecord::AddFb { handle, fb, .. } if handle != 1 => Some(fb),
            _ => None,
        })
        .collect();
    assert_eq!(
        initial_sprite_fbs.len(),
        3,
        "two initial sprite buffers plus one bounded growth slot"
    );
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .records()
            .iter()
            .filter(|record| {
                matches!(record, CursorBufferRecord::Create { handle, .. } if *handle != 1)
            })
            .count(),
        3,
        "N=2 CRTCs cap the pool at max(2, N+1)=3 sprite buffers",
    );

    let f0_before = f0_bytes_initial;
    let f1_before = framebuffer_storage(&fixture, f1_binding.framebuffer).2;
    let f2_before = framebuffer_storage(&fixture, f2_binding.framebuffer).2;
    assert_eq!(&f0_before[..4], &f0_pixels);
    assert_eq!(&f1_before[..4], &f1_pixels);
    assert_eq!(&f2_before[..4], &f2_pixels);

    let frame0_pixels = [0x40, 0x41, 0x42, 0xff];
    let frame1_pixels = [0x50, 0x51, 0x52, 0xff];
    let frame2_pixels = [0x60, 0x61, 0x62, 0xff];
    let frame0 = argb_cursor(&mut fixture, frame0_pixels, 0, 0);
    let frame1 = argb_cursor(&mut fixture, frame1_pixels, 0, 0);
    let frame2 = argb_cursor(&mut fixture, frame2_pixels, 0, 0);
    let animation = Backend::create_anim_cursor(
        &mut fixture.backend,
        None,
        &[(frame0, 1), (frame1, 1), (frame2, 60_000)],
    )
    .expect("create cursor animation through Backend")
    .expect("animation handle");
    Backend::define_cursor(&mut fixture.backend, None, root, animation.as_raw())
        .expect("define animation through Backend while every slot is in use");
    fixture.retire_output(0, 30);
    assert!(fixture.backend.scene.has_pending_page_flip(1));
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit newest animation image attempt");
    let intermediate = drive_animation_to_next_frame(&mut fixture, animation.as_raw());
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .records()
            .iter()
            .filter(|record| {
                matches!(record, CursorBufferRecord::Create { handle, .. } if *handle != 1)
            })
            .count(),
        3,
        "pressure defers animation instead of growing past the bound",
    );
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend)
        .expect("attempt intermediate frame through Backend");
    let newest = drive_animation_to_next_frame(&mut fixture, animation.as_raw());
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("attempt newest frame through Backend");
    assert_ne!(intermediate, newest);
    let newest_record = fixture.backend.cursor_records[&animation.as_raw()].clone();
    assert_eq!(newest_record.bgra_bytes, frame2_pixels);
    let mut frame = 120;
    for _ in 0..32 {
        let wanted = fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .for_tests_sprite_pool_versions()
            .0;
        if wanted == Some(newest) {
            break;
        }
        Backend::maybe_composite(&mut fixture.backend)
            .expect("retry latest animation frame through Backend while B remains held");
        if fixture.backend.scene.has_pending_page_flip(0)
            || fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(0)
        {
            fixture.retire_output(0, frame);
            frame += 1;
        }
    }
    for (lease, framebuffer, bytes) in [
        (f0_binding_a.lease, f0_binding_a.framebuffer, f0_before),
        (f1_binding.lease, f1_binding.framebuffer, f1_before),
        (f2_binding.lease, f2_binding.framebuffer, f2_before),
    ] {
        let actual = framebuffer_storage(&fixture, framebuffer).2;
        assert_eq!(
            actual, bytes,
            "busy framebuffer is immutable under animation pressure"
        );
        assert!(
            fixture.backend.platform.devices[0]
                .cursor
                .plane
                .as_ref()
                .unwrap()
                .sprite_user_count(lease)
                .is_some_and(|users| users > 0),
            "lease version {} stays user-owned through pressure",
            lease.version(),
        );
    }
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .for_tests_sprite_pool_versions(),
        (Some(newest), Some(f2_binding.lease.version())),
        "the pool retains the newest animation frame while every slot has a user",
    );
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .records()
            .iter()
            .filter(|record| {
                matches!(record, CursorBufferRecord::Create { handle, .. } if *handle != 1)
            })
            .count(),
        3,
        "the full pool defers newest animation without growing beyond max(2, N+1)",
    );

    // A's replacement event has retired; B's final replacement event alone
    // releases F0 and allows the retained newest frame to make progress.
    fixture.retire_output(1, 32);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(f0_binding_a.lease),
        Some(0),
        "B's final replacing event releases the last F0 user",
    );
    let mut newest_accepted = false;
    let mut wanted_after_release = None;
    for frame in 33..64 {
        Backend::maybe_composite(&mut fixture.backend)
            .expect("retry newest frame without pointer motion");
        wanted_after_release = fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .for_tests_sprite_pool_versions()
            .0;
        newest_accepted = (0..2).all(|output_idx| {
            let state = &fixture.backend.platform.devices[0].cursor.crtc_states
                [&fixture.backend.platform.outputs[output_idx].output.crtc];
            state
                .applied
                .is_some_and(|binding| binding.lease.version() == newest)
                && state.in_flight.is_some()
        });
        if newest_accepted {
            break;
        }
        for output_idx in 0..2 {
            if fixture.backend.scene.has_pending_page_flip(output_idx)
                || fixture
                    .backend
                    .platform
                    .cursor_atomic_event_pending_for_output(output_idx)
            {
                fixture.retire_output(output_idx, frame);
            }
        }
    }
    assert!(
        newest_accepted,
        "resource release schedules the newest animation frame without pointer motion"
    );
    assert!(
        fixture
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
    assert_eq!(
        wanted_after_release,
        Some(newest),
        "resource release retries the newest frame and drops the intermediate wanted image",
    );
    assert_eq!(
        fixture
            .recorders
            .cursor_buffer_ledger
            .records()
            .iter()
            .filter(|record| {
                matches!(record, CursorBufferRecord::Create { handle, .. } if *handle != 1)
            })
            .count(),
        3,
        "pressure reuses a released slot without growing past the pool bound",
    );
    let newest_binding = (0..2)
        .map(|output_idx| cursor_binding(&fixture, output_idx))
        .find(|binding| binding.lease.version() == newest)
        .expect("newest animation lease is accepted");
    assert_eq!(
        &framebuffer_storage(&fixture, newest_binding.framebuffer).2[..4],
        &frame2_pixels
    );
    assert_ne!(
        &framebuffer_storage(&fixture, newest_binding.framebuffer).2[..4],
        &frame1_pixels,
        "intermediate animation image was never replayed after pressure cleared",
    );

    disable_and_drain(&mut fixture, 64);
    assert_eq!(
        cursor_framebuffer_trace(&fixture),
        vec![
            (1, f0_binding_a.framebuffer),
            (2, f0_binding_b.framebuffer),
            (1, f1_binding.framebuffer),
            (2, f2_binding.framebuffer),
            (1, newest_binding.framebuffer),
            (2, newest_binding.framebuffer),
            (1, 0),
            (2, 0),
            (1, 0),
            (2, 0),
        ],
        "the retained newest animation image is the only post-pressure frame committed",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: reuse the current framebuffer for equal pixels and keep it bound across a hotspot change.
fn atomic_cursor_only_hotspot_only_switches_framebuffer() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_hotspot_properties().expect("build hotspot cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let pixels = [0x5a, 0x6b, 0x7c, 0xff];
    let original = argb_cursor(&mut fixture, pixels, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, original.as_raw())
        .expect("define original cursor through Backend");
    let original_image = Backend::get_active_cursor_image(&fixture.backend)
        .expect("Backend reports active original cursor");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);
    let prior = cursor_binding(&fixture, 0);
    let prior_bytes = framebuffer_storage(&fixture, prior.framebuffer).2;

    let changed_hotspot = argb_cursor(&mut fixture, pixels, 1, 1);
    Backend::define_cursor(&mut fixture.backend, None, root, changed_hotspot.as_raw())
        .expect("define byte-identical cursor with a changed hotspot through Backend");
    let changed_image = Backend::get_active_cursor_image(&fixture.backend)
        .expect("Backend reports active changed-hotspot cursor");
    assert!(
        changed_image.serial > original_image.serial,
        "Backend cursor creation gives the changed hotspot a new monotonic version",
    );
    assert_eq!(original_image.bgra_bytes, changed_image.bgra_bytes);
    assert_ne!(
        (original_image.hot_x, original_image.hot_y),
        (changed_image.hot_x, changed_image.hot_y),
    );

    damage_cursor_root(&mut fixture);
    let records_before = fixture.recorders.atomic.records().len();
    Backend::maybe_composite(&mut fixture.backend).expect("submit hotspot-only replacement");
    assert!(
        fixture.backend.scene.has_pending_page_flip(0),
        "accepted hotspot replacement awaits its shared scene event",
    );

    let next = cursor_binding(&fixture, 0);
    assert_eq!(next.lease.version(), u64::from(changed_image.serial));
    assert_ne!(
        next.framebuffer, prior.framebuffer,
        "hotspot identity needs a distinct FB"
    );
    assert_eq!((next.hot_x, next.hot_y), (1, 1));
    assert_eq!(
        (next.x, next.y),
        (99, 89),
        "CRTC coordinates subtract the new hotspot"
    );
    assert_eq!(
        framebuffer_storage(&fixture, prior.framebuffer).2,
        prior_bytes
    );
    assert_eq!(
        remove_fb_count(&fixture, prior.framebuffer),
        0,
        "the old framebuffer remains allocated while its replacement event is pending",
    );
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("sprite pool")
            .sprite_user_count(prior.lease),
        Some(1),
        "the accepted replacement retains the old framebuffer through retirement",
    );
    assert!(
        fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[0].output.crtc]
            .in_flight
            .as_ref()
            .is_some_and(|commit| commit.prior == Some(prior))
    );
    let records = fixture.recorders.atomic.records();
    let update = records[records_before..]
        .iter()
        .find(|record| {
            record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok
                && record.properties.iter().any(|property| {
                    property.object == next.properties.plane.into()
                        && property.property == next.properties.fb_id
                        && property.value == u64::from(next.framebuffer)
                })
        })
        .expect("accepted commit binds the distinct hotspot framebuffer");
    for (property, value) in [
        (next.properties.crtc_x, i64::from(next.x) as u64),
        (next.properties.crtc_y, i64::from(next.y) as u64),
    ] {
        assert!(update.properties.iter().any(|field| {
            field.object == next.properties.plane.into()
                && field.property == property
                && field.value == value
        }));
    }
    for (property, value) in [
        (
            next.properties
                .hotspot_x
                .expect("fixture HOTSPOT_X property"),
            next.hot_x,
        ),
        (
            next.properties
                .hotspot_y
                .expect("fixture HOTSPOT_Y property"),
            next.hot_y,
        ),
    ] {
        assert!(update.properties.iter().any(|field| {
            field.object == next.properties.plane.into()
                && field.property == property
                && field.value == u64::from(value)
        }));
    }

    fixture.retire_output(0, 4);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .expect("sprite pool")
            .sprite_user_count(prior.lease),
        Some(0),
        "the replacing event releases the old framebuffer user",
    );
    assert_eq!(
        framebuffer_storage(&fixture, prior.framebuffer).2,
        prior_bytes
    );
    disable_and_drain(&mut fixture, 4);
    assert_eq!(
        cursor_framebuffer_trace(&fixture),
        vec![
            (1, prior.framebuffer),
            (1, next.framebuffer),
            (1, 0),
            (1, 0)
        ],
        "byte-identical hotspot replacement has a distinct framebuffer before detach",
    );
    let ledger = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
