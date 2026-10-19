//! Scene-frame recovery when a software-cursor hide is rejected.

use std::{collections::HashSet, io, path::PathBuf, rc::Rc};

use super::{
    round_a::damage_cursor_root,
    round_b_frames::install_copied_recording_route_with_hotspot,
    round_b_producers::{begin_hw_cursor, create_argb_cursor_extent},
    support::AtomicCursorFixture,
};
use crate::{
    drm::atomic::{
        AtomicCommitKind,
        tests_support::{AtomicCommitRecord, RecordingAtomicCommitExecutor, ScriptedResult},
    },
    kms::render::{
        platform::{
            cursor_recovery::RecordingCursorRecoveryReporter,
            cursor_state::{CursorBinding, CursorFallbackState},
        },
        scene::{CursorAcceptedMode, CursorPrimaryContent},
    },
};
use ::drm::control::{AtomicCommitFlags, property};
use yserver_core::backend::Backend;

fn make_fixture(
    copied: bool,
) -> Option<(
    AtomicCursorFixture,
    Rc<RecordingCursorRecoveryReporter>,
    Rc<RecordingAtomicCommitExecutor>,
)> {
    let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
    let mut fixture =
        AtomicCursorFixture::new_with_hotspot_properties_and_reporter(Rc::clone(&reporter))
            .expect("build cursor frame fixture")?;
    let atomic = if copied {
        install_copied_recording_route_with_hotspot(&mut fixture, true)
    } else {
        fixture.recorders.atomic.clone()
    };
    Some((fixture, reporter, atomic))
}

fn cursor_properties(
    fixture: &AtomicCursorFixture,
) -> (Vec<(&'static str, property::Handle)>, property::Handle) {
    let output = &fixture.backend.platform.outputs[0];
    let properties = fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(output.output.crtc))
        .expect("output cursor properties");
    let mut handles = vec![
        ("fb_id", properties.fb_id),
        ("crtc_id", properties.crtc_id),
        ("crtc_x", properties.crtc_x),
        ("crtc_y", properties.crtc_y),
        ("crtc_w", properties.crtc_w),
        ("crtc_h", properties.crtc_h),
        ("src_x", properties.src_x),
        ("src_y", properties.src_y),
        ("src_w", properties.src_w),
        ("src_h", properties.src_h),
    ];
    if let (Some(x), Some(y)) = (properties.hotspot_x, properties.hotspot_y) {
        handles.push(("hotspot_x", x));
        handles.push(("hotspot_y", y));
    }
    (
        handles,
        fixture.backend.platform.outputs[0].output.plane_fb_id_prop,
    )
}

fn live_records(atomic: &RecordingAtomicCommitExecutor) -> Vec<AtomicCommitRecord> {
    atomic
        .records()
        .into_iter()
        .filter(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .collect()
}

fn cursor_dump_files() -> HashSet<PathBuf> {
    std::fs::read_dir(".")
        .expect("read test working directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("yserver-") && name.ends_with(".ppm"))
        })
        .collect()
}

fn ppm_first_rgb_pixel(path: &std::path::Path) -> Vec<u8> {
    let bytes = std::fs::read(path).expect("read cursor dump PPM");
    let payload_start = bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(2)
        .map(|(index, _)| index + 1)
        .expect("PPM header has three lines");
    bytes[payload_start..payload_start + 3].to_vec()
}

fn property_value(record: &AtomicCommitRecord, property: property::Handle) -> Option<u64> {
    record
        .properties
        .iter()
        .find_map(|update| (update.property == property).then_some(update.value))
}

fn applied_binding(fixture: &AtomicCursorFixture) -> Option<CursorBinding> {
    let output = &fixture.backend.platform.outputs[0];
    fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
        .and_then(|state| state.applied)
}

fn sprite_user_count(
    fixture: &AtomicCursorFixture,
    lease: crate::kms::cursor_plane::sprites::SpriteBufferLease,
) -> usize {
    let output = &fixture.backend.platform.outputs[0];
    fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.sprite_user_count(lease))
        .unwrap_or(0)
}

pub(super) fn read_output_pixel(fixture: &mut AtomicCursorFixture, x: i32, y: i32) -> [u8; 4] {
    crate::kms::render::backend::read_scanout_region(
        &mut fixture.backend,
        ash::vk::Rect2D {
            offset: ash::vk::Offset2D { x, y },
            extent: ash::vk::Extent2D {
                width: 1,
                height: 1,
            },
        },
        crate::kms::render::backend::ScanoutReadSelection::PermissiveDump,
    )
    .expect("read the composed output pixel")
    .try_into()
    .expect("one BGRA pixel")
}

fn settle_copied_frame(fixture: &mut AtomicCursorFixture, job_id: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !fixture
        .recorders
        .render_completion_readiness
        .observed_ready_jobs()
        .contains(&job_id)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "copied source frame {job_id} did not complete",
        );
        assert!(
            fixture
                .backend
                .platform
                .for_tests_pending_scanout_render_completion_ids()
                .contains(&job_id)
        );
        Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
        std::thread::yield_now();
    }
    fixture
        .recorders
        .render_completion_readiness
        .release_ready_job(job_id);
    Backend::on_scanout_render_completion(&mut fixture.backend, &mut fixture.state);
}

fn record_next_scene_frame(fixture: &mut AtomicCursorFixture, copied: bool) {
    Backend::maybe_composite(&mut fixture.backend).expect("submit scene frame");
    if copied {
        let job = *fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_ids()
            .first()
            .expect("copied scene frame has a render completion");
        settle_copied_frame(fixture, job);
    }
}

fn drive_scene_retry_deadline_through_backend(fixture: &mut AtomicCursorFixture) {
    let Some(deadline) = fixture.backend.scene.earliest_retry_deadline() else {
        return;
    };
    while std::time::Instant::now() < deadline {
        let _ = Backend::maybe_composite(&mut fixture.backend);
        std::thread::yield_now();
    }
}

fn script_hide_then_detach(atomic: &RecordingAtomicCommitExecutor, detach_succeeds: bool) {
    // Live hide failure, full TEST_ONLY failure, cursor-stripped TEST_ONLY
    // success, then the explicit-detach retry and its optional attribution.
    atomic.script_result(Err(io::Error::from_raw_os_error(libc::EINVAL)));
    atomic.script_result(Err(io::Error::from_raw_os_error(libc::EINVAL)));
    atomic.script_result(Ok(()));
    if detach_succeeds {
        atomic.script_result(Ok(()));
    } else {
        atomic.script_result(Err(io::Error::from_raw_os_error(libc::EINVAL)));
        atomic.script_result(Err(io::Error::from_raw_os_error(libc::EINVAL)));
        atomic.script_result(Ok(()));
    }
}

