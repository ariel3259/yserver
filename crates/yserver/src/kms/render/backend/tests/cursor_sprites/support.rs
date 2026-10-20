use crate::{
    drm::atomic::tests_support::{
        AtomicCommitRecord, RecordingAtomicCommitExecutor, ScriptedResult,
    },
    kms::render::backend::KmsBackend,
};
use std::{
    collections::HashSet,
    os::fd::{AsFd as _, AsRawFd as _, RawFd},
    time::Duration,
};
use yserver_core::backend::{AnyHandle, Backend, CursorHandle};

use super::super::commit_identity::support::{AtomicEventFixture, accepted_event_id};

pub(crate) fn create_argb_cursor(
    backend: &mut KmsBackend,
    extent: (u16, u16),
    pixel: [u8; 4],
    hotspot: (u16, u16),
) -> CursorHandle {
    let (width, height) = extent;
    let source = Backend::create_pixmap(backend, None, 32, width, height)
        .expect("create ARGB cursor pixmap");
    Backend::put_image(
        backend,
        None,
        source.as_raw(),
        32,
        width,
        height,
        0,
        0,
        &pixel.repeat(usize::from(width) * usize::from(height)),
    )
    .expect("write ARGB cursor pixels");
    let picture =
        Backend::render_create_picture(backend, None, AnyHandle::Pixmap(source), 0, 0, &[])
            .expect("create ARGB cursor picture")
            .expect("picture handle");
    Backend::render_create_cursor(backend, None, picture, hotspot.0, hotspot.1)
        .expect("create ARGB cursor")
        .expect("cursor handle")
}

pub(crate) fn create_bitmap_cursor(backend: &mut KmsBackend, visible: bool) -> CursorHandle {
    let source = Backend::create_pixmap(backend, None, 1, 1, 1).expect("create source bitmap");
    let source_pixel = if visible { [1, 0, 0, 0] } else { [0; 4] };
    Backend::put_image(backend, None, source.as_raw(), 1, 1, 1, 0, 0, &source_pixel)
        .expect("write source bitmap bit");
    let mask = if visible {
        None
    } else {
        let mask = Backend::create_pixmap(backend, None, 1, 1, 1).expect("create mask bitmap");
        Backend::put_image(backend, None, mask.as_raw(), 1, 1, 1, 0, 0, &[0; 4])
            .expect("write transparent mask bit");
        Some(mask)
    };
    Backend::create_cursor(
        backend,
        None,
        source,
        mask,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
        0,
        0,
    )
    .expect("create bitmap cursor")
}

pub(crate) fn define_root_cursor(backend: &mut KmsBackend, cursor: CursorHandle) {
    Backend::define_cursor(backend, None, backend.core.window_id, cursor.as_raw())
        .expect("define root cursor");
}

pub(crate) fn accepted_live_records(
    recorder: &RecordingAtomicCommitExecutor,
) -> Vec<AtomicCommitRecord> {
    recorder
        .records()
        .into_iter()
        .filter(|record| {
            record.result == ScriptedResult::Ok
                && !record
                    .flags
                    .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
        })
        .collect()
}