fn run_hide_failure_case(copied: bool, detach_succeeds: bool) {
    let Some((mut fixture, reporter, atomic)) = make_fixture(copied) else {
        return;
    };
    if copied {
        fixture
            .recorders
            .render_completion_readiness
            .hold_ready_jobs();
        fixture.define_visible_cursor();
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
        damage_cursor_root(&mut fixture);
        record_next_scene_frame(&mut fixture, true);
        fixture.retire_output(0, 1);
    } else {
        begin_hw_cursor(&mut fixture);
    }
    let baseline_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted hardware baseline receipt");
    assert_eq!(baseline_receipt.mode, CursorAcceptedMode::Hardware);
    assert_eq!(
        baseline_receipt.primary_cursor,
        CursorPrimaryContent::Cursorless
    );
    assert!(
        !fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    let old_binding = applied_binding(&fixture).expect("accepted hardware cursor binding");
    assert_eq!(sprite_user_count(&fixture, old_binding.lease), 1);
    let before_failure = live_records(&atomic).len();

    let root = fixture.backend.core.window_id;
    let oversized =
        create_argb_cursor_extent(&mut fixture.backend, [0x00, 0x20, 0xff, 0xff], 65, 65, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, oversized.as_raw())
        .expect("select the software-sized cursor through Backend");
    damage_cursor_root(&mut fixture);
    script_hide_then_detach(&atomic, detach_succeeds);
    record_next_scene_frame(&mut fixture, copied);

    let (cursor, primary_fb_property) = cursor_properties(&fixture);
    let rejected_live = &live_records(&atomic)[before_failure..];
    assert_eq!(rejected_live.len(), 2);
    assert_eq!(rejected_live[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(
        rejected_live[0].result,
        ScriptedResult::Error {
            kind: io::ErrorKind::InvalidInput,
            raw_os_error: Some(libc::EINVAL),
        },
        "the original software-cursor hide is rejected",
    );
    assert_eq!(rejected_live[1].kind, AtomicCommitKind::SceneWithCursor);
    let original_primary = property_value(&rejected_live[0], primary_fb_property)
        .expect("rejected hide carries a primary framebuffer");
    let retry_primary = property_value(&rejected_live[1], primary_fb_property)
        .expect("detach retry carries the same primary framebuffer");
    assert_eq!(
        original_primary, retry_primary,
        "retry is the same primary frame"
    );
    let fb_id = cursor.iter().find(|(name, _)| *name == "fb_id").unwrap().1;
    let crtc_id = cursor
        .iter()
        .find(|(name, _)| *name == "crtc_id")
        .unwrap()
        .1;
    assert!(
        rejected_live[1]
            .properties
            .iter()
            .any(|update| update.property == fb_id && update.value == 0),
        "the retry explicitly detaches FB_ID",
    );
    assert!(
        rejected_live[1]
            .properties
            .iter()
            .any(|update| update.property == crtc_id && update.value == 0),
        "the retry explicitly detaches CRTC_ID",
    );
    assert_eq!(
        rejected_live[1].result,
        if detach_succeeds {
            ScriptedResult::Ok
        } else {
            ScriptedResult::Error {
                kind: io::ErrorKind::InvalidInput,
                raw_os_error: Some(libc::EINVAL),
            }
        },
    );
    if rejected_live[1].result == ScriptedResult::Ok {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted intermediate retry has a cursor-content receipt");
        let retry_detached = property_value(&rejected_live[1], fb_id) == Some(0)
            && property_value(&rejected_live[1], crtc_id) == Some(0);
        let binding_remains = !retry_detached;
        assert!(
            !matches!(
                receipt.primary_cursor,
                CursorPrimaryContent::Software { .. }
            ) || !binding_remains,
            "unsafe accepted intermediate primary has software cursor content while the previous hardware binding remains: record={:?}, receipt={receipt:?}",
            rejected_live[1],
        );
    }

    if detach_succeeds {
        let receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted scene frame with explicit detach");
        assert_eq!(receipt.mode, CursorAcceptedMode::Software);
        assert!(matches!(
            receipt.primary_cursor,
            CursorPrimaryContent::Software { .. }
        ));
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        assert_eq!(applied_binding(&fixture), None);
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 1);
        fixture.retire_output(0, 2);
        assert!(
            fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(0)
        );
        let under_cursor = read_output_pixel(&mut fixture, 110, 100);
        let beside_cursor = read_output_pixel(&mut fixture, 200, 200);
        assert_ne!(
            &under_cursor[..3],
            &beside_cursor[..3],
            "the accepted software primary contains the opaque cursor pixels",
        );
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 0);
        assert_eq!(reporter.calls(), 0);
        let accepted_retry = &live_records(&atomic)[before_failure + 1];
        assert!(
            accepted_retry
                .properties
                .iter()
                .any(|update| update.property == fb_id && update.value == 0)
                && accepted_retry
                    .properties
                    .iter()
                    .any(|update| update.property == crtc_id && update.value == 0),
            "the accepted SW primary is paired with the actual hardware detach",
        );
    } else {
        assert_eq!(
            fixture.backend.scene.for_tests_cursor_commit_receipt(0),
            Some(baseline_receipt)
        );
        assert!(!fixture.backend.scene.has_pending_page_flip(0));
        assert!(
            !fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(0)
        );
        assert_eq!(applied_binding(&fixture), Some(old_binding));
        assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
        let retained_state = fixture
            .backend
            .platform
            .device_for_key(fixture.backend.platform.outputs[0].key.device_key)
            .and_then(|device| {
                device
                    .cursor
                    .crtc_states
                    .get(&fixture.backend.platform.outputs[0].output.crtc)
            })
            .expect("retained cursor state");
        assert!(retained_state.intent.is_none());
        assert!(retained_state.in_flight.is_none());
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 1);
        assert_eq!(reporter.calls(), 1);
        let old_primary_under_cursor = read_output_pixel(&mut fixture, 110, 100);
        let old_primary_beside_cursor = read_output_pixel(&mut fixture, 200, 200);
        assert_eq!(
            &old_primary_under_cursor[..3],
            &old_primary_beside_cursor[..3],
            "both rejected attempts leave the cursorless old primary on screen",
        );

        // The replacement is a real cursorless render while the old hardware
        // binding remains accepted. It carries neither cursor properties nor
        // the rejected software frame's content annotation.
        record_next_scene_frame(&mut fixture, copied);
        let replacement = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted cursorless replacement receipt");
        assert_eq!(replacement.primary_cursor, CursorPrimaryContent::Cursorless);
        assert_eq!(replacement.mode, CursorAcceptedMode::Hardware);
        let retained_state = fixture
            .backend
            .platform
            .device_for_key(fixture.backend.platform.outputs[0].key.device_key)
            .and_then(|device| {
                device
                    .cursor
                    .crtc_states
                    .get(&fixture.backend.platform.outputs[0].output.crtc)
            })
            .expect("retained cursor state after replacement acceptance");
        assert_eq!(
            retained_state.fallback,
            crate::kms::render::platform::cursor_state::CursorFallbackState::RetainHardware,
        );
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        let replacement_record = live_records(&atomic)
            .last()
            .cloned()
            .expect("replacement commit");
        assert_eq!(replacement_record.kind, AtomicCommitKind::Scene);
        assert!(replacement_record.properties.iter().all(|update| {
            !cursor
                .iter()
                .any(|(_, property)| *property == update.property)
        }));
        fixture.retire_output(0, 3);
        assert!(
            !fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(0)
        );
        let replacement_under_cursor = read_output_pixel(&mut fixture, 110, 100);
        let replacement_beside_cursor = read_output_pixel(&mut fixture, 200, 200);
        assert_eq!(
            &replacement_under_cursor[..3],
            &replacement_beside_cursor[..3],
            "the cursorless replacement primary has no software cursor pixels",
        );
        assert_eq!(applied_binding(&fixture), Some(old_binding));
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 1);
        assert_eq!(reporter.calls(), 1);
    }

    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0,
    );
    fixture
        .backend
        .disable_output()
        .expect("Backend teardown detaches and drains the retained cursor");
    fixture.assert_cursor_quiescent();
    assert!(!fixture.backend.scene.has_pending_page_flips());
    let mut expected_sequence = vec![AtomicCommitKind::SceneWithCursor; 3];
    if !copied {
        expected_sequence.push(AtomicCommitKind::Scene);
        if !detach_succeeds {
            expected_sequence.push(AtomicCommitKind::Scene);
        }
    } else if !detach_succeeds {
        expected_sequence.push(AtomicCommitKind::Scene);
    }
    assert_eq!(
        live_records(&atomic)
            .iter()
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        expected_sequence,
        "baseline, failed hide, explicit detach retry, replacement, and teardown sequence",
    );
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (run and seen to fail): M1 retries software pixels without detach, M2 publishes Software while the old binding remains, M3 reuses Software metadata for the cursorless replacement.
fn atomic_cursor_only_hide_failure_retries_detach_or_discards_software_frame() {
    for copied in [false, true] {
        run_hide_failure_case(copied, true);
        run_hide_failure_case(copied, false);
    }
}

fn script_riding_rejection_then_retry(
    atomic: &RecordingAtomicCommitExecutor,
    retry: io::Result<()>,
) {
    atomic.script_result(Err(io::Error::from_raw_os_error(libc::EINVAL)));
    atomic.script_result(Err(io::Error::from_raw_os_error(libc::EINVAL)));
    atomic.script_result(Ok(()));
    atomic.script_result(retry);
}

fn establish_hw_cursor(fixture: &mut AtomicCursorFixture, copied: bool) -> CursorBinding {
    if copied {
        fixture
            .recorders
            .render_completion_readiness
            .hold_ready_jobs();
        fixture.define_visible_cursor();
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
        damage_cursor_root(fixture);
        record_next_scene_frame(fixture, true);
        fixture.retire_output(0, 1);
    } else {
        begin_hw_cursor(fixture);
    }
    applied_binding(fixture).expect("accepted initial hardware cursor binding")
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail): accept the retry before its result; enter Software before the explicit detach; strip the primary or IN_FENCE_FD fields from the retry; or settle the rejected cursor lease as accepted.
fn atomic_cursor_only_riding_failure_retries_primary_then_detaches() {
    for copied in [false, true] {
        let Some((mut fixture, reporter, atomic)) = make_fixture(copied) else {
            return;
        };
        let old_binding = establish_hw_cursor(&mut fixture, copied);
        let old_receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted hardware cursor baseline receipt");
        assert_eq!(old_receipt.mode, CursorAcceptedMode::Hardware);
        assert_eq!(old_receipt.primary_cursor, CursorPrimaryContent::Cursorless);
        let old_under = read_output_pixel(&mut fixture, 110, 100);
        let old_beside = read_output_pixel(&mut fixture, 200, 200);
        assert_eq!(&old_under[..3], &old_beside[..3]);

        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 130, 110);
        damage_cursor_root(&mut fixture);
        let before = live_records(&atomic).len();
        script_riding_rejection_then_retry(&atomic, Ok(()));
        record_next_scene_frame(&mut fixture, copied);

        let live = live_records(&atomic);
        assert_eq!(live.len(), before + 2, "rejected ride and primary retry");
        let rejected = &live[before];
        let retry = &live[before + 1];
        assert_eq!(rejected.kind, AtomicCommitKind::SceneWithCursor);
        assert_eq!(retry.kind, AtomicCommitKind::Scene);
        assert_eq!(retry.result, ScriptedResult::Ok);
        let output = &fixture.backend.platform.outputs[0].output;
        let primary_fb = output.plane_fb_id_prop;
        let original_primary = rejected
            .properties
            .iter()
            .find(|update| update.object == output.plane.into() && update.property == primary_fb)
            .map(|update| update.value)
            .expect("original primary FB");
        assert_eq!(
            retry
                .properties
                .iter()
                .find(|update| update.object == output.plane.into() && update.property == primary_fb)
                .map(|update| update.value),
            Some(original_primary),
            "cursor-stripped retry preserves the original primary FB",
        );
        let in_fence = fixture.backend.platform.outputs[0]
            .output
            .plane_in_fence_fd_prop
            .expect("fixture IN_FENCE_FD property");
        assert!(
            rejected
                .properties
                .iter()
                .any(|update| update.property == in_fence),
            "initial request carries IN_FENCE_FD",
        );
        assert!(
            retry
                .properties
                .iter()
                .any(|update| update.property == in_fence),
            "primary retry preserves IN_FENCE_FD",
        );
        let (cursor, _) = cursor_properties(&fixture);
        assert!(retry.properties.iter().all(|update| {
            !cursor
                .iter()
                .any(|(_, property)| *property == update.property)
        }));
        assert_eq!(applied_binding(&fixture), Some(old_binding));
        assert_eq!(
            fixture
                .backend
                .scene
                .for_tests_cursor_commit_receipt(0)
                .expect("accepted cursorless primary receipt")
                .mode,
            CursorAcceptedMode::Hardware,
            "the accepted retry keeps ownership with the previous HW binding",
        );
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        assert!(
            !fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(0)
        );
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 1);
        assert_eq!(reporter.calls(), 0);
        let output = &fixture.backend.platform.outputs[0];
        let fallback = fixture
            .backend
            .platform
            .device_for_key(output.key.device_key)
            .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
            .map(|state| state.fallback);
        assert!(
            matches!(fallback, Some(CursorFallbackState::DetachPending { .. })),
            "the accepted cursorless primary keeps HW ownership until detach acceptance: fallback={fallback:?}",
        );

        fixture.retire_output(0, 2);
        let detach = live_records(&atomic)
            .last()
            .cloned()
            .expect("pending detach is submitted after primary retirement");
        assert_eq!(
            detach.kind,
            AtomicCommitKind::CursorOnly,
            "{:?}",
            live_records(&atomic)
        );
        assert_eq!(detach.result, ScriptedResult::Ok);
        assert_eq!(detach.events.len(), 1);
        let fb_id = cursor.iter().find(|(name, _)| *name == "fb_id").unwrap().1;
        let crtc_id = cursor
            .iter()
            .find(|(name, _)| *name == "crtc_id")
            .unwrap()
            .1;
        assert_eq!(property_value(&detach, fb_id), Some(0));
        assert_eq!(property_value(&detach, crtc_id), Some(0));
        assert_eq!(applied_binding(&fixture), None);
        let output = &fixture.backend.platform.outputs[0];
        assert!(matches!(
            fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                .map(|state| state.fallback),
            Some(CursorFallbackState::Software { .. })
        ));
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 1);
        assert_eq!(reporter.calls(), 0);

        fixture.retire_output(0, 3);
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 0);
        Backend::maybe_composite(&mut fixture.backend).expect("paint software after detach");
        if copied {
            settle_all_copied_frames(&mut fixture);
        }
        let software = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("software cursor frame receipt");
        assert_eq!(software.mode, CursorAcceptedMode::Software);
        assert!(matches!(
            software.primary_cursor,
            CursorPrimaryContent::Software { .. }
        ));
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        fixture.retire_output(0, 4);
        assert!(
            fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(0)
        );
        let software_under = read_output_pixel(&mut fixture, 130, 110);
        let software_beside = read_output_pixel(&mut fixture, 131, 110);
        assert_ne!(&software_under[..3], &software_beside[..3]);
        assert_eq!(reporter.calls(), 0);
        assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
        assert_eq!(
            fixture
                .backend
                .platform
                .for_tests_pending_scanout_render_completion_count(),
            0,
        );
        fixture.backend.disable_output().expect("Backend teardown");
        fixture.assert_cursor_quiescent();
        assert!(!fixture.backend.scene.has_pending_page_flips());
        let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail): report a cursor retry as accepted after its primary fails; erase the original primary on retry; or swallow the retry's ENOMEM as a successful presentation.