pub(crate) fn record_property(
    record: &AtomicCommitRecord,
    property: ::drm::control::property::Handle,
) -> Option<u64> {
    record
        .properties
        .iter()
        .find(|update| update.property == property)
        .map(|update| update.value)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CompletionKey {
    pub(crate) device_fd: RawFd,
    pub(crate) crtc: ::drm::control::crtc::Handle,
    pub(crate) commit_id: u64,
}

pub(crate) fn retire_pending_output(
    events: &mut AtomicEventFixture,
    delivered: &mut HashSet<CompletionKey>,
    output_idx: usize,
    frame: u32,
) -> u64 {
    let layout = &events.backend.platform.outputs[output_idx];
    let crtc = layout.output.crtc;
    let device_key = layout.key.device_key;
    let device = events
        .backend
        .platform
        .device_for_key(device_key)
        .expect("test output has an owning device");
    let device_fd = device.device.as_fd().as_raw_fd();
    let recorder = events
        .recorders
        .get(&device_fd)
        .expect("recording executor exists for output device");
    let matching: Vec<_> = accepted_live_records(recorder)
        .into_iter()
        .filter_map(|record| {
            let id = accepted_event_id(&record)?;
            (record.device_fd == device_fd
                && record.events.iter().any(|event| event.crtc == crtc)
                && !delivered.contains(&CompletionKey {
                    device_fd,
                    crtc,
                    commit_id: id,
                }))
            .then_some(id)
        })
        .collect();
    let id = match matching.as_slice() {
        [] => panic!("no accepted unretired commit for output"),
        [id] => *id,
        _ => panic!("more than one accepted unretired commit for output"),
    };

    events.queue_commit_completion(
        device_fd,
        crtc,
        id,
        frame,
        Duration::from_micros(u64::from(frame)),
    );
    Backend::on_page_flip_ready(&mut events.backend, &mut events.state, device_fd);
    delivered.insert(CompletionKey {
        device_fd,
        crtc,
        commit_id: id,
    });
    id
}

pub(crate) fn assert_no_cursor_resources(backend: &KmsBackend) {
    assert!(
        !backend.platform.any_cursor_commit_in_flight(),
        "cursor commit still in flight",
    );
    for (output_idx, output) in backend.platform.outputs.iter().enumerate() {
        assert!(
            backend
                .platform
                .for_tests_atomic_cursor_pending_position(
                    output.key.device_key,
                    output.output.crtc,
                )
                .is_none(),
            "cursor position still pending for output {output_idx}",
        );
        assert!(
            !backend.platform.cursor_retry_waits_for_flip(output_idx),
            "cursor retry still waiting for output {output_idx}",
        );
    }
}

fn has_unretired_accepted_event(
    events: &AtomicEventFixture,
    delivered: &HashSet<CompletionKey>,
    output_idx: usize,
) -> bool {
    let layout = &events.backend.platform.outputs[output_idx];
    let crtc = layout.output.crtc;
    let device_key = layout.key.device_key;
    let device = events
        .backend
        .platform
        .device_for_key(device_key)
        .expect("test output has an owning device");
    let device_fd = device.device.as_fd().as_raw_fd();
    events
        .recorders
        .get(&device_fd)
        .expect("recording executor exists for output device")
        .records()
        .into_iter()
        .any(|record| {
            accepted_event_id(&record).is_some_and(|id| {
                record.device_fd == device_fd
                    && record.events.iter().any(|event| event.crtc == crtc)
                    && !delivered.contains(&CompletionKey {
                        device_fd,
                        crtc,
                        commit_id: id,
                    })
            })
        })
}

pub(crate) fn finish_fixture(
    events: &mut AtomicEventFixture,
    delivered: &mut HashSet<CompletionKey>,
) {
    for output_idx in 0..events.backend.platform.outputs.len() {
        for iteration in 0..8 {
            if !has_unretired_accepted_event(events, delivered, output_idx) {
                break;
            }
            retire_pending_output(
                events,
                delivered,
                output_idx,
                u32::try_from(iteration + 1).expect("fixture frame fits u32"),
            );
        }
        assert!(
            !has_unretired_accepted_event(events, delivered, output_idx),
            "fixture did not settle",
        );
    }
    for _ in 0..4 {
        Backend::before_block(&mut events.backend);
    }
    events.assert_quiescent();
    assert_no_cursor_resources(&events.backend);
}

#[cfg(test)]
mod tests {
    use super::super::super::commit_identity::{
        recording_scene::RecordingSceneFixture,
        support::{AtomicEventFixture, accepted_event_id},
    };
    use crate::drm::atomic::{
        AtomicCommitKind, AtomicCommitRequest, AtomicCommitSink,
        tests_support::{RecordingAtomicCommitExecutor, ScriptedResult},
    };
    use std::{
        collections::HashSet,
        os::fd::{AsFd as _, AsRawFd as _, RawFd},
        rc::Rc,
    };
    use yserver_core::backend::Backend;

    fn raw<H: ::drm::control::ResourceHandle>(value: u32) -> H {
        ::drm::control::from_u32(value).expect("nonzero test handle")
    }

    fn output_fd(events: &AtomicEventFixture, output_idx: usize) -> RawFd {
        let layout = &events.backend.platform.outputs[output_idx];
        events
            .backend
            .platform
            .device_for_key(layout.key.device_key)
            .expect("test output has an owning device")
            .device
            .as_fd()
            .as_raw_fd()
    }

    #[test]
    // Mutation: define the cursor on a child window instead of the root, or
    // ignore `extent` and always construct a 4x4 ARGB cursor.
    #[ignore = "needs live Vulkan ICD (lavapipe)"]
    fn p2_fixture_cursor_helpers_control_the_effective_cursor() {
        let mut backend = match crate::kms::render::backend::KmsBackend::for_tests_with_vk() {
            Ok(backend) => backend,
            Err(error) => {
                eprintln!("skipping: no Vulkan test device: {error}");
                return;
            }
        };

        let visible = super::create_bitmap_cursor(&mut backend, true);
        super::define_root_cursor(&mut backend, visible);
        assert!(
            !backend.effective_cursor_hidden(),
            "visible cursor record: {:?}",
            backend.cursor_records[&visible.as_raw()],
        );
        assert_eq!(backend.effective_cursor_xid, Some(visible.as_raw()));

        let invisible = super::create_bitmap_cursor(&mut backend, false);
        super::define_root_cursor(&mut backend, invisible);
        assert!(backend.effective_cursor_hidden());
        assert_eq!(backend.effective_cursor_xid, Some(invisible.as_raw()));

        let argb = super::create_argb_cursor(&mut backend, (6, 3), [1, 2, 3, 4], (1, 2));
        let record = backend
            .cursor_records
            .get(&argb.as_raw())
            .expect("ARGB cursor record");
        assert_eq!(record.width, 6);
        assert_eq!(record.height, 3);
        assert_eq!(record.hot_x, 1);
        assert_eq!(record.hot_y, 2);
    }

    #[test]
    // Mutation: deliver `id + 1` instead of the accepted event identity.
    fn p2_fixture_retire_pending_output_delivers_exactly_the_accepted_id() {
        let mut events = AtomicEventFixture::new(1);
        let output = &events.backend.platform.outputs[0];
        let device_key = output.key.device_key;
        let crtc = output.output.crtc;
        events
            .backend
            .platform
            .for_tests_seed_atomic_cursor_position(0, raw(51), raw(61), raw(62), 100, 100, 0, 0);
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        Backend::maybe_composite(&mut events.backend).expect("submit cursor-only move");

        let fd = output_fd(&events, 0);
        let recorder = &events.recorders[&fd];
        let accepted = super::accepted_live_records(recorder);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].kind, AtomicCommitKind::CursorOnly);
        let accepted_id = accepted_event_id(&accepted[0]).expect("accepted cursor event id");

        let mut delivered = HashSet::new();
        assert_eq!(
            super::retire_pending_output(&mut events, &mut delivered, 0, 1),
            accepted_id,
        );
        assert!(!events.backend.platform.any_cursor_commit_in_flight());
        assert_eq!(
            events.backend.platform.devices[0]
                .atomic_commit_sink
                .for_tests_pending_page_flip_entry_count(),
            0,
        );
        let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::retire_pending_output(&mut events, &mut delivered, 0, 2)
        }))
        .expect_err("a completion cannot be delivered twice");
        let message = second
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| second.downcast_ref::<&str>().copied());
        assert_eq!(message, Some("no accepted unretired commit for output"));
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd,
            crtc,
            commit_id: accepted_id,
        }));
        super::finish_fixture(&mut events, &mut delivered);
        assert!(
            events
                .backend
                .platform
                .for_tests_atomic_cursor_pending_position(device_key, crtc)
                .is_none()
        );
    }

    #[test]
    // Mutation: key delivered completions by numeric ID alone, dropping the
    // device identity even though each device starts its own ID sequence.
    fn p2_fixture_retire_pending_output_qualifies_equal_ids_by_device() {
        let mut events = AtomicEventFixture::new(2);
        for (idx, plane, crtc_x, crtc_y) in [(0, 51, 61, 62), (1, 52, 63, 64)] {
            events
                .backend
                .platform
                .for_tests_seed_atomic_cursor_position(
                    idx,
                    raw(plane),
                    raw(crtc_x),
                    raw(crtc_y),
                    100,
                    100,
                    0,
                    0,
                );
        }
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        Backend::maybe_composite(&mut events.backend).expect("submit cursor-only moves");

        let fd0 = output_fd(&events, 0);
        let fd1 = output_fd(&events, 1);
        let records0 = super::accepted_live_records(&events.recorders[&fd0]);
        let records1 = super::accepted_live_records(&events.recorders[&fd1]);
        assert_eq!(records0.len(), 1);
        assert_eq!(records1.len(), 1);
        assert_eq!(records0[0].kind, AtomicCommitKind::CursorOnly);
        assert_eq!(records1[0].kind, AtomicCommitKind::CursorOnly);
        assert_eq!(accepted_event_id(&records0[0]), Some(1));
        assert_eq!(accepted_event_id(&records1[0]), Some(1));

        let crtc0 = events.backend.platform.outputs[0].output.crtc;
        let crtc1 = events.backend.platform.outputs[1].output.crtc;
        assert_eq!(crtc0, crtc1);
        let mut delivered = HashSet::new();
        assert_eq!(
            super::retire_pending_output(&mut events, &mut delivered, 0, 1),
            1,
        );
        assert_eq!(
            super::retire_pending_output(&mut events, &mut delivered, 1, 2),
            1,
        );
        assert!(!events.backend.platform.any_cursor_commit_in_flight());
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd0,
            crtc: crtc0,
            commit_id: 1,
        }));
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd1,
            crtc: crtc1,
            commit_id: 1,
        }));
        super::finish_fixture(&mut events, &mut delivered);
    }

    #[test]
    #[ignore = "requires the lavapipe GPU scene fixture"]
    // Mutation: key grouped deliveries by `(fd, id)`, suppressing the second
    // target CRTC that shares the accepted cookie.
    fn p2_fixture_retire_pending_output_accepts_a_group_cookie_per_crtc() {
        let Some(mut fixture) =
            RecordingSceneFixture::new(2).expect("two-output recording scene fixture construction")
        else {
            return;
        };
        let direct = fixture.install_direct_present(1, 0);
        for attempt in 0..8 {
            let accepted = Backend::try_present_direct(
                &mut fixture.events.backend,
                direct.candidate,
                direct.event.clone(),
            )
            .expect("direct candidate through Backend");
            assert_eq!(accepted, attempt == 7, "drive the existing probation");
        }

        let fd = fixture.kms_fd(0);
        let record = fixture.events.recorders[&fd]
            .records()
            .into_iter()
            .find(|record| {
                accepted_event_id(record).is_some()
                    && record.events.len() == 2
                    && record.events[0].crtc != record.events[1].crtc
            })
            .expect("accepted grouped record targets both CRTCs");
        let id = accepted_event_id(&record).expect("grouped record has an accepted ID");
        let crtc0 = fixture.events.backend.platform.outputs[0].output.crtc;
        let crtc1 = fixture.events.backend.platform.outputs[1].output.crtc;
        assert!(record.events.iter().any(|event| event.crtc == crtc0));
        assert!(record.events.iter().any(|event| event.crtc == crtc1));

        let mut delivered = HashSet::new();
        assert_eq!(
            super::retire_pending_output(&mut fixture.events, &mut delivered, 0, 1),
            id,
        );
        assert_eq!(
            super::retire_pending_output(&mut fixture.events, &mut delivered, 1, 2),
            id,
        );
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd,
            crtc: crtc0,
            commit_id: id,
        }));
        assert!(delivered.contains(&super::CompletionKey {
            device_fd: fd,
            crtc: crtc1,
            commit_id: id,
        }));

        let completed = Backend::drain_completed_present_events(&mut fixture.events.backend);
        assert_eq!(
            completed
                .iter()
                .map(|event| event.present_id)
                .collect::<Vec<_>>(),
            [direct.candidate.present_id],
        );
        assert!(Backend::drain_retired_present_idle_events(&mut fixture.events.backend).is_empty());

        let mut successor = direct.candidate;
        successor.present_id += 1;
        successor.valid_region_xid = 0x55;
        let mut successor_event = direct.event.clone();
        successor_event.present_id = successor.present_id;
        successor_event.serial = successor_event.serial.wrapping_add(1);
        assert!(
            !Backend::try_present_direct(&mut fixture.events.backend, successor, successor_event,)
                .expect("ineligible successor requests a composed unflip"),
            "the successor must request a composed unflip",
        );
        assert!(fixture.events.backend.pstate.scanout_m2.unflip_requested);

        let unflip_offset = fixture.events.recorders[&fd].records().len();
        Backend::maybe_composite(&mut fixture.events.backend).expect("submit grouped unflip");
        let unflip_record = fixture.events.recorders[&fd]
            .records()
            .into_iter()
            .skip(unflip_offset)
            .find(|record| record.events.len() == 2 && accepted_event_id(record).is_some())
            .expect("accepted grouped unflip record");
        let unflip_id = accepted_event_id(&unflip_record).expect("unflip has an accepted ID");
        assert_ne!(unflip_id, id);
        assert_eq!(
            super::retire_pending_output(&mut fixture.events, &mut delivered, 0, 3),
            unflip_id,
        );
        assert_eq!(
            super::retire_pending_output(&mut fixture.events, &mut delivered, 1, 4),
            unflip_id,
        );
        assert!(fixture.events.backend.pstate.scanout_m2.current.is_none());
        assert!(fixture.events.backend.present_source_pins.is_empty());
        assert!(Backend::drain_completed_present_events(&mut fixture.events.backend).is_empty());
        let idled = Backend::drain_retired_present_idle_events(&mut fixture.events.backend);
        assert_eq!(
            idled
                .iter()
                .map(|event| event.present_id)
                .collect::<Vec<_>>(),
            [direct.candidate.present_id],
        );

        super::finish_fixture(&mut fixture.events, &mut delivered);
        fixture.finish_after_normal_drain();
    }

    #[test]
    // Mutation: include TEST_ONLY records in accepted live records, or match
    // `record_property` by object handle instead of property handle.
    fn p2_fixture_accepted_live_records_exclude_test_only_and_failed_commits() {
        let mut events = AtomicEventFixture::new(2);
        let recorder = Rc::new(RecordingAtomicCommitExecutor::default());
        let fds: Vec<_> = events
            .backend
            .platform
            .devices
            .iter()
            .map(|device| device.device.as_fd().as_raw_fd())
            .collect();
        for device in &mut events.backend.platform.devices {
            let executor: Rc<dyn crate::drm::atomic::AtomicCommitExecutor> = recorder.clone();
            device.atomic_commit_sink = Rc::new(AtomicCommitSink::new(executor));
        }
        events.recorders.clear();
        for fd in &fds {
            events.recorders.insert(*fd, Rc::clone(&recorder));
        }
        for (idx, plane, crtc_x, crtc_y) in [(0, 51, 61, 62), (1, 52, 63, 64)] {
            events
                .backend
                .platform
                .for_tests_seed_atomic_cursor_position(
                    idx,
                    raw(plane),
                    raw(crtc_x),
                    raw(crtc_y),
                    100,
                    100,
                    0,
                    0,
                );
        }
        recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        Backend::maybe_composite(&mut events.backend).expect("submit cursor-only moves");

        let test_only_device = &events.backend.platform.devices[0];
        test_only_device
            .atomic_commit_sink
            .test_only(
                &test_only_device.device,
                AtomicCommitKind::CursorOnly,
                AtomicCommitRequest::new(),
            )
            .expect("inject a TEST_ONLY record");

        let records = recorder.records();
        assert!(records.iter().any(|record| {
            record.kind == AtomicCommitKind::CursorOnly
                && record.result
                    == ScriptedResult::Error {
                        kind: std::io::ErrorKind::InvalidInput,
                        raw_os_error: Some(libc::EINVAL),
                    }
        }));
        assert!(records.iter().any(|record| {
            record
                .flags
                .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
        }));
        let accepted = super::accepted_live_records(&recorder);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].kind, AtomicCommitKind::CursorOnly);
        assert_eq!(accepted[0].result, ScriptedResult::Ok);
        let crtc_x = raw(63);
        let x_value = accepted[0]
            .properties
            .iter()
            .find(|update| update.property == crtc_x)
            .expect("accepted record carries CRTC_X")
            .value;
        assert_eq!(super::record_property(&accepted[0], crtc_x), Some(x_value));
        assert_eq!(
            super::record_property(&accepted[0], raw(52)),
            None,
            "plane object handle is not a property handle",
        );

        let mut delivered = HashSet::new();
        super::finish_fixture(&mut events, &mut delivered);
    }

    #[test]
    #[should_panic(expected = "cursor commit still in flight")]
    // Mutation: skip the in-flight assertion in `assert_no_cursor_resources`.
    fn p2_fixture_finish_rejects_an_unretired_cursor_commit() {
        let mut events = AtomicEventFixture::new(1);
        events
            .backend
            .platform
            .for_tests_seed_atomic_cursor_position(0, raw(51), raw(61), raw(62), 100, 100, 0, 0);
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        Backend::maybe_composite(&mut events.backend).expect("submit cursor-only move");
        let assertion = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::assert_no_cursor_resources(&events.backend);
        }));
        let mut delivered = HashSet::new();
        super::finish_fixture(&mut events, &mut delivered);
        std::panic::resume_unwind(
            assertion.expect_err("an unretired cursor commit must be rejected"),
        );
    }
}