fn atomic_cursor_only_retry_failure_keeps_original_flip_error_path() {
    for copied in [false, true] {
        let Some((mut fixture, reporter, atomic)) = make_fixture(copied) else {
            return;
        };
        let old_binding = establish_hw_cursor(&mut fixture, copied);
        let baseline = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("baseline hardware receipt");
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 130, 110);
        damage_cursor_root(&mut fixture);
        let before = live_records(&atomic).len();
        script_riding_rejection_then_retry(
            &atomic,
            Err(io::Error::from_raw_os_error(libc::ENOMEM)),
        );
        record_next_scene_frame(&mut fixture, copied);
        let live = live_records(&atomic);
        assert_eq!(
            live.len(),
            before + 2,
            "rejected riding frame and failed retry: {:?}",
            &live[before..]
        );
        assert_eq!(live[before].kind, AtomicCommitKind::SceneWithCursor);
        assert_eq!(live[before + 1].kind, AtomicCommitKind::Scene);
        assert_eq!(
            live[before + 1].result,
            ScriptedResult::Error {
                kind: io::Error::from_raw_os_error(libc::ENOMEM).kind(),
                raw_os_error: Some(libc::ENOMEM),
            },
            "retry failure remains the producer's ENOMEM",
        );
        let primary_fb = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
        assert_eq!(
            property_value(&live[before], primary_fb),
            property_value(&live[before + 1], primary_fb),
            "failed retry keeps the attempted primary identity",
        );
        assert!(!fixture.backend.scene.has_pending_page_flip(0));
        assert_eq!(
            fixture.backend.scene.for_tests_cursor_commit_receipt(0),
            Some(baseline),
            "neither rejected request creates a scene receipt",
        );
        assert_eq!(applied_binding(&fixture), Some(old_binding));
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 1);
        assert!(
            !fixture
                .backend
                .platform
                .cursor_commit_in_flight_for_output(0)
        );
        assert_eq!(reporter.calls(), 0);
        assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
        assert_eq!(
            fixture
                .backend
                .platform
                .for_tests_pending_scanout_render_completion_count(),
            0,
        );

        fixture.backend.disable_output().expect("Backend teardown");
        fixture.assert_cursor_quiescent();
        assert!(!fixture.backend.scene.has_pending_page_flips());
        let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(ledger.end_state(), (0, 0, 0));

        // An ENOSPC bandwidth rejection and a commit-phase ENOMEM are not
        // attributed to the cursor when TEST_ONLY accepts the full request.
        for errno in [libc::ENOSPC, libc::ENOMEM] {
            let Some((mut bandwidth_fixture, _, bandwidth_atomic)) = make_fixture(copied) else {
                return;
            };
            let bandwidth_binding = establish_hw_cursor(&mut bandwidth_fixture, copied);
            Backend::warp_pointer_root(
                &mut bandwidth_fixture.backend,
                &mut bandwidth_fixture.state,
                130,
                110,
            );
            damage_cursor_root(&mut bandwidth_fixture);
            let bandwidth_before = live_records(&bandwidth_atomic).len();
            bandwidth_atomic.script_result(Err(io::Error::from_raw_os_error(errno)));
            record_next_scene_frame(&mut bandwidth_fixture, copied);
            let after_bandwidth = live_records(&bandwidth_atomic);
            let cursor_only_progress = !copied;
            assert_eq!(
                after_bandwidth.len(),
                bandwidth_before + if cursor_only_progress { 2 } else { 1 },
            );
            assert_eq!(
                after_bandwidth[bandwidth_before].kind,
                AtomicCommitKind::SceneWithCursor
            );
            assert_eq!(
                after_bandwidth[bandwidth_before].result,
                ScriptedResult::Error {
                    kind: io::Error::from_raw_os_error(errno).kind(),
                    raw_os_error: Some(errno),
                },
                "unattributed producer failure {errno} follows the flip-error path",
            );
            assert!(!bandwidth_fixture.backend.scene.has_pending_page_flip(0));
            if cursor_only_progress {
                assert_eq!(
                    after_bandwidth[bandwidth_before + 1].kind,
                    AtomicCommitKind::CursorOnly,
                    "cursor motion progresses after the primary fails without a scene commit",
                );
                assert_eq!(
                    after_bandwidth[bandwidth_before + 1].result,
                    ScriptedResult::Ok,
                );
                let moved_binding = applied_binding(&bandwidth_fixture)
                    .expect("the cursor-only motion remains hardware-bound");
                assert_eq!(moved_binding.lease, bandwidth_binding.lease);
                assert_eq!((moved_binding.x, moved_binding.y), (130, 110));
                assert!(
                    bandwidth_fixture
                        .backend
                        .platform
                        .cursor_commit_in_flight_for_output(0),
                    "the accepted cursor-only event is retired before lifecycle teardown",
                );
                bandwidth_fixture.retire_output(0, 3);
                assert!(
                    !bandwidth_fixture
                        .backend
                        .platform
                        .cursor_commit_in_flight_for_output(0),
                    "the independent cursor commit is retired before teardown",
                );
            } else {
                assert_eq!(applied_binding(&bandwidth_fixture), Some(bandwidth_binding));
                assert!(
                    !bandwidth_fixture
                        .backend
                        .platform
                        .cursor_commit_in_flight_for_output(0),
                    "copied scene work still owns the cursor intent",
                );
            }
            assert_eq!(
                bandwidth_fixture
                    .backend
                    .scene
                    .for_tests_cursor_commit_receipt(0)
                    .expect("previous accepted receipt remains current")
                    .mode,
                CursorAcceptedMode::Hardware
            );
            bandwidth_fixture
                .backend
                .disable_output()
                .expect("Backend teardown after producer failure");
            bandwidth_fixture.assert_cursor_quiescent();
            assert!(!bandwidth_fixture.backend.scene.has_pending_page_flips());
            let ledger = Rc::clone(&bandwidth_fixture.recorders.cursor_buffer_ledger);
            drop(bandwidth_fixture);
            assert_eq!(ledger.end_state(), (0, 0, 0));
        }
    }
}

fn run_riding_hide_frame_retry(copied: bool, retry_succeeds: bool) {
    let Some((mut fixture, reporter, atomic)) = make_fixture(copied) else {
        return;
    };
    let old_binding = establish_hw_cursor(&mut fixture, copied);
    let baseline = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted hardware receipt before riding hide");
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    damage_cursor_root(&mut fixture);
    let before = live_records(&atomic).len();
    script_riding_rejection_then_retry(
        &atomic,
        if retry_succeeds {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(libc::ENOMEM))
        },
    );
    record_next_scene_frame(&mut fixture, copied);
    let live = live_records(&atomic);
    assert_eq!(
        live.len(),
        before + 2,
        "hardware-only hide retries the original primary without cursor properties: {live:?}",
    );
    assert_eq!(live[before].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(live[before + 1].kind, AtomicCommitKind::Scene);
    assert_eq!(
        property_value(
            &live[before],
            fixture.backend.platform.outputs[0].output.plane_fb_id_prop,
        ),
        property_value(
            &live[before + 1],
            fixture.backend.platform.outputs[0].output.plane_fb_id_prop,
        ),
        "the retry preserves the primary framebuffer",
    );
    let (cursor, _) = cursor_properties(&fixture);
    assert!(live[before + 1].properties.iter().all(|update| {
        !cursor
            .iter()
            .any(|(_, property)| *property == update.property)
    }));
    assert_eq!(applied_binding(&fixture), Some(old_binding));
    assert_eq!(sprite_user_count(&fixture, old_binding.lease), 1);
    assert_eq!(reporter.calls(), 0);
    if retry_succeeds {
        assert_eq!(live[before + 1].result, ScriptedResult::Ok);
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        let output = &fixture.backend.platform.outputs[0];
        let state = fixture
            .backend
            .platform
            .device_for_key(output.key.device_key)
            .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
            .unwrap_or_else(|| panic!("hardware-only hide state; records={live:?}"));
        assert_eq!(
            state.fallback,
            CursorFallbackState::DetachPending {
                rejected_key: old_binding.lease.sprite_key(),
            },
            "output 0 keeps the refused cursor delta's captured key pending for detach",
        );
        assert_eq!(
            fixture
                .backend
                .scene
                .for_tests_cursor_commit_receipt(0)
                .expect("accepted cursorless primary receipt")
                .mode,
            CursorAcceptedMode::Hardware,
            "the old HW binding remains authoritative until detach acceptance",
        );
        fixture.retire_output(0, 20);
        let detach = live_records(&atomic)
            .last()
            .cloned()
            .expect("detach follows primary retirement");
        assert_eq!(detach.kind, AtomicCommitKind::CursorOnly);
        let fb_id = cursor.iter().find(|(name, _)| *name == "fb_id").unwrap().1;
        let crtc_id = cursor
            .iter()
            .find(|(name, _)| *name == "crtc_id")
            .unwrap()
            .1;
        assert_eq!(property_value(&detach, fb_id), Some(0));
        assert_eq!(property_value(&detach, crtc_id), Some(0));
        assert_eq!(applied_binding(&fixture), None);
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 1);
        fixture.retire_output(0, 21);
        assert_eq!(sprite_user_count(&fixture, old_binding.lease), 0);
    } else {
        assert_eq!(
            live[before + 1].result,
            ScriptedResult::Error {
                kind: io::Error::from_raw_os_error(libc::ENOMEM).kind(),
                raw_os_error: Some(libc::ENOMEM),
            }
        );
        assert!(!fixture.backend.scene.has_pending_page_flip(0));
        assert_eq!(
            fixture.backend.scene.for_tests_cursor_commit_receipt(0),
            Some(baseline),
            "neither rejected request creates a receipt or event obligation",
        );
        let output = &fixture.backend.platform.outputs[0];
        assert!(
            fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                .is_none_or(|state| state.in_flight.is_none())
        );
    }
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0,
    );
    fixture.backend.disable_output().expect("Backend teardown");
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: require both prior and next bindings, which must skip a hardware-owned hide with no next binding.
fn atomic_cursor_only_riding_hide_retry_then_detaches() {
    for copied in [false, true] {
        run_riding_hide_frame_retry(copied, true);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: swallow a cursorless retry ENOMEM or install a receipt for the rejected hide and retry.
fn atomic_cursor_only_riding_hide_failed_retry_preserves_flip_error() {
    for copied in [false, true] {
        run_riding_hide_frame_retry(copied, false);
    }
}

fn run_mixed_output_hide_and_show_recovery(retry_succeeds: bool) {
    let Some((mut fixture, _, atomic)) = make_two_output_fixture(false) else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("establish the first output's binding");
    for output_idx in 0..2 {
        if fixture.backend.scene.has_pending_page_flip(output_idx) {
            fixture.retire_output(output_idx, 1 + output_idx as u32);
        }
    }
    let prior = applied_binding_for_output(&fixture, 0).expect("output 0 owns the initial binding");
    assert_eq!(applied_binding_for_output(&fixture, 1), None);
    let baseline_receipts = [
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        fixture.backend.scene.for_tests_cursor_commit_receipt(1),
    ];

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 900, 90);
    damage_cursor_root(&mut fixture);
    let before = live_records(&atomic).len();
    for _ in 0..2 {
        for result in [
            Err(io::Error::from_raw_os_error(libc::EINVAL)),
            Err(io::Error::from_raw_os_error(libc::EINVAL)),
            Ok(()),
            if retry_succeeds {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(libc::ENOMEM))
            },
        ] {
            atomic.script_result(result);
        }
    }
    Backend::maybe_composite(&mut fixture.backend).expect("retry both output cursor edges");
    let batch = live_records(&atomic)[before..].to_vec();
    let output0 = records_for_crtc(&batch, fixture.backend.platform.outputs[0].output.crtc);
    let output1 = records_for_crtc(&batch, fixture.backend.platform.outputs[1].output.crtc);
    assert_eq!(
        output0.len(),
        2,
        "hide request and stripped retry: {batch:?}"
    );
    assert_eq!(
        output1.len(),
        2,
        "attach request and stripped retry: {batch:?}"
    );
    for (output_idx, records) in [(0, output0), (1, output1)] {
        assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
        assert_eq!(
            records[0].result,
            ScriptedResult::Error {
                kind: io::Error::from_raw_os_error(libc::EINVAL).kind(),
                raw_os_error: Some(libc::EINVAL),
            }
        );
        assert_eq!(records[1].kind, AtomicCommitKind::Scene);
        assert_eq!(
            records[1].result,
            if retry_succeeds {
                ScriptedResult::Ok
            } else {
                ScriptedResult::Error {
                    kind: io::Error::from_raw_os_error(libc::ENOMEM).kind(),
                    raw_os_error: Some(libc::ENOMEM),
                }
            }
        );
        let primary_fb = fixture.backend.platform.outputs[output_idx]
            .output
            .plane_fb_id_prop;
        assert_eq!(
            property_value(&records[0], primary_fb),
            property_value(&records[1], primary_fb),
            "output {output_idx} retry keeps its submitted primary",
        );
        let properties = fixture
            .backend
            .platform
            .device_for_key(fixture.backend.platform.outputs[output_idx].key.device_key)
            .and_then(|device| device.cursor.plane.as_ref())
            .and_then(|plane| {
                plane.properties_for(fixture.backend.platform.outputs[output_idx].output.crtc)
            })
            .expect("output cursor properties");
        let cursor_properties = [
            properties.fb_id,
            properties.crtc_id,
            properties.crtc_x,
            properties.crtc_y,
            properties.crtc_w,
            properties.crtc_h,
            properties.src_x,
            properties.src_y,
            properties.src_w,
            properties.src_h,
        ];
        assert!(
            records[1]
                .properties
                .iter()
                .all(|update| { !cursor_properties.contains(&update.property) })
        );
    }
    assert_eq!(applied_binding_for_output(&fixture, 0), Some(prior));
    assert_eq!(applied_binding_for_output(&fixture, 1), None);
    if retry_succeeds {
        for output_idx in 0..2 {
            let output = &fixture.backend.platform.outputs[output_idx];
            let state = fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                .expect("each rejected update retains its CRTC state");
            if output_idx == 0 {
                assert!(
                    matches!(state.fallback, CursorFallbackState::DetachPending { .. }),
                    "the refused hardware hide keeps its prior binding until detach: {state:?}"
                );
            } else {
                assert!(
                    matches!(state.fallback, CursorFallbackState::Software { .. }),
                    "the scene-owned show retry retains software ownership: {state:?}"
                );
            }
            assert!(fixture.backend.scene.has_pending_page_flip(output_idx));
        }
        fixture.retire_output(0, 10);
        fixture.retire_output(1, 11);
        let output0 = &fixture.backend.platform.outputs[0];
        let detach = live_records(&atomic)
            .iter()
            .rev()
            .find(|record| {
                record_crtc(record, output0.output.crtc)
                    && record.kind == AtomicCommitKind::CursorOnly
            })
            .cloned()
            .expect("output 0 hide gets its explicit detach after retirement");
        assert_eq!(detach.result, ScriptedResult::Ok);
        let props = fixture
            .backend
            .platform
            .device_for_key(output0.key.device_key)
            .and_then(|device| device.cursor.plane.as_ref())
            .and_then(|plane| plane.properties_for(output0.output.crtc))
            .expect("output 0 cursor properties");
        assert_eq!(property_value(&detach, props.fb_id), Some(0));
        assert_eq!(property_value(&detach, props.crtc_id), Some(0));
        fixture.retire_output(0, 20);
        assert_eq!(applied_binding_for_output(&fixture, 0), None);
        assert_eq!(applied_binding_for_output(&fixture, 1), None);
    } else {
        for (output_idx, baseline_receipt) in baseline_receipts.iter().copied().enumerate() {
            assert!(!fixture.backend.scene.has_pending_page_flip(output_idx));
            assert_eq!(
                fixture
                    .backend
                    .scene
                    .for_tests_cursor_commit_receipt(output_idx),
                baseline_receipt,
                "failed batch retry creates no output {output_idx} receipt",
            );
            let output = &fixture.backend.platform.outputs[output_idx];
            let state = fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc));
            assert!(state.is_none_or(|state| state.in_flight.is_none()));
        }
        assert_eq!(applied_binding_for_output(&fixture, 0), Some(prior));
        assert_eq!(applied_binding_for_output(&fixture, 1), None);
    }
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    fixture.backend.disable_output().expect("Backend teardown");
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: route the scene-owned output-1 show through hardware-only detach recovery.
fn atomic_cursor_only_mixed_output_hide_and_show_recovery() {
    run_mixed_output_hide_and_show_recovery(true);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: falsely accept either cursorless retry after ENOMEM or settle its rejected receipt.
fn atomic_cursor_only_mixed_output_failed_retries_keep_each_flip_error() {
    run_mixed_output_hide_and_show_recovery(false);
}

fn make_two_output_fixture(
    copied: bool,
) -> Option<(
    AtomicCursorFixture,
    Rc<RecordingCursorRecoveryReporter>,
    Rc<RecordingAtomicCommitExecutor>,
)> {
    let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
    let mut fixture = AtomicCursorFixture::new_with_two_outputs()
        .expect("build two-output failed-show fixture")?;
    fixture.backend.platform.services.cursor_recovery_reporter = reporter.clone();
    let atomic = if copied {
        install_copied_recording_route_with_hotspot(&mut fixture, true)
    } else {
        fixture
            .backend
            .platform
            .for_tests_accept_output_power_commits();
        fixture.recorders.atomic.clone()
    };
    Some((fixture, reporter, atomic))
}

fn settle_all_copied_frames(fixture: &mut AtomicCursorFixture) {
    while let Some(job_id) = fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()
        .first()
        .copied()
    {
        settle_copied_frame(fixture, job_id);
    }
}

fn record_crtc(record: &AtomicCommitRecord, crtc: ::drm::control::crtc::Handle) -> bool {
    record.events.iter().any(|event| event.crtc == crtc)
}

fn records_for_crtc(
    records: &[AtomicCommitRecord],
    crtc: ::drm::control::crtc::Handle,
) -> Vec<AtomicCommitRecord> {
    records
        .iter()
        .filter(|record| record_crtc(record, crtc))
        .cloned()
        .collect()
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (run and seen to fail): M11 labels the cursorless retry Hardware; M12 claims its primary already contains SW pixels; M13 omits the immediate recompose wake; M14 replays the rejected show when the cursorless retry retires.
fn atomic_cursor_only_failed_show_accepts_one_cursorless_frame_then_software() {
    use crate::kms::render::scene::CursorPlaneMode;

    for copied in [false, true] {
        let Some((mut fixture, reporter, atomic)) = make_two_output_fixture(copied) else {
            return;
        };
        let root = fixture.backend.core.window_id;
        let large =
            create_argb_cursor_extent(&mut fixture.backend, [0x00, 0x20, 0xff, 0xff], 65, 65, 0, 0);
        Backend::define_cursor(&mut fixture.backend, None, root, large.as_raw())
            .expect("select a software-only cursor through Backend");
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 790, 90);
        damage_cursor_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.backend).expect("establish software ownership");
        if copied {
            settle_all_copied_frames(&mut fixture);
        }
        for output_idx in [0, 1] {
            let receipt = fixture
                .backend
                .scene
                .for_tests_cursor_commit_receipt(output_idx)
                .expect("accepted initial software frame receipt");
            assert_eq!(receipt.mode, CursorAcceptedMode::Software);
            assert!(matches!(
                receipt.primary_cursor,
                CursorPrimaryContent::Software { .. }
            ));
        }
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        assert!(fixture.backend.scene.has_pending_page_flip(1));
        fixture.retire_output(0, 40);
        fixture.retire_output(1, 41);
        assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);
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

        let small =
            create_argb_cursor_extent(&mut fixture.backend, [0x00, 0x20, 0xff, 0xff], 32, 32, 0, 0);
        Backend::define_cursor(&mut fixture.backend, None, root, small.as_raw())
            .expect("request hardware qualification through Backend");
        let before_show = live_records(&atomic).len();
        for result in [
            Err(io::Error::from_raw_os_error(libc::EINVAL)),
            Err(io::Error::from_raw_os_error(libc::EINVAL)),
            Ok(()),
            Ok(()),
        ] {
            atomic.script_result(result);
        }
        Backend::maybe_composite(&mut fixture.backend).expect("retry the rejected show frame");
        if copied {
            settle_all_copied_frames(&mut fixture);
        }

        let outputs = &fixture.backend.platform.outputs;
        let crtc0 = outputs[0].output.crtc;
        let crtc1 = outputs[1].output.crtc;
        let commits = live_records(&atomic);
        let output0 = records_for_crtc(&commits[before_show..], crtc0);
        let output1 = records_for_crtc(&commits[before_show..], crtc1);
        assert_eq!(output0.len(), 2, "failed show plus cursorless retry");
        assert_eq!(output0[0].kind, AtomicCommitKind::SceneWithCursor);
        assert_eq!(
            output0[0].result,
            ScriptedResult::Error {
                kind: io::ErrorKind::InvalidInput,
                raw_os_error: Some(libc::EINVAL),
            },
            "the hardware show is cursor-attributed as rejected",
        );
        assert_eq!(output0[1].kind, AtomicCommitKind::Scene);
        assert_eq!(output0[1].result, ScriptedResult::Ok);
        assert_eq!(output1.len(), 1, "the peer accepts its hardware frame");
        assert_eq!(output1[0].kind, AtomicCommitKind::SceneWithCursor);
        assert_eq!(output1[0].result, ScriptedResult::Ok);
        let primary_fb = outputs[0].output.plane_fb_id_prop;
        assert_eq!(
            property_value(&output0[0], primary_fb),
            property_value(&output0[1], primary_fb),
            "retry accepts the original cursorless primary framebuffer",
        );
        let (cursor_properties, _) = cursor_properties(&fixture);
        assert!(output0[1].properties.iter().all(|update| {
            !cursor_properties
                .iter()
                .any(|(_, property)| *property == update.property)
        }));
        let cursorless_receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted cursorless retry receipt");
        assert_eq!(cursorless_receipt.mode, CursorAcceptedMode::Software);
        assert_eq!(
            cursorless_receipt.primary_cursor,
            CursorPrimaryContent::Cursorless
        );
        assert!(cursorless_receipt.needs_recompose);
        let peer_receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(1)
            .expect("accepted peer hardware frame receipt");
        assert_eq!(peer_receipt.mode, CursorAcceptedMode::Hardware);
        assert_eq!(
            peer_receipt.primary_cursor,
            CursorPrimaryContent::Cursorless
        );
        assert_eq!(applied_binding(&fixture), None);
        assert!(applied_binding_for_output(&fixture, 1).is_some());
        assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Mixed);
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        assert!(fixture.backend.scene.has_pending_page_flip(1));
        assert!(fixture.backend.for_tests_scene_wants_scene());
        assert_eq!(reporter.calls(), 0);

        let records_before_retirement = commits.len();
        fixture.retire_output(0, 42);
        assert!(!fixture.backend.scene.has_pending_page_flip(0));
        assert!(fixture.backend.scene.has_pending_page_flip(1));
        assert!(
            !fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(0),
            "retirement cannot install SW metadata for the cursorless primary",
        );
        let no_cursor_under = read_output_pixel(&mut fixture, 795, 100);
        let no_cursor_beside = read_output_pixel(&mut fixture, 750, 100);
        assert_eq!(&no_cursor_under[..3], &no_cursor_beside[..3]);
        assert_eq!(live_records(&atomic).len(), records_before_retirement);

        // No pointer motion or client damage occurs between retirement and this
        // next Backend scheduling opportunity.
        Backend::maybe_composite(&mut fixture.backend)
            .expect("immediately recompose the software cursor");
        if copied {
            settle_all_copied_frames(&mut fixture);
        }
        assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Mixed);
        let software_receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted software cursor repaint receipt");
        assert_eq!(software_receipt.mode, CursorAcceptedMode::Software);
        assert!(matches!(
            software_receipt.primary_cursor,
            CursorPrimaryContent::Software { .. }
        ));
        assert!(!software_receipt.needs_recompose);
        assert!(fixture.backend.scene.has_pending_page_flip(0));
        assert!(fixture.backend.scene.has_pending_page_flip(1));
        let software_commit = live_records(&atomic)
            .into_iter()
            .rfind(|record| record_crtc(record, crtc0))
            .expect("software recompose commit");
        assert_eq!(software_commit.kind, AtomicCommitKind::Scene);
        assert!(software_commit.properties.iter().all(|update| {
            !cursor_properties
                .iter()
                .any(|(_, property)| *property == update.property)
        }));
        fixture.retire_output(0, 43);
        assert!(
            fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(0)
        );
        let software_under = read_output_pixel(&mut fixture, 795, 100);
        let software_beside = read_output_pixel(&mut fixture, 750, 100);
        assert_ne!(&software_under[..3], &software_beside[..3]);
        assert_eq!(applied_binding(&fixture), None);
        assert!(applied_binding_for_output(&fixture, 1).is_some());
        assert_eq!(
            live_records(&atomic)
                .iter()
                .map(|record| (record.events[0].crtc, record.kind))
                .collect::<Vec<_>>(),
            vec![
                (crtc0, AtomicCommitKind::SceneWithCursor),
                (crtc1, AtomicCommitKind::SceneWithCursor),
                (crtc0, AtomicCommitKind::SceneWithCursor),
                (crtc0, AtomicCommitKind::Scene),
                (crtc1, AtomicCommitKind::SceneWithCursor),
                (crtc0, AtomicCommitKind::Scene),
            ],
            "accepted primary sequence is software baseline, failed show retry, peer HW, then immediate SW",
        );

        fixture.retire_output(1, 44);
        assert!(
            !fixture
                .backend
                .scene
                .for_tests_cursor_footprint_was_presented(1),
            "the accepted peer hardware primary remains cursorless",
        );
        fixture
            .backend
            .disable_output()
            .expect("Backend teardown detaches and drains the hardware peer");
        fixture.assert_cursor_quiescent();
        assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
        assert_eq!(
            fixture
                .backend
                .platform
                .for_tests_pending_scanout_render_completion_count(),
            0,
        );
        assert!(!fixture.backend.scene.has_pending_page_flips());
        let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: settle or publish the rejected show after its cursorless retry returns ENOMEM; record the rejected key only after a successful retry.
fn atomic_cursor_only_failed_show_retry_failure_keeps_original_flip_error() {
    for copied in [false, true] {
        let Some((mut fixture, reporter, atomic)) = make_fixture(copied) else {
            return;
        };
        if copied {
            fixture
                .recorders
                .render_completion_readiness
                .hold_ready_jobs();
        }
        let root = fixture.backend.core.window_id;
        let oversized =
            create_argb_cursor_extent(&mut fixture.backend, [0x20, 0x30, 0xe0, 0xff], 65, 65, 0, 0);
        Backend::define_cursor(&mut fixture.backend, None, root, oversized.as_raw())
            .expect("select a software-only cursor through Backend");
        Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
        damage_cursor_root(&mut fixture);
        record_next_scene_frame(&mut fixture, copied);
        let software_receipt = fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted software baseline receipt");
        assert_eq!(software_receipt.mode, CursorAcceptedMode::Software);
        assert!(matches!(
            software_receipt.primary_cursor,
            CursorPrimaryContent::Software { .. }
        ));
        fixture.retire_output(0, 30);

        let small =
            create_argb_cursor_extent(&mut fixture.backend, [0x20, 0x30, 0xe0, 0xff], 32, 32, 0, 0);
        Backend::define_cursor(&mut fixture.backend, None, root, small.as_raw())
            .expect("request a hardware show through Backend");
        damage_cursor_root(&mut fixture);
        let before = live_records(&atomic).len();
        script_riding_rejection_then_retry(
            &atomic,
            Err(io::Error::from_raw_os_error(libc::ENOMEM)),
        );
        record_next_scene_frame(&mut fixture, copied);
        let live = live_records(&atomic);
        assert_eq!(live.len(), before + 2);
        assert_eq!(live[before].kind, AtomicCommitKind::SceneWithCursor);
        assert_eq!(live[before + 1].kind, AtomicCommitKind::Scene);
        assert_eq!(
            live[before + 1].result,
            ScriptedResult::Error {
                kind: io::Error::from_raw_os_error(libc::ENOMEM).kind(),
                raw_os_error: Some(libc::ENOMEM),
            }
        );
        let primary_fb = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
        assert_eq!(
            property_value(&live[before], primary_fb),
            property_value(&live[before + 1], primary_fb),
            "failed cursorless retry preserves the original primary",
        );
        let (cursor, _) = cursor_properties(&fixture);
        assert!(live[before + 1].properties.iter().all(|update| {
            !cursor
                .iter()
                .any(|(_, property)| *property == update.property)
        }));
        assert_eq!(
            fixture.backend.scene.for_tests_cursor_commit_receipt(0),
            Some(software_receipt),
            "failed show and failed primary retry create no receipt",
        );
        assert!(!fixture.backend.scene.has_pending_page_flip(0));
        assert_eq!(applied_binding(&fixture), None);
        assert!(
            !fixture
                .backend
                .platform
                .cursor_commit_in_flight_for_output(0)
        );
        assert_eq!(reporter.calls(), 0);
        assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
        assert_eq!(
            fixture
                .backend
                .platform
                .for_tests_pending_scanout_render_completion_count(),
            0,
        );

        let output = &fixture.backend.platform.outputs[0];
        let rejected_key = (32, 32, 0, 0);
        let cursor_device = fixture
            .backend
            .platform
            .device_for_key(output.key.device_key)
            .expect("output cursor device");
        let cursor_state = cursor_device
            .cursor
            .crtc_states
            .get(&output.output.crtc)
            .unwrap_or_else(|| {
                panic!(
                    "classified cursor rejection creates CRTC state: {:?}",
                    cursor_device.cursor.crtc_states
                )
            });
        let rejected_epoch = cursor_device.cursor.cursor_topology_epoch;
        assert!(
            cursor_state
                .qualification
                .rejected_in_epoch(rejected_epoch, Some(rejected_key)),
            "classifier-attributed cursor rejection is remembered despite ENOMEM retry",
        );

        let before_same_key = live_records(&atomic).len();
        damage_cursor_root(&mut fixture);
        drive_scene_retry_deadline_through_backend(&mut fixture);
        record_next_scene_frame(&mut fixture, copied);
        let same_key_records = live_records(&atomic)[before_same_key..].to_vec();
        assert_eq!(
            same_key_records.len(),
            1,
            "same-epoch scheduling renders only a cursorless successor: {same_key_records:?}",
        );
        assert_eq!(same_key_records[0].kind, AtomicCommitKind::Scene);
        assert_eq!(same_key_records[0].result, ScriptedResult::Ok);
        let (cursor, _) = cursor_properties(&fixture);
        assert!(same_key_records[0].properties.iter().all(|update| {
            !cursor
                .iter()
                .any(|(_, property)| *property == update.property)
        }));
        assert_eq!(applied_binding(&fixture), None);
        assert_eq!(
            fixture.backend.scene.cursor_mode(),
            crate::kms::render::scene::CursorPlaneMode::Sw
        );
        if fixture.backend.scene.has_pending_page_flip(0) {
            fixture.retire_output(0, 31);
        }
        assert!(!fixture.backend.scene.has_pending_page_flip(0));
        assert!(
            !fixture
                .backend
                .platform
                .cursor_commit_in_flight_for_output(0)
        );
        assert!(fixture.backend.platform.staged_cursor_frames.is_empty());

        fixture
            .backend
            .platform
            .for_tests_accept_output_power_commits();
        fixture.backend.disable_output().expect("Backend teardown");
        fixture.assert_cursor_quiescent();
        let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        fixture.shutdown_drawables();
        drop(fixture);
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation (run and seen to fail): record the device's current sprite signature at copied-frame settlement; sprite B is then incorrectly blacklisted by sprite A's rejection.
fn copied_failed_show_rejects_captured_sprite_and_keeps_new_sprite_eligible() {
    use crate::kms::render::scene::CursorPlaneMode;

    let Some((mut fixture, _, atomic)) = make_fixture(true) else {
        return;
    };
    fixture
        .recorders
        .render_completion_readiness
        .hold_ready_jobs();

    let root = fixture.backend.core.window_id;
    let oversized =
        create_argb_cursor_extent(&mut fixture.backend, [0x00, 0x20, 0xff, 0xff], 65, 65, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, oversized.as_raw())
        .expect("select initial software cursor through Backend");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    record_next_scene_frame(&mut fixture, true);
    let initial = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted initial software frame receipt");
    assert_eq!(initial.mode, CursorAcceptedMode::Software);
    assert!(matches!(
        initial.primary_cursor,
        CursorPrimaryContent::Software { .. }
    ));
    fixture.retire_output(0, 70);

    let sprite_a =
        create_argb_cursor_extent(&mut fixture.backend, [0x00, 0x20, 0xff, 0xff], 32, 32, 0, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, sprite_a.as_raw())
        .expect("select sprite A through Backend");
    let before_a = live_records(&atomic).len();
    Backend::maybe_composite(&mut fixture.backend).expect("stage copied frame containing A");
    let job_a = *fixture
        .backend
        .platform
        .for_tests_pending_scanout_render_completion_ids()
        .first()
        .expect("copied frame A is waiting for completion");
    let staged_a = fixture
        .backend
        .platform
        .staged_cursor_frames
        .values()
        .find(|staged| {
            staged
                .cursor_updates
                .iter()
                .any(|update| update.prepared.next.is_some())
        })
        .expect("copied frame A captured its hardware binding");
    let update_a = staged_a
        .cursor_updates
        .iter()
        .find(|update| update.prepared.next.is_some())
        .expect("A show update");
    let captured_a_key = update_a.sprite_key;
    let captured_a_fb = update_a.prepared.next.expect("A show binding").framebuffer;
    assert_eq!(captured_a_key, Some((32, 32, 0, 0)));

    let sprite_b =
        create_argb_cursor_extent(&mut fixture.backend, [0x00, 0x20, 0xff, 0xff], 32, 32, 1, 0);
    Backend::define_cursor(&mut fixture.backend, None, root, sprite_b.as_raw())
        .expect("select newer sprite B before A settles");
    let expected_b = (32, 32, 1, 0);

    for result in [
        Err(io::Error::from_raw_os_error(libc::EINVAL)),
        Err(io::Error::from_raw_os_error(libc::EINVAL)),
        Ok(()),
        Ok(()),
    ] {
        atomic.script_result(result);
    }
    settle_copied_frame(&mut fixture, job_a);

    let (cursor, _) = cursor_properties(&fixture);
    let commits = live_records(&atomic);
    let rejected_a = &commits[before_a..];
    assert_eq!(rejected_a.len(), 2, "A show and its cursorless retry");
    assert_eq!(rejected_a[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(
        rejected_a[0].result,
        ScriptedResult::Error {
            kind: io::ErrorKind::InvalidInput,
            raw_os_error: Some(libc::EINVAL),
        }
    );
    assert_eq!(rejected_a[1].kind, AtomicCommitKind::Scene);
    assert_eq!(rejected_a[1].result, ScriptedResult::Ok);
    let fb_id = cursor.iter().find(|(name, _)| *name == "fb_id").unwrap().1;
    let rejected_a_fb = property_value(&rejected_a[0], fb_id).expect("A show framebuffer");
    assert_eq!(rejected_a_fb, u64::from(captured_a_fb));
    let primary_fb = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
    assert_eq!(
        property_value(&rejected_a[0], primary_fb),
        property_value(&rejected_a[1], primary_fb),
        "cursorless retry preserves A's original primary"
    );
    assert!(rejected_a[1].properties.iter().all(|update| {
        !cursor
            .iter()
            .any(|(_, property)| *property == update.property)
    }));
    let cursorless = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted cursorless retry receipt");
    assert_eq!(cursorless.mode, CursorAcceptedMode::Software);
    assert_eq!(cursorless.primary_cursor, CursorPrimaryContent::Cursorless);
    assert!(cursorless.needs_recompose);
    assert_eq!(applied_binding(&fixture), None);
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Sw);
    let output = &fixture.backend.platform.outputs[0];
    let fallback_after_a = fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
        .map(|state| state.fallback)
        .expect("A's CRTC fallback state");
    assert_eq!(
        fallback_after_a,
        CursorFallbackState::Software {
            rejected_key: captured_a_key
        }
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_plane_fits_for_output(0, 32, 32)
    );

    fixture.retire_output(0, 71);
    Backend::maybe_composite(&mut fixture.backend).expect("compose newer sprite B immediately");
    settle_all_copied_frames(&mut fixture);
    let hardware = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted B cursor receipt");
    assert_eq!(hardware.mode, CursorAcceptedMode::Hardware);
    assert_eq!(hardware.primary_cursor, CursorPrimaryContent::Cursorless);
    let binding_b = applied_binding(&fixture).expect("B is accepted on the hardware plane");
    assert_ne!(
        u64::from(binding_b.framebuffer),
        rejected_a_fb,
        "B does not reuse rejected A"
    );
    assert_eq!(
        (binding_b.hot_x, binding_b.hot_y),
        (expected_b.2, expected_b.3)
    );
    assert_eq!(binding_b.lease.sprite_key(), Some(expected_b));
    assert_eq!(fixture.backend.scene.cursor_mode(), CursorPlaneMode::Mixed);
    assert!(!hardware.needs_recompose);

    let sequence = live_records(&atomic)
        .iter()
        .map(|record| (record.kind, record.result))
        .collect::<Vec<_>>();
    assert_eq!(
        sequence,
        vec![
            (AtomicCommitKind::SceneWithCursor, ScriptedResult::Ok),
            (
                AtomicCommitKind::SceneWithCursor,
                ScriptedResult::Error {
                    kind: io::ErrorKind::InvalidInput,
                    raw_os_error: Some(libc::EINVAL),
                },
            ),
            (AtomicCommitKind::Scene, ScriptedResult::Ok),
            (AtomicCommitKind::SceneWithCursor, ScriptedResult::Ok),
        ],
        "the exact baseline, rejected A, cursorless retry, and accepted B sequence"
    );

    fixture.retire_output(0, 72);
    fixture.backend.disable_output().expect("Backend teardown");
    fixture.assert_cursor_quiescent();
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0
    );
    assert!(!fixture.backend.scene.has_pending_page_flips());
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

fn applied_binding_for_output(
    fixture: &AtomicCursorFixture,
    output_idx: usize,
) -> Option<CursorBinding> {
    let output = &fixture.backend.platform.outputs[output_idx];
    fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
        .and_then(|state| state.applied)
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: publish the rejected sprite before detach, dump the candidate instead of the applied sprite, discard its software candidate, or hide the accepted prior binding before the retry.
fn atomic_cursor_only_rejected_replacement_preserves_candidate_and_binding() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new().expect("build cursor fixture") else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let root = fixture.backend.core.window_id;
    let width = fixture.backend.platform.fb_w;
    let height = fixture.backend.platform.fb_h;
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor frame");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);
    let old_binding = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states
        .get(&crtc)
        .unwrap()
        .applied
        .unwrap();

    let moved_binding = old_binding;
    let replacement_pixels = [0xde, 0xad, 0xbe, 0xff];
    let replacement_cursor =
        create_argb_cursor_extent(&mut fixture.backend, replacement_pixels, 4, 4, 0, 0);
    Backend::define_cursor(
        &mut fixture.backend,
        None,
        root,
        replacement_cursor.as_raw(),
    )
    .expect("define replacement sprite through Backend");
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
    .expect("damage replacement through Backend");
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    fixture.recorders.atomic.script_result(Ok(()));
    Backend::maybe_composite(&mut fixture.backend).expect("submit replacement frame");
    let state_after_rejection = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states
        .get(&crtc)
        .unwrap()
        .clone();
    let replacement_version = fixture.backend.cursor_records[&replacement_cursor.as_raw()].version;
    let replacement_candidate = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .plane
        .as_ref()
        .unwrap()
        .candidate_sprite()
        .unwrap();
    assert_eq!(replacement_candidate.0.version(), replacement_version);
    let plane = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("cursor plane");
    let applied_pixels = plane
        .for_tests_sprite_bytes(old_binding.lease)
        .expect("accepted prior sprite remains mapped");
    let candidate_pixels = plane
        .for_tests_sprite_bytes(replacement_candidate.0)
        .expect("rejected candidate remains mapped for retry");
    assert_ne!(applied_pixels[..4], candidate_pixels[..4]);

    let rejected_applied = state_after_rejection.applied;
    let replacement_in_flight_prior = state_after_rejection
        .in_flight
        .as_ref()
        .and_then(|commit| commit.prior);
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    let rejected = fixture
        .recorders
        .atomic
        .records()
        .into_iter()
        .find(|record| {
            record.result
                == crate::drm::atomic::tests_support::ScriptedResult::Error {
                    kind: std::io::ErrorKind::InvalidInput,
                    raw_os_error: Some(libc::EINVAL),
                }
        })
        .expect("scripted replacement rejection is recorded");
    assert_eq!(
        rejected.kind,
        crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    );
    assert_eq!(
        rejected.result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            kind: std::io::ErrorKind::InvalidInput,
            raw_os_error: Some(libc::EINVAL),
        }
    );
    assert_eq!(replacement_candidate.0.version(), replacement_version);
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
        "a rejected HW-only replacement leaves the accepted output mode unchanged",
    );
    assert_eq!(rejected_applied, Some(moved_binding));
    assert!(replacement_in_flight_prior.is_none());
    assert!(matches!(
        state_after_rejection.intent,
        Some(crate::kms::render::platform::cursor_state::CursorIntent::Hide)
    ));
    assert!(matches!(
        state_after_rejection.fallback,
        crate::kms::render::platform::cursor_state::CursorFallbackState::DetachPending { .. }
    ));
    assert!(state_after_rejection.in_flight.is_none());
    assert_eq!(state_after_rejection.applied, Some(moved_binding));
    assert!(fixture.backend.scene.has_pending_page_flip(0));

    let files_before_dump = cursor_dump_files();
    Backend::dump_scanout(&mut fixture.backend);
    let files_after_dump = cursor_dump_files();
    let created_files: Vec<_> = files_after_dump
        .difference(&files_before_dump)
        .cloned()
        .collect();
    let applied_dump = created_files
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("-crtc"))
        })
        .expect("Backend dump writes the retained applied sprite");
    assert_eq!(
        ppm_first_rgb_pixel(applied_dump),
        vec![applied_pixels[2], applied_pixels[1], applied_pixels[0]],
        "cursor diagnostics dump the applied sprite while a different candidate awaits detach",
    );
    for path in created_files {
        std::fs::remove_file(path).expect("remove generated test dump");
    }

    fixture.retire_output(0, 3);
    let detached_state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states
        .get(&crtc)
        .unwrap();
    assert!(detached_state.applied.is_none());
    assert!(detached_state.in_flight.is_some());
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    fixture.retire_output(0, 4);

    let detached_state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states
        .get(&crtc)
        .unwrap();
    assert!(detached_state.applied.is_none());
    assert!(detached_state.in_flight.is_none());
    assert!(matches!(
        detached_state.fallback,
        crate::kms::render::platform::cursor_state::CursorFallbackState::Software { .. }
    ));
    Backend::maybe_composite(&mut fixture.backend).expect("compose the replacement in software");
    let software = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("accepted software replacement receipt");
    assert_eq!(
        software.mode,
        crate::kms::render::scene::CursorAcceptedMode::Software
    );
    assert!(matches!(
        software.primary_cursor,
        crate::kms::render::scene::CursorPrimaryContent::Software { .. }
    ));
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    fixture.retire_output(0, 5);
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    let software_state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states
        .get(&crtc)
        .unwrap();
    assert!(software_state.applied.is_none());
    assert!(software_state.in_flight.is_none());
    assert!(software_state.intent.is_none());

    fixture
        .backend
        .disable_output()
        .expect("blocking lifecycle teardown detaches the retained prior binding");
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    let state = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .unwrap()
        .cursor
        .crtc_states
        .get(&crtc)
        .unwrap();
    assert!(state.applied.is_none());
    assert!(state.in_flight.is_none());
    assert!(state.intent.is_none());
    fixture.assert_cursor_quiescent();
    assert_eq!(
        fixture
            .recorders
            .atomic
            .records()
            .iter()
            .filter(|record| !record
                .flags
                .contains(::drm::control::AtomicCommitFlags::TEST_ONLY))
            .map(|record| record.kind)
            .collect::<Vec<_>>(),
        vec![
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::SceneWithCursor,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::CursorOnly,
            crate::drm::atomic::AtomicCommitKind::Scene,
            crate::drm::atomic::AtomicCommitKind::Scene,
        ],
        "the accepted cursorless retry precedes detach, then the software frame",
    );
    let resources = std::rc::Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    fixture.shutdown_drawables();
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}
