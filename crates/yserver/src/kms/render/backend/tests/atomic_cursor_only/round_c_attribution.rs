use std::rc::Rc;

use super::{round_a::damage_cursor_root, support::AtomicCursorFixture};
use crate::{
    drm::atomic::{
        AtomicCommitKind,
        tests_support::{AtomicCommitRecord, RecordingAtomicCommitExecutor, ScriptedResult},
    },
    kms::render::{
        backend::KmsBackend, platform::cursor_recovery::RecordingCursorRecoveryReporter,
    },
};
use ::drm::control::{AtomicCommitFlags, property};

fn einval() -> ScriptedResult {
    ScriptedResult::Error {
        kind: std::io::ErrorKind::InvalidInput,
        raw_os_error: Some(libc::EINVAL),
    }
}

/// The output's cursor plane properties, with the hotspot pair when present.
pub(super) fn output_cursor_handles(backend: &KmsBackend) -> Vec<(&'static str, property::Handle)> {
    let crtc = backend.platform.outputs[0].output.crtc;
    let device_key = backend.platform.outputs[0].key.device_key;
    let props = backend
        .platform
        .device_for_key(device_key)
        .expect("output KMS device")
        .cursor
        .plane
        .as_ref()
        .expect("output cursor plane")
        .properties_for(crtc)
        .expect("output cursor properties");
    let mut handles = vec![
        ("fb_id", props.fb_id),
        ("crtc_id", props.crtc_id),
        ("crtc_x", props.crtc_x),
        ("crtc_y", props.crtc_y),
        ("crtc_w", props.crtc_w),
        ("crtc_h", props.crtc_h),
        ("src_x", props.src_x),
        ("src_y", props.src_y),
        ("src_w", props.src_w),
        ("src_h", props.src_h),
    ];
    if let (Some(hot_x), Some(hot_y)) = (props.hotspot_x, props.hotspot_y) {
        handles.push(("hotspot_x", hot_x));
        handles.push(("hotspot_y", hot_y));
    }
    handles
}

/// Cursor handles of the output when it has a cursor plane, otherwise none.
fn optional_cursor_handles(backend: &KmsBackend) -> Vec<property::Handle> {
    let crtc = backend.platform.outputs[0].output.crtc;
    let device_key = backend.platform.outputs[0].key.device_key;
    let has_plane = backend
        .platform
        .device_for_key(device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(crtc))
        .is_some();
    if has_plane {
        cursor_only_handles(&output_cursor_handles(backend))
    } else {
        Vec::new()
    }
}

/// Hotspot-capable fixture whose cursor recovery reports to the counting reporter.
pub(super) fn cursor_fixture(
    reporter: &Rc<RecordingCursorRecoveryReporter>,
) -> Option<(AtomicCursorFixture, Vec<(&'static str, property::Handle)>)> {
    let mut fixture =
        AtomicCursorFixture::new_with_hotspot_properties_and_reporter(Rc::clone(reporter))
            .expect("build cursor fixture")?;
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let handles = output_cursor_handles(&fixture.backend);
    Some((fixture, handles))
}

/// Make the cursor visible through Backend entries only: the first composite
/// after defining it submits the show.
fn submit_visible_cursor_frame(fixture: &mut AtomicCursorFixture) -> Result<(), std::io::Error> {
    use yserver_core::backend::Backend;

    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(fixture);
    Backend::maybe_composite(&mut fixture.backend)
}

/// Backend teardown: the disable commit releases the cursor and every sprite
/// user, and the cursor buffer ledger ends empty.
pub(super) fn teardown(mut fixture: AtomicCursorFixture) {
    fixture.recorders.atomic.clear_property_rejections();
    fixture
        .backend
        .disable_output()
        .expect("blocking Backend teardown detaches the cursor");
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

pub(super) fn cursor_only_handles(
    handles: &[(&'static str, property::Handle)],
) -> Vec<property::Handle> {
    handles.iter().map(|(_, handle)| *handle).collect()
}

fn live_records(atomic: &RecordingAtomicCommitExecutor) -> Vec<AtomicCommitRecord> {
    atomic
        .records()
        .into_iter()
        .filter(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .collect()
}

fn property_value(record: &AtomicCommitRecord, property: property::Handle) -> Option<u64> {
    record
        .properties
        .iter()
        .find_map(|update| (update.property == property).then_some(update.value))
}

fn applied_binding(
    fixture: &AtomicCursorFixture,
) -> Option<crate::kms::render::platform::cursor_state::CursorBinding> {
    let output = &fixture.backend.platform.outputs[0];
    fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
        .and_then(|state| state.applied)
}

fn assert_no_cursor_properties(record: &AtomicCommitRecord, cursor: &[property::Handle]) {
    assert!(
        record
            .properties
            .iter()
            .all(|update| !cursor.contains(&update.property)),
        "the stripped request carries no cursor property: {record:?}",
    );
}

/// The only sequence that blames the cursor: the rejected request, the identical
/// full `TEST_ONLY` rejection, and the cursor-stripped `TEST_ONLY` acceptance.
fn assert_cursor_attribution_trace(
    records: &[AtomicCommitRecord],
    live_kind: AtomicCommitKind,
    cursor: &[property::Handle],
    name: &str,
) {
    assert_eq!(records.len(), 3, "{name}: {records:?}");
    let (live, full, stripped) = (&records[0], &records[1], &records[2]);
    assert_eq!(live.kind, live_kind, "{name}");
    assert!(!live.flags.contains(AtomicCommitFlags::TEST_ONLY), "{name}");
    assert_eq!(live.result, einval(), "{name}");
    assert!(full.flags.contains(AtomicCommitFlags::TEST_ONLY), "{name}");
    assert_eq!(full.kind, live_kind, "{name}");
    assert_eq!(
        full.properties, live.properties,
        "{name}: identical request"
    );
    assert_eq!(full.result, einval(), "{name}");
    assert!(
        stripped.flags.contains(AtomicCommitFlags::TEST_ONLY),
        "{name}"
    );
    assert_eq!(stripped.kind, AtomicCommitKind::Scene, "{name}");
    assert_eq!(stripped.result, ScriptedResult::Ok, "{name}");
    assert_no_cursor_properties(stripped, cursor);
    assert!(
        stripped.events.is_empty(),
        "{name}: TEST_ONLY asks for no event"
    );
}

/// A show rejection adds an accepted cursorless retry after attribution.
fn assert_show_cursor_attribution_trace(
    records: &[AtomicCommitRecord],
    live_kind: AtomicCommitKind,
    cursor: &[property::Handle],
    name: &str,
) {
    assert_eq!(records.len(), 4, "{name}: {records:?}");
    assert_cursor_attribution_trace(&records[..3], live_kind, cursor, name);
    let retry = &records[3];
    assert!(
        !retry.flags.contains(AtomicCommitFlags::TEST_ONLY),
        "{name}"
    );
    assert_eq!(retry.kind, AtomicCommitKind::Scene, "{name}");
    assert_eq!(retry.result, ScriptedResult::Ok, "{name}");
    assert_no_cursor_properties(retry, cursor);
    assert_eq!(
        retry.events.len(),
        1,
        "{name}: accepted retry owns the flip event"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail on this test): M1 tags every cursor field except FB_ID/CRTC_ID, so a rejected
// FB_ID or CRTC_ID survives stripping; M3 blames every EINVAL without a probe; M4 inverts the full TEST_ONLY probe.
fn atomic_cursor_only_attribution_strips_every_cursor_field() {
    let Some((_, names)) = cursor_fixture(&Rc::new(RecordingCursorRecoveryReporter::default()))
    else {
        return;
    };
    for index in 0..names.len() {
        let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
        let Some((mut fixture, handles)) = cursor_fixture(&reporter) else {
            return;
        };
        let (name, handle) = handles[index];
        let cursor = cursor_only_handles(&handles);
        fixture.recorders.atomic.reject_properties(&[handle]);
        let _ = submit_visible_cursor_frame(&mut fixture);
        let records = fixture.recorders.atomic.records();
        assert_show_cursor_attribution_trace(
            &records,
            AtomicCommitKind::SceneWithCursor,
            &cursor,
            name,
        );
        assert_eq!(
            reporter.calls(),
            0,
            "{name}: a show rejection is not a detach"
        );
        assert!(
            !fixture.backend.platform.cursor_plane_visible_for_output(0),
            "{name}"
        );
        fixture.retire_output(0, 1);
        fixture.assert_cursor_quiescent();
        teardown(fixture);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail on this test): M1 tags every cursor field except FB_ID/CRTC_ID; M3 blames
// every EINVAL without a probe.
fn atomic_cursor_only_copied_attribution_strips_every_cursor_field() {
    use super::round_b_frames::install_copied_recording_route_with_hotspot;

    let Some(mut probe) =
        AtomicCursorFixture::new_with_hotspot_properties().expect("build copied cursor fixture")
    else {
        return;
    };
    install_copied_recording_route_with_hotspot(&mut probe, true);
    let names: Vec<_> = output_cursor_handles(&probe.backend)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    drop(probe);
    for (index, name) in names.into_iter().enumerate() {
        let Some(mut fixture) = AtomicCursorFixture::new_with_hotspot_properties()
            .expect("build copied cursor fixture")
        else {
            return;
        };
        let sink = install_copied_recording_route_with_hotspot(&mut fixture, true);
        let handles = output_cursor_handles(&fixture.backend);
        let cursor = cursor_only_handles(&handles);
        sink.reject_properties(&[handles[index].1]);
        let _ = submit_visible_cursor_frame(&mut fixture);
        assert_show_cursor_attribution_trace(
            &sink.records(),
            AtomicCommitKind::SceneWithCursor,
            &cursor,
            name,
        );
        sink.clear_property_rejections();
        fixture.retire_output(0, 1);
        fixture
            .backend
            .disable_output()
            .expect("copied teardown detaches the cursor");
        let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
        drop(fixture);
        assert_eq!(ledger.end_state(), (0, 0, 0), "{name}");
    }
}

const DIRECT_WINDOW: u32 = 0x0100_7805;

/// Held Vsync direct frame whose hardware cursor was hidden and re-shown, so
/// the next direct present carries the cursor attach. The hide is drained
/// through Backend flip delivery before the attach is requested.
fn held_direct_with_attach_pending() -> Option<(KmsBackend, Rc<RecordingAtomicCommitExecutor>, u32)>
{
    use super::super::tearing::{
        define_visible_cursor_through_backend, install_atomic_cursor_plane_stub,
        live_scene_backend_with_clock, materialize_test_cow_stage, prepare_visible_hw_cursor,
        prime_cursorless_scene_primary, recording_sink, retire_direct_present,
        try_live_direct_for_ids,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use yserver_core::{backend::Backend, server::ServerState};
    use yserver_protocol::x11::{ClientId, ResourceId};

    let clock_base = std::time::Instant::now();
    let scheduler_time_ns = std::sync::Arc::new(AtomicU64::new(0));
    let clock_elapsed_ns = std::sync::Arc::clone(&scheduler_time_ns);
    let clock = Rc::new(move || {
        clock_base + std::time::Duration::from_nanos(clock_elapsed_ns.load(Ordering::SeqCst))
    });
    let mut backend = live_scene_backend_with_clock(clock).map(|mut backend| {
        prime_cursorless_scene_primary(&mut backend);
        backend
    })?;
    for device in &mut backend.platform.devices {
        device.async_page_flip = true;
    }
    let recorder = recording_sink(&mut backend, 0);
    let mut state = ServerState::new();
    let window = ResourceId(DIRECT_WINDOW);
    let host_window = materialize_test_cow_stage(&mut backend, &mut state, ClientId(1), window);
    install_atomic_cursor_plane_stub(&mut backend);
    let _cursor = define_visible_cursor_through_backend(&mut backend, &mut state);
    prepare_visible_hw_cursor(&mut backend, &mut state);
    for index in 0..7_u64 {
        assert!(
            try_live_direct_for_ids(&mut backend, 12_650 + index, DIRECT_WINDOW, host_window, 0)
                .is_none()
        );
    }
    assert!(try_live_direct_for_ids(&mut backend, 12_657, DIRECT_WINDOW, host_window, 0).is_some());
    retire_direct_present(&mut backend, &mut state, 2);
    assert!(backend.pstate.scanout_m2.current.is_some());

    Backend::set_cursor_hidden(&mut backend, true);
    Backend::maybe_composite(&mut backend).expect("commit the hide under the held direct frame");
    drain_cursor_flip_event(&mut backend, &mut state);
    assert!(!backend.platform.any_cursor_commit_in_flight());
    Backend::set_cursor_hidden(&mut backend, false);
    recorder.clear_records();
    Some((backend, recorder, host_window))
}

fn run_direct_riding_attach(retry_succeeds: bool) {
    use super::super::tearing::prepare_direct_present_for_ids_with_storage;
    use yserver_core::{backend::Backend, server::ServerState};

    let Some((mut backend, recorder, host_window)) = held_direct_with_attach_pending() else {
        return;
    };
    let mut state = ServerState::new();
    let crtc = backend.platform.outputs[0].output.crtc;
    let primary_fb = backend.platform.outputs[0].output.plane_fb_id_prop;
    let old_direct = backend
        .pstate
        .scanout_m2
        .current
        .as_ref()
        .expect("held direct primary")
        .source_id;
    let handles = output_cursor_handles(&backend);
    let cursor = cursor_only_handles(&handles);
    let rejected_key = backend.platform.devices[0]
        .cursor
        .crtc_states
        .get(&crtc)
        .and_then(|state| state.intent)
        .and_then(|intent| match intent {
            crate::kms::render::platform::cursor_state::CursorIntent::Show(binding) => {
                binding.lease.sprite_key()
            }
            crate::kms::render::platform::cursor_state::CursorIntent::Hide => None,
        });
    let (candidate, event, _) = prepare_direct_present_for_ids_with_storage(
        &mut backend,
        12_675,
        DIRECT_WINDOW,
        host_window,
        800,
        600,
        0,
        true,
    );
    let before = live_records(&recorder).len();
    for result in [
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Ok(()),
        if retry_succeeds {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(libc::ENOMEM))
        },
    ] {
        recorder.script_result(result);
    }
    let result = Backend::try_present_direct(&mut backend, candidate, event);
    let live = live_records(&recorder);
    assert_eq!(
        live.len(),
        before + 2,
        "riding attach request and retry; result={result:?}; records={live:?}"
    );
    assert_eq!(live[before].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(live[before + 1].kind, AtomicCommitKind::Scene);
    assert_eq!(
        property_value(&live[before], primary_fb),
        property_value(&live[before + 1], primary_fb),
        "cursorless retry preserves the submitted direct primary",
    );
    assert_no_cursor_properties(&live[before + 1], &cursor);
    let state_after = &backend.platform.devices[0].cursor.crtc_states[&crtc];
    if retry_succeeds {
        assert!(result.expect("accepted cursorless direct retry"));
        assert_eq!(live[before + 1].result, ScriptedResult::Ok);
        assert_eq!(state_after.applied, None);
        assert_eq!(
            state_after.fallback,
            crate::kms::render::platform::cursor_state::CursorFallbackState::DetachPending {
                rejected_key,
            },
            "accepted retry queues detach using the refused request's captured key",
        );
        assert_eq!(
            state_after.intent,
            Some(crate::kms::render::platform::cursor_state::CursorIntent::Hide)
        );
        assert_eq!(
            backend
                .pstate
                .scanout_m2
                .current
                .as_ref()
                .map(|frame| frame.source_id),
            Some(old_direct),
            "the prior direct primary remains current until the accepted successor retires",
        );
        assert!(backend.pstate.scanout_m2.pending.is_some());

        drain_cursor_flip_event(&mut backend, &mut state);
        let detach = live_records(&recorder)
            .last()
            .cloned()
            .expect("cursor-only detach follows successor retirement");
        assert_eq!(detach.kind, AtomicCommitKind::CursorOnly);
        assert_eq!(detach.result, ScriptedResult::Ok);
        let fb_id = handles.iter().find(|(name, _)| *name == "fb_id").unwrap().1;
        let crtc_id = handles
            .iter()
            .find(|(name, _)| *name == "crtc_id")
            .unwrap()
            .1;
        assert_eq!(property_value(&detach, fb_id), Some(0));
        assert_eq!(property_value(&detach, crtc_id), Some(0));
        assert_eq!(
            backend.platform.devices[0].cursor.crtc_states[&crtc].applied,
            None
        );
        assert!(
            backend.platform.devices[0].cursor.crtc_states[&crtc]
                .in_flight
                .is_some()
        );
        drain_cursor_flip_event(&mut backend, &mut state);
        assert!(
            backend.platform.devices[0].cursor.crtc_states[&crtc]
                .in_flight
                .is_none()
        );
    } else {
        let error = result.expect_err("the failed primary retry keeps the direct producer error");
        assert_eq!(error.raw_os_error(), Some(libc::ENOMEM));
        assert_eq!(
            live[before + 1].result,
            ScriptedResult::Error {
                kind: std::io::Error::from_raw_os_error(libc::ENOMEM).kind(),
                raw_os_error: Some(libc::ENOMEM),
            },
        );
        assert!(backend.pstate.scanout_m2.pending.is_none());
        assert_eq!(
            backend
                .pstate
                .scanout_m2
                .current
                .as_ref()
                .map(|frame| frame.source_id),
            Some(old_direct),
        );
        assert_eq!(state_after.applied, None);
        assert!(state_after.in_flight.is_none());
        assert_eq!(
            state_after.fallback,
            crate::kms::render::platform::cursor_state::CursorFallbackState::Hardware
        );
        assert!(!backend.platform.any_cursor_commit_in_flight());
    }

    backend.platform.for_tests_accept_output_power_commits();
    backend
        .disable_output()
        .expect("Backend teardown after riding attach retry");
    let _ = Backend::drain_completed_present_events(&mut backend);
    let _ = Backend::drain_retired_present_idle_events(&mut backend);
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert!(backend.pstate.scanout_m2.current.is_none());
    backend.shutdown_destroy_drawables();
    assert!(backend.pstate.scanout_m2.queued_successor.is_none());
    assert!(!backend.platform.any_cursor_commit_in_flight());
    let ledger = Rc::clone(
        &backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording KMS services")
            .cursor_buffer_ledger,
    );
    drop(backend);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore the bound-to-bound-only retry guard; the no-prior attach must lose its primary retry.
fn atomic_cursor_only_riding_attach_retry_then_detaches() {
    run_direct_riding_attach(true);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: falsely accept the cursorless retry after ENOMEM; this direct path must preserve the producer error.
fn atomic_cursor_only_riding_attach_failed_retry_preserves_flip_error() {
    run_direct_riding_attach(false);
}

fn run_mixed_output_riding_attach(retry_succeeds: bool) {
    use super::{
        round_a::setup_cursor_only_direct_present, round_b_producers::admit_held_direct_cursorless,
    };
    use yserver_core::backend::Backend;

    let Some(mut fixture) = AtomicCursorFixture::new_with_two_outputs()
        .expect("build mixed-output riding cursor fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = setup_cursor_only_direct_present(&mut fixture);
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 810, 100);
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("establish cursorless output primaries");
    for output_idx in 0..2 {
        if fixture.backend.scene.has_pending_page_flip(output_idx) {
            fixture.retire_output(output_idx, 1 + output_idx as u32);
        }
    }
    let output0_crtc = fixture.backend.platform.outputs[0].output.crtc;
    let output1_crtc = fixture.backend.platform.outputs[1].output.crtc;
    let output0_device_key = fixture.backend.platform.outputs[0].key.device_key;
    let output1_device_key = fixture.backend.platform.outputs[1].key.device_key;
    for (output_idx, device_key, crtc) in [
        (0, output0_device_key, output0_crtc),
        (1, output1_device_key, output1_crtc),
    ] {
        assert_eq!(
            fixture
                .backend
                .platform
                .device_for_key(device_key)
                .and_then(|device| device.cursor.crtc_states.get(&crtc))
                .and_then(|state| state.applied),
            None,
            "output {output_idx} has no prior cursor binding",
        );
    }
    let baseline_receipts = [
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        fixture.backend.scene.for_tests_cursor_commit_receipt(1),
    ];
    let direct_present = admit_held_direct_cursorless(&mut fixture, &direct, 28_000, 20);
    assert!(fixture.backend.pstate.scanout_m2.current.is_some());

    // Reveal only after the direct primary is held. The direct Backend
    // successor must carry both unbound attaches in one atomic request.
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    let rejected_keys: Vec<_> = [
        (output0_device_key, output0_crtc),
        (output1_device_key, output1_crtc),
    ]
    .into_iter()
    .map(|(device_key, crtc)| {
        Some(
            fixture
                .backend
                .platform
                .device_for_key(device_key)
                .and_then(|device| device.cursor.crtc_states.get(&crtc))
                .and_then(|state| state.intent)
                .and_then(|intent| match intent {
                    crate::kms::render::platform::cursor_state::CursorIntent::Show(binding) => {
                        binding.lease.sprite_key()
                    }
                    crate::kms::render::platform::cursor_state::CursorIntent::Hide => None,
                })
                .expect("each unbound attach retains its submitted request key"),
        )
    })
    .collect();
    let old_direct = fixture
        .backend
        .pstate
        .scanout_m2
        .current
        .as_ref()
        .expect("held direct primary")
        .source_id;
    let (candidate, event, _) = super::super::tearing::prepare_direct_present_for_ids_with_storage(
        &mut fixture.backend,
        direct_present + 1,
        direct.window_xid,
        direct.window_xid,
        direct.width,
        direct.height,
        0,
        true,
    );
    let before = live_records(&fixture.recorders.atomic).len();
    let raw_before = fixture.recorders.atomic.records().len();
    for result in [
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Ok(()),
        if retry_succeeds {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(libc::ENOMEM))
        },
    ] {
        fixture.recorders.atomic.script_result(result);
    }
    let result = Backend::try_present_direct(&mut fixture.backend, candidate, event);
    let raw_records = fixture.recorders.atomic.records();
    let raw_batch = &raw_records[raw_before..];
    assert_eq!(
        raw_batch.len(),
        4,
        "full probe, stripped probe, then retry: {raw_batch:?}"
    );
    assert!(!raw_batch[0].flags.contains(AtomicCommitFlags::TEST_ONLY));
    assert!(raw_batch[1].flags.contains(AtomicCommitFlags::TEST_ONLY));
    assert!(raw_batch[2].flags.contains(AtomicCommitFlags::TEST_ONLY));
    assert!(!raw_batch[3].flags.contains(AtomicCommitFlags::TEST_ONLY));
    assert_eq!(raw_batch[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(raw_batch[1].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(raw_batch[2].kind, AtomicCommitKind::Scene);
    assert_eq!(raw_batch[3].kind, AtomicCommitKind::Scene);
    let records = live_records(&fixture.recorders.atomic);
    assert_eq!(
        records.len(),
        before + 2,
        "one mixed unbound-attach request and one cursor-stripped retry: {records:?}; result={result:?}",
    );
    assert_eq!(records[before].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(records[before + 1].kind, AtomicCommitKind::Scene);
    assert_eq!(
        records[before].result,
        ScriptedResult::Error {
            kind: std::io::Error::from_raw_os_error(libc::EINVAL).kind(),
            raw_os_error: Some(libc::EINVAL),
        }
    );
    assert_eq!(
        records[before + 1].result,
        if retry_succeeds {
            ScriptedResult::Ok
        } else {
            ScriptedResult::Error {
                kind: std::io::Error::from_raw_os_error(libc::ENOMEM).kind(),
                raw_os_error: Some(libc::ENOMEM),
            }
        }
    );
    let batch_crtcs: Vec<_> = records[before]
        .events
        .iter()
        .map(|event| event.crtc)
        .collect();
    assert!(batch_crtcs.contains(&output0_crtc));
    assert!(batch_crtcs.contains(&output1_crtc));
    let retry = &records[before + 1];
    for output in [
        &fixture.backend.platform.outputs[0],
        &fixture.backend.platform.outputs[1],
    ] {
        assert_eq!(
            property_value(&records[before], output.output.plane_fb_id_prop),
            property_value(retry, output.output.plane_fb_id_prop),
            "cursorless retry preserves output {:?}'s primary framebuffer",
            output.output.crtc,
        );
        let properties = fixture
            .backend
            .platform
            .device_for_key(output.key.device_key)
            .and_then(|device| device.cursor.plane.as_ref())
            .and_then(|plane| plane.properties_for(output.output.crtc))
            .expect("cursor plane properties for each output");
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
            retry
                .properties
                .iter()
                .all(|update| !cursor_properties.contains(&update.property))
        );
    }
    for (output_idx, device_key, crtc) in [
        (0, output0_device_key, output0_crtc),
        (1, output1_device_key, output1_crtc),
    ] {
        assert_eq!(
            fixture
                .backend
                .platform
                .device_for_key(device_key)
                .and_then(|device| device.cursor.crtc_states.get(&crtc))
                .and_then(|state| state.applied),
            None,
            "rejected output {output_idx} attach never publishes a binding",
        );
    }
    if retry_succeeds {
        assert!(result.expect("accepted mixed cursorless primary retry"));
        for (output_idx, expected_key) in [(0, rejected_keys[0]), (1, rejected_keys[1])] {
            let output = &fixture.backend.platform.outputs[output_idx];
            let state = fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                .expect("hardware-only output state");
            assert_eq!(
                state.fallback,
                crate::kms::render::platform::cursor_state::CursorFallbackState::DetachPending {
                    rejected_key: expected_key,
                },
                "output {output_idx} queues detach with its rejected request key",
            );
        }
        assert_eq!(
            fixture
                .backend
                .pstate
                .scanout_m2
                .current
                .as_ref()
                .map(|frame| frame.source_id),
            Some(old_direct),
            "the prior direct primary remains current until the accepted successor retires",
        );
        assert!(fixture.backend.pstate.scanout_m2.pending.is_some());
        assert_eq!(
            [
                fixture.backend.scene.for_tests_cursor_commit_receipt(0),
                fixture.backend.scene.for_tests_cursor_commit_receipt(1),
            ],
            baseline_receipts,
            "cursorless direct retry does not fabricate per-output cursor receipts",
        );
        fixture.retire_output(0, 40);
        fixture.retire_output(1, 41);
        for output_idx in 0..2 {
            if fixture
                .backend
                .platform
                .for_tests_atomic_cursor_commit_is_in_flight(
                    fixture.backend.platform.outputs[output_idx].key.device_key,
                    fixture.backend.platform.outputs[output_idx].output.crtc,
                )
            {
                fixture.retire_output(output_idx, 50 + output_idx as u32);
            }
        }
        for output in &fixture.backend.platform.outputs {
            assert_eq!(
                fixture
                    .backend
                    .platform
                    .device_for_key(output.key.device_key)
                    .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                    .and_then(|state| state.applied),
                None,
                "accepted explicit detach clears {:?}",
                output.output.crtc,
            );
        }
    } else {
        assert_eq!(
            result
                .expect_err("failed retry preserves direct producer error")
                .raw_os_error(),
            Some(libc::ENOMEM)
        );
        assert!(fixture.backend.pstate.scanout_m2.pending.is_none());
        assert_eq!(
            fixture
                .backend
                .pstate
                .scanout_m2
                .current
                .as_ref()
                .map(|frame| frame.source_id),
            Some(old_direct),
            "the original direct primary remains current after retry failure",
        );
        assert_eq!(
            [
                fixture.backend.scene.for_tests_cursor_commit_receipt(0),
                fixture.backend.scene.for_tests_cursor_commit_receipt(1),
            ],
            baseline_receipts,
            "failed mixed retry creates no receipt or event obligation",
        );
        assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
        for output in &fixture.backend.platform.outputs {
            let state = fixture
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
                .expect("cursor state remains after failed retry");
            assert!(state.in_flight.is_none());
            assert_eq!(
                state.fallback,
                crate::kms::render::platform::cursor_state::CursorFallbackState::Hardware
            );
        }
    }

    fixture
        .backend
        .disable_output()
        .expect("Backend teardown after mixed riding retry");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    while fixture.backend.scene.has_pending_page_flips() {
        for output_idx in 0..2 {
            if fixture.backend.scene.has_pending_page_flip(output_idx) {
                fixture.retire_output(output_idx, 60 + output_idx as u32);
            }
        }
    }
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore the bound-to-bound-only recovery guard; both unbound attaches must lose the primary retry.
fn atomic_cursor_only_mixed_output_riding_attach_retry_then_detaches() {
    run_mixed_output_riding_attach(true);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: falsely accept the cursorless retry after ENOMEM; the prior direct primary must remain current.
fn atomic_cursor_only_mixed_output_riding_attach_failed_retry_preserves_flip_error() {
    run_mixed_output_riding_attach(false);
}

/// Deliver the page-flip event of the outstanding cursor-only commit through
/// the same recording event source the fixture uses.
fn drain_cursor_flip_event(
    backend: &mut KmsBackend,
    state: &mut yserver_core::server::ServerState,
) {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    use yserver_core::backend::Backend;

    let crtc = backend.platform.outputs[0].output.crtc;
    let device = Rc::clone(&backend.platform.devices[0].device);
    let events = Rc::clone(
        &backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording KMS services")
            .page_flip_events,
    );
    events.register_device(&device);
    events.queue_flip_for_device(&device, crtc, 99);
    Backend::on_page_flip_ready(backend, state, device.as_fd().as_raw_fd());
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail on this test): M1 tags every cursor field except FB_ID/CRTC_ID; M3 blames
// every EINVAL without a probe.
fn atomic_cursor_only_direct_attribution_strips_every_cursor_field() {
    use super::super::tearing::prepare_direct_present_for_ids_with_storage;
    use yserver_core::{backend::Backend, server::ServerState};

    let Some((probe, _, _)) = held_direct_with_attach_pending() else {
        return;
    };
    let names: Vec<_> = output_cursor_handles(&probe)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    drop(probe);
    for (index, name) in names.into_iter().enumerate() {
        let Some((mut backend, recorder, host_window)) = held_direct_with_attach_pending() else {
            return;
        };
        let handles = output_cursor_handles(&backend);
        let cursor = cursor_only_handles(&handles);
        recorder.reject_properties(&[handles[index].1]);
        let (candidate, event, _shape) = prepare_direct_present_for_ids_with_storage(
            &mut backend,
            12_670,
            DIRECT_WINDOW,
            host_window,
            800,
            600,
            0,
            true,
        );
        let mut state = ServerState::new();
        let submitted = Backend::try_present_direct(&mut backend, candidate, event);
        assert!(
            submitted.expect("the cursorless direct retry accepts the primary"),
            "{name}",
        );
        assert_show_cursor_attribution_trace(
            &recorder.records(),
            AtomicCommitKind::SceneWithCursor,
            &cursor,
            name,
        );
        assert!(matches!(
            backend.platform.devices[0].cursor.crtc_states
                [&backend.platform.outputs[0].output.crtc]
                .fallback,
            crate::kms::render::platform::cursor_state::CursorFallbackState::DetachPending { .. }
        ));
        recorder.clear_property_rejections();
        drain_cursor_flip_event(&mut backend, &mut state);
        assert_eq!(
            live_records(&recorder).last().map(|record| record.kind),
            Some(AtomicCommitKind::CursorOnly),
            "{name}: accepted retry retires before the explicit detach",
        );
        drain_cursor_flip_event(&mut backend, &mut state);
        backend.platform.for_tests_accept_output_power_commits();
        backend
            .disable_output()
            .expect("direct teardown detaches the cursor");
        let ledger = Rc::clone(
            &backend
                .platform
                .services
                .test_recorders
                .as_ref()
                .expect("recording KMS services")
                .cursor_buffer_ledger,
        );
        drop(backend);
        assert_eq!(ledger.end_state(), (0, 0, 0), "{name}");
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation (run and seen to fail): M2 drops the first primary field when stripping, so the primary-only rejection
// becomes cursor-attributed and the frame is blamed on the cursor.
fn atomic_cursor_only_primary_rejection_is_frame_failure() {
    let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
    let Some((mut fixture, handles)) = cursor_fixture(&reporter) else {
        return;
    };
    let cursor = cursor_only_handles(&handles);
    let primary_fb = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
    fixture.recorders.atomic.reject_properties(&[primary_fb]);
    let _ = submit_visible_cursor_frame(&mut fixture);
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(records[0].result, einval());
    assert_eq!(records[1].result, einval(), "the full probe rejects");
    assert_eq!(records[2].kind, AtomicCommitKind::Scene);
    assert_no_cursor_properties(&records[2], &cursor);
    assert_eq!(
        records[2].result,
        einval(),
        "the stripped probe also rejects, so the frame, not the cursor, failed"
    );
    assert_eq!(reporter.calls(), 0);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    teardown(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation (run and seen to fail): M4 inverts the full TEST_ONLY probe, so an accepted full probe counts as a
// cursor rejection.
fn atomic_cursor_only_full_probe_acceptance_is_frame_failure() {
    let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
    let Some((mut fixture, handles)) = cursor_fixture(&reporter) else {
        return;
    };
    let cursor = cursor_only_handles(&handles);
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    fixture.recorders.atomic.script_result(Ok(()));
    let _ = submit_visible_cursor_frame(&mut fixture);
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(records[0].result, einval());
    assert!(records[1].flags.contains(AtomicCommitFlags::TEST_ONLY));
    assert_eq!(records[1].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(records[1].result, ScriptedResult::Ok);
    assert!(
        records[1]
            .properties
            .iter()
            .any(|update| cursor.contains(&update.property)),
        "the full probe carries the cursor fields it validated",
    );
    assert_eq!(reporter.calls(), 0);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    teardown(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation (run and seen to fail on this test): M5b makes the classifier ignore EBUSY, so the cursor-only wait
// becomes a cursor failure instead of waiting for the flip.
fn atomic_cursor_only_ebusy_is_never_cursor_failure() {
    use yserver_core::backend::Backend;

    let reporter = Rc::new(RecordingCursorRecoveryReporter::default());
    let Some((mut fixture, handles)) = cursor_fixture(&reporter) else {
        return;
    };
    let cursor = cursor_only_handles(&handles);
    submit_visible_cursor_frame(&mut fixture).expect("submit the accepted cursor show");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);
    let accepted = fixture.recorders.atomic.records().len();

    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EBUSY)));
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 125, 95);
    Backend::maybe_composite(&mut fixture.backend).expect("EBUSY waits for the flip");
    let records = fixture.recorders.atomic.records();
    assert_eq!(
        records.len(),
        accepted + 1,
        "EBUSY issues no TEST_ONLY attribution probe: {records:?}",
    );
    assert_eq!(records[accepted].kind, AtomicCommitKind::CursorOnly);
    assert_eq!(
        records[accepted].result,
        ScriptedResult::Error {
            kind: std::io::ErrorKind::ResourceBusy,
            raw_os_error: Some(libc::EBUSY),
        },
    );
    assert!(
        fixture.backend.platform.cursor_plane_visible_for_output(0),
        "EBUSY keeps the accepted hardware binding",
    );
    assert_eq!(
        fixture.backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
        "EBUSY is not a software fallback",
    );
    assert!(!fixture.backend.platform.scene_cursor_hide_failed);
    let device_key = fixture.backend.platform.outputs[0].key.device_key;
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    assert!(
        !fixture
            .backend
            .platform
            .for_tests_cursor_permanently_disabled(device_key)
    );
    assert!(fixture.backend.platform.cursor_retry_waits_for_flip(0));
    assert_eq!(reporter.calls(), 0);

    // Idle turns send nothing while the flip is outstanding.
    Backend::maybe_composite(&mut fixture.backend).expect("idle turn does not retry EBUSY");
    assert_eq!(fixture.recorders.atomic.records().len(), accepted + 1);

    // The real flip carries the waiting intent without any cursor probe.
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("real flip carries the intent");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), accepted + 2, "{records:?}");
    assert_eq!(
        records[accepted + 1].kind,
        AtomicCommitKind::SceneWithCursor
    );
    assert_eq!(records[accepted + 1].result, ScriptedResult::Ok);
    assert!(
        records[accepted..]
            .iter()
            .all(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY)),
        "no TEST_ONLY probe was issued on this path",
    );
    assert!(
        records[accepted + 1]
            .properties
            .iter()
            .any(|update| cursor.contains(&update.property)),
        "the retried frame carries the waiting cursor fields",
    );
    fixture.retire_output(0, 3);
    assert!(!fixture.backend.platform.cursor_retry_waits_for_flip(0));
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_atomic_cursor_pending_position(device_key, crtc),
        None,
        "the waiting intent settled with the real flip",
    );
    assert_eq!(reporter.calls(), 0);
    teardown(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation (run and seen to fail): M6 probes the Async request with TEST_ONLY, so a cursor-free Async rejection
// issues an attribution probe it must never issue.
fn atomic_cursor_only_async_einval_is_shape_fallback_not_cursor_blame() {
    use super::super::tearing::{
        assert_scene_async_vsync_pair, damage_root, enter_tearing_without_direct_scanout,
        live_scene_backend, recording_sink, retire_composed_frame,
    };
    use yserver_core::backend::Backend;

    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let mut state = enter_tearing_without_direct_scanout(&mut backend, 0x78b0, 15_500);
    assert!(backend.pstate.tearing.is_on());
    recorder.clear_records();
    let crtc = backend.platform.outputs[0].output.crtc;
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Ok(()));
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("Async EINVAL falls back to Vsync");

    let records = recorder.records();
    assert_eq!(records.len(), 2, "{records:?}");
    assert_scene_async_vsync_pair(&records, 0);
    assert_eq!(records[0].result, einval());
    assert!(
        records
            .iter()
            .all(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY)),
        "an Async shape rejection issues no cursor-attribution probe",
    );
    assert_no_cursor_properties(&records[0], &optional_cursor_handles(&backend));
    assert_eq!(
        backend
            .platform
            .async_outcomes
            .get(&crate::drm::async_flip::AsyncStream::Composed(crtc)),
        Some(&crate::drm::async_flip::AsyncOutcome::FellBack { suspended: false }),
    );
    assert!(!backend.platform.scene_cursor_hide_failed);
    retire_composed_frame(&mut backend, &mut state, 1);
    assert_eq!(recorder.records().len(), 2);
    backend.platform.for_tests_accept_output_power_commits();
    backend
        .disable_output()
        .expect("Backend teardown after the Async shape fallback");
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation (run and seen to fail): M6 probes the Async request with TEST_ONLY on the direct route.
fn atomic_cursor_only_direct_async_einval_is_shape_fallback_not_cursor_blame() {
    use super::super::tearing::{
        create_bitmap_cursor_via_backend, install_atomic_cursor_plane_stub, live_scene_backend,
        prepare_direct_present_for_ids, qualify_test_direct_async, recording_sink,
        retire_direct_present,
    };
    use yserver_core::{backend::Backend, server::ServerState};

    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    qualify_test_direct_async(&mut backend, &recorder, 0x7380, 3100);
    install_atomic_cursor_plane_stub(&mut backend);
    let cursor_fields = cursor_only_handles(&output_cursor_handles(&backend));
    let cursor_bitmap = create_bitmap_cursor_via_backend(&mut backend, true, true);
    let root = backend.core.window_id;
    Backend::define_cursor(&mut backend, None, root, cursor_bitmap.as_raw())
        .expect("define cursor through Backend");
    let mut state = ServerState::new();
    Backend::warp_pointer_root(&mut backend, &mut state, 123, 45);
    recorder.clear_records();
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Ok(()));
    let (mut candidate, mut event, shape) = prepare_direct_present_for_ids(
        &mut backend,
        31,
        0x7380,
        0x7380,
        800,
        600,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    );
    candidate.completion_dst_host_xid = 0x7380;
    event.dst_host_xid = 0x7380;
    let submitted = Backend::try_present_direct(&mut backend, candidate, event);
    assert!(
        submitted.is_ok(),
        "Async EINVAL falls back to Vsync: {submitted:?}; records {:?}",
        recorder
            .records()
            .iter()
            .map(|r| (r.kind, r.flags, r.result))
            .collect::<Vec<_>>(),
    );

    let records = recorder.records();
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].kind, AtomicCommitKind::Async);
    assert_eq!(records[0].result, einval());
    assert_eq!(records[1].kind, AtomicCommitKind::Scene);
    assert_eq!(records[1].result, ScriptedResult::Ok);
    assert!(
        records
            .iter()
            .all(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY)),
        "an Async shape rejection issues no cursor-attribution probe",
    );
    assert_no_cursor_properties(&records[0], &cursor_fields);
    assert_eq!(
        backend
            .pstate
            .scanout_m2
            .pending
            .as_ref()
            .map(|frame| frame.async_outcome),
        Some(crate::drm::async_flip::AsyncOutcome::FellBack { suspended: false }),
    );
    assert!(
        backend
            .platform
            .async_ledger
            .allows(crate::drm::async_flip::AsyncStream::Direct, &shape),
        "one Async EINVAL does not suspend the direct stream",
    );
    retire_direct_present(&mut backend, &mut state, 1);
    backend.platform.for_tests_accept_output_power_commits();
    backend
        .disable_output()
        .expect("Backend teardown after the direct Async shape fallback");
}

/// Held Vsync direct frame with the cursor attach pending: the composed unflip
/// requested by the window unmap is the next commit that can carry it.
fn held_direct_with_unflip_attach_pending() -> Option<(
    AtomicCursorFixture,
    super::round_a::CursorOnlyDirectPresentSetup,
)> {
    use super::round_a::{direct_present_for_id, setup_cursor_only_direct_present};
    use yserver_core::backend::Backend;

    let mut fixture =
        AtomicCursorFixture::new_with_hotspot_properties().expect("build unflip cursor fixture")?;
    fixture
        .backend
        .platform
        .for_tests_accept_output_power_commits();
    let direct = setup_cursor_only_direct_present(&mut fixture);
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial visible cursor frame");
    fixture.retire_output(0, 1);
    fixture.retire_output(0, 2);
    let mut entered = false;
    for attempt in 0..16_u64 {
        let (candidate, event) = direct_present_for_id(&direct, 25_000 + attempt);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit direct probe through Backend")
        {
            fixture.retire_output(0, 10 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend).expect("settle direct retirement");
            if fixture.backend.pstate.scanout_m2.active() {
                entered = true;
                break;
            }
        } else if fixture.backend.scene.has_pending_page_flips() {
            fixture.retire_output(0, 40 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle composed direct-probation frame");
        }
    }
    assert!(entered, "Backend direct Presents enter held scanout");
    // Hide and drain the hide's cursor-only commit under the held frame, then
    // re-show: the attach is pending when the unflip is built.
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    Backend::maybe_composite(&mut fixture.backend).expect("commit the hide under the held frame");
    fixture.retire_output(0, 60);
    Backend::set_cursor_hidden(&mut fixture.backend, false);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    Some((fixture, direct))
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail on this test): M1 tags every cursor field except FB_ID/CRTC_ID; M3 blames
// every EINVAL without a probe.
fn atomic_cursor_only_composed_unflip_attribution_strips_every_cursor_field() {
    use yserver_core::backend::Backend;

    let Some((probe, _)) = held_direct_with_unflip_attach_pending() else {
        return;
    };
    let names: Vec<_> = output_cursor_handles(&probe.backend)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    drop(probe);
    for (index, name) in names.into_iter().enumerate() {
        let Some((mut fixture, direct)) = held_direct_with_unflip_attach_pending() else {
            return;
        };
        let handles = output_cursor_handles(&fixture.backend);
        let cursor = cursor_only_handles(&handles);
        let before = fixture.recorders.atomic.records().len();
        fixture
            .recorders
            .atomic
            .reject_properties(&[handles[index].1]);
        Backend::unmap_subwindow(&mut fixture.backend, None, direct.window_xid)
            .expect("request composed unflip through Backend window unmap");
        let _ = Backend::maybe_composite(&mut fixture.backend);
        let records = fixture.recorders.atomic.records();
        assert_show_cursor_attribution_trace(
            &records[before..],
            AtomicCommitKind::SceneWithCursor,
            &cursor,
            name,
        );
        assert_eq!(records.len(), before + 4, "{name}: {records:?}");
        // The accepted cursorless unflip keeps the refused attachment unapplied.
        assert!(
            !fixture.backend.platform.cursor_plane_visible_for_output(0),
            "{name}"
        );
        assert!(
            fixture.backend.pstate.scanout_m2.active(),
            "{name}: direct primary stays owned until the unflip event"
        );
        assert!(matches!(
            fixture.backend.platform.devices[0].cursor.crtc_states
                [&fixture.backend.platform.outputs[0].output.crtc]
                .fallback,
            crate::kms::render::platform::cursor_state::CursorFallbackState::DetachPending { .. }
        ));
        fixture.recorders.atomic.clear_property_rejections();
        fixture.retire_output(0, 61);
        if fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
        {
            fixture.retire_output(0, 62);
        }
        teardown_after_unflip(fixture);
    }
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail): report the retry accepted without issuing it; omit primary fields from the stripped retry; or settle the refused lease as accepted instead of retaining the prior binding through detach.
fn atomic_cursor_only_direct_successor_riding_failure_retries_primary_then_detaches() {
    use super::super::tearing::prepare_direct_present_for_ids_with_storage;
    use yserver_core::{backend::Backend, server::ServerState};

    let Some((mut backend, recorder, host_window)) = held_direct_with_attach_pending() else {
        return;
    };
    let mut state = ServerState::new();
    Backend::maybe_composite(&mut backend).expect("attach cursor while direct primary is held");
    assert!(backend.platform.cursor_plane_visible_for_output(0));
    drain_cursor_flip_event(&mut backend, &mut state);
    let old_binding = backend.platform.devices[0]
        .cursor
        .crtc_states
        .get(&backend.platform.outputs[0].output.crtc)
        .and_then(|state| state.applied)
        .expect("accepted direct hardware cursor binding");
    let old_direct = backend
        .pstate
        .scanout_m2
        .current
        .as_ref()
        .expect("held direct primary")
        .source_id;
    Backend::warp_pointer_root(&mut backend, &mut state, 145, 115);
    let (candidate, event, _) = prepare_direct_present_for_ids_with_storage(
        &mut backend,
        12_670,
        DIRECT_WINDOW,
        host_window,
        800,
        600,
        0,
        true,
    );
    let handles = output_cursor_handles(&backend);
    let cursor = cursor_only_handles(&handles);
    let primary_fb = backend.platform.outputs[0].output.plane_fb_id_prop;
    let output = &backend.platform.outputs[0].output;
    let direct_primary_fields = [
        output.plane_crtc_id_prop,
        output.plane_src_x_prop,
        output.plane_src_y_prop,
        output.plane_src_w_prop,
        output.plane_src_h_prop,
    ];
    let before = live_records(&recorder).len();
    for result in [
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Ok(()),
        Ok(()),
    ] {
        recorder.script_result(result);
    }
    assert!(
        Backend::try_present_direct(&mut backend, candidate, event)
            .expect("accepted cursorless direct primary retry")
    );
    let records = live_records(&recorder);
    assert_eq!(records.len(), before + 2);
    assert_eq!(records[before].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(records[before + 1].kind, AtomicCommitKind::Scene);
    assert_eq!(records[before + 1].result, ScriptedResult::Ok);
    assert_eq!(
        property_value(&records[before], primary_fb),
        property_value(&records[before + 1], primary_fb),
        "direct retry keeps the submitted primary FB",
    );
    for primary_field in direct_primary_fields {
        assert!(
            records[before + 1]
                .properties
                .iter()
                .any(|update| update.property == primary_field)
        );
    }
    assert_no_cursor_properties(&records[before + 1], &cursor);
    assert_eq!(
        backend
            .pstate
            .scanout_m2
            .current
            .as_ref()
            .map(|frame| frame.source_id),
        Some(old_direct),
        "the old direct primary remains current until successor retirement",
    );
    assert!(backend.pstate.scanout_m2.pending.is_some());
    assert_eq!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&backend.platform.outputs[0].output.crtc)
            .and_then(|state| state.applied),
        Some(old_binding),
        "the old binding survives the accepted primary retry",
    );
    assert!(!backend.platform.any_cursor_commit_in_flight());

    drain_cursor_flip_event(&mut backend, &mut state);
    let detach = live_records(&recorder)
        .last()
        .cloned()
        .expect("cursor-only detach follows accepted direct successor retirement");
    assert_eq!(detach.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(detach.result, ScriptedResult::Ok);
    let fb_id = handles.iter().find(|(name, _)| *name == "fb_id").unwrap().1;
    let crtc_id = handles
        .iter()
        .find(|(name, _)| *name == "crtc_id")
        .unwrap()
        .1;
    assert_eq!(property_value(&detach, fb_id), Some(0));
    assert_eq!(property_value(&detach, crtc_id), Some(0));
    assert_eq!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&backend.platform.outputs[0].output.crtc)
            .and_then(|state| state.applied),
        None,
    );
    assert!(backend.platform.any_cursor_commit_in_flight());
    drain_cursor_flip_event(&mut backend, &mut state);
    assert!(!backend.platform.any_cursor_commit_in_flight());

    backend.platform.for_tests_accept_output_power_commits();
    backend.disable_output().expect("Backend direct teardown");
    let _ = Backend::drain_completed_present_events(&mut backend);
    let _ = Backend::drain_retired_present_idle_events(&mut backend);
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert!(backend.pstate.scanout_m2.current.is_none());
    assert!(backend.pstate.scanout_m2.queued_successor.is_none());
    let ledger = Rc::clone(
        &backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording services")
            .cursor_buffer_ledger,
    );
    drop(backend);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation (run and seen to fail): swallow the cursorless direct retry's ENOMEM as success or install a receipt/event for its rejected primary.
fn atomic_cursor_only_direct_retry_failure_keeps_original_flip_error_path() {
    use super::super::tearing::prepare_direct_present_for_ids_with_storage;
    use yserver_core::{backend::Backend, server::ServerState};

    let Some((mut backend, recorder, host_window)) = held_direct_with_attach_pending() else {
        return;
    };
    let mut state = ServerState::new();
    Backend::maybe_composite(&mut backend).expect("attach cursor while direct primary is held");
    drain_cursor_flip_event(&mut backend, &mut state);
    let old_binding = backend.platform.devices[0]
        .cursor
        .crtc_states
        .get(&backend.platform.outputs[0].output.crtc)
        .and_then(|state| state.applied)
        .expect("accepted cursor binding");
    let old_direct = backend
        .pstate
        .scanout_m2
        .current
        .as_ref()
        .expect("held direct primary")
        .source_id;
    Backend::warp_pointer_root(&mut backend, &mut state, 145, 115);
    let (candidate, event, _) = prepare_direct_present_for_ids_with_storage(
        &mut backend,
        12_671,
        DIRECT_WINDOW,
        host_window,
        800,
        600,
        0,
        true,
    );
    let before = live_records(&recorder).len();
    for result in [
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Ok(()),
        Err(std::io::Error::from_raw_os_error(libc::ENOMEM)),
    ] {
        recorder.script_result(result);
    }
    let error = Backend::try_present_direct(&mut backend, candidate, event)
        .expect_err("the retry failure follows the direct producer error path");
    assert_eq!(error.raw_os_error(), Some(libc::ENOMEM));
    let live = live_records(&recorder);
    assert_eq!(live.len(), before + 2);
    assert_eq!(live[before].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(live[before + 1].kind, AtomicCommitKind::Scene);
    assert_eq!(
        live[before + 1].result,
        ScriptedResult::Error {
            kind: std::io::Error::from_raw_os_error(libc::ENOMEM).kind(),
            raw_os_error: Some(libc::ENOMEM),
        },
    );
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert_eq!(
        backend
            .pstate
            .scanout_m2
            .current
            .as_ref()
            .map(|frame| frame.source_id),
        Some(old_direct),
        "the previous direct primary remains current",
    );
    assert_eq!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&backend.platform.outputs[0].output.crtc)
            .and_then(|state| state.applied),
        Some(old_binding),
        "the rejected retry leaves applied cursor state unchanged",
    );
    let direct_crtc = backend.platform.outputs[0].output.crtc;
    let direct_epoch = backend.platform.devices[0].cursor.cursor_topology_epoch;
    assert!(
        backend.platform.devices[0].cursor.crtc_states[&direct_crtc]
            .qualification
            .rejected_in_epoch(direct_epoch, old_binding.lease.sprite_key()),
        "direct cursor rejection is remembered even when its cursorless retry fails",
    );
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert!(backend.pstate.scanout_m2.queued_successor.is_none());

    backend.platform.for_tests_accept_output_power_commits();
    backend
        .disable_output()
        .expect("Backend teardown after retry error");
    let _ = Backend::drain_completed_present_events(&mut backend);
    let _ = Backend::drain_retired_present_idle_events(&mut backend);
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert!(backend.pstate.scanout_m2.current.is_none());
    backend.shutdown_destroy_drawables();
    let ledger = Rc::clone(
        &backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording services")
            .cursor_buffer_ledger,
    );
    drop(backend);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: settle rejected unflip cursor properties as accepted, compose software before detach, remove primary fields from the retry, or leave an accepted recovery detach in DetachPending so requalified motion detaches again.
fn atomic_cursor_only_composed_unflip_riding_failure_retries_primary_then_detaches() {
    use yserver_core::backend::Backend;

    let Some((mut fixture, direct)) = held_direct_with_unflip_attach_pending() else {
        return;
    };
    Backend::maybe_composite(&mut fixture.backend).expect("accept the cursor show under direct");
    fixture.retire_output(0, 60);
    let old_binding = applied_binding(&fixture).expect("accepted HW binding under direct");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 145, 115);
    let before = live_records(&fixture.recorders.atomic).len();
    for result in [
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Ok(()),
        Ok(()),
    ] {
        fixture.recorders.atomic.script_result(result);
    }
    Backend::unmap_subwindow(&mut fixture.backend, None, direct.window_xid)
        .expect("request composed unflip through Backend window unmap");
    Backend::maybe_composite(&mut fixture.backend).expect("retry rejected unflip primary");

    let live = live_records(&fixture.recorders.atomic);
    assert_eq!(live.len(), before + 2, "rejected unflip plus primary retry");
    assert_eq!(live[before].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(live[before + 1].kind, AtomicCommitKind::Scene);
    assert_eq!(live[before + 1].result, ScriptedResult::Ok);
    let handles = output_cursor_handles(&fixture.backend);
    let cursor = cursor_only_handles(&handles);
    assert_no_cursor_properties(&live[before + 1], &cursor);
    let output = &fixture.backend.platform.outputs[0].output;
    let primary_fb = output.plane_fb_id_prop;
    assert_eq!(
        property_value(&live[before], primary_fb),
        property_value(&live[before + 1], primary_fb),
        "unflip retry keeps its original composed primary",
    );
    assert_eq!(
        fixture
            .backend
            .scene
            .for_tests_cursor_commit_receipt(0)
            .expect("accepted composed unflip receipt")
            .mode,
        crate::kms::render::scene::CursorAcceptedMode::Hardware,
        "the retained primary stays HW-owned until cursor-only detach",
    );
    assert_eq!(applied_binding(&fixture), Some(old_binding));
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(
        !fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0)
    );
    assert!(
        !fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );

    // The §3 recovery detach is an explicit cursor-only commit even though scene damage is pending.
    let before_retire = live_records(&fixture.recorders.atomic).len();
    fixture.retire_output(0, 61);
    assert_eq!(
        live_records(&fixture.recorders.atomic).len(),
        before_retire + 1,
        "the completing event submits the recovery detach",
    );
    let detach = live_records(&fixture.recorders.atomic)[before_retire].clone();
    assert_eq!(detach.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(detach.result, ScriptedResult::Ok);
    let fb_id = handles.iter().find(|(name, _)| *name == "fb_id").unwrap().1;
    let crtc_id = handles
        .iter()
        .find(|(name, _)| *name == "crtc_id")
        .unwrap()
        .1;
    let crtc_x = handles
        .iter()
        .find(|(name, _)| *name == "crtc_x")
        .unwrap()
        .1;
    let crtc_y = handles
        .iter()
        .find(|(name, _)| *name == "crtc_y")
        .unwrap()
        .1;
    assert_eq!(property_value(&detach, fb_id), Some(0));
    assert_eq!(property_value(&detach, crtc_id), Some(0));
    assert_eq!(applied_binding(&fixture), None);
    assert!(
        fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    fixture.retire_output(0, 62);
    Backend::maybe_composite(&mut fixture.backend).expect("compose the software fallback");
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    fixture.retire_output(0, 63);
    assert!(
        fixture
            .backend
            .scene
            .for_tests_cursor_footprint_was_presented(0),
        "the software cursor is visible after the explicit detach retires",
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0)
    );
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0,
    );

    let output = &fixture.backend.platform.outputs[0];
    let device_key = output.key.device_key;
    let crtc = output.output.crtc;
    let settled_fallback = fixture
        .backend
        .platform
        .device_for_key(device_key)
        .expect("cursor device")
        .cursor
        .crtc_states[&crtc]
        .fallback;
    assert!(
        matches!(
            settled_fallback,
            crate::kms::render::platform::cursor_state::CursorFallbackState::Software { .. }
        ),
        "accepted detach settles fallback out of DetachPending: {settled_fallback:?}",
    );

    // A new hotspot key can requalify after the accepted recovery detach. Its
    // later motion must update the binding instead of issuing another detach.
    let requalified = super::round_b_producers::create_argb_cursor_extent(
        &mut fixture.backend,
        [0x71, 0x42, 0x33, 0xff],
        4,
        4,
        1,
        0,
    );
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, requalified.as_raw())
        .expect("requalify a different cursor key through Backend");
    Backend::maybe_composite(&mut fixture.backend).expect("accept the requalified cursor frame");
    let requalified_record = live_records(&fixture.recorders.atomic)
        .last()
        .cloned()
        .expect("requalification scene commit");
    assert_eq!(requalified_record.kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(requalified_record.result, ScriptedResult::Ok);
    let requalified_binding = applied_binding(&fixture).expect("requalified cursor binding");
    fixture.retire_output(0, 64);
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 165, 135);
    let before_motion = live_records(&fixture.recorders.atomic).len();
    Backend::maybe_composite(&mut fixture.backend).expect("move the requalified hardware cursor");
    let after_motion = live_records(&fixture.recorders.atomic);
    let motion = after_motion[before_motion..]
        .iter()
        .find(|record| !record.flags.contains(AtomicCommitFlags::TEST_ONLY))
        .expect("motion commit");
    assert_eq!(motion.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(
        property_value(motion, fb_id),
        None,
        "motion preserves the binding"
    );
    assert_eq!(
        applied_binding(&fixture).unwrap().framebuffer,
        requalified_binding.framebuffer
    );
    assert_eq!(
        property_value(motion, crtc_id),
        None,
        "motion does not detach"
    );
    assert_eq!(property_value(motion, crtc_x), Some(164));
    assert_eq!(property_value(motion, crtc_y), Some(135));
    fixture.retire_output(0, 65);
    let recovery_sequence: Vec<_> = live_records(&fixture.recorders.atomic)[before_retire..]
        .iter()
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        recovery_sequence,
        [
            AtomicCommitKind::CursorOnly,
            AtomicCommitKind::Scene,
            AtomicCommitKind::SceneWithCursor,
            AtomicCommitKind::CursorOnly,
        ],
        "recovery detach, software fallback, requalification, then motion",
    );
    teardown_after_unflip(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation (run and seen to fail): swallow the cursorless unflip retry's ENOMEM as an accepted presentation.
fn atomic_cursor_only_composed_unflip_retry_failure_keeps_original_flip_error_path() {
    use yserver_core::backend::Backend;

    let Some((mut fixture, direct)) = held_direct_with_unflip_attach_pending() else {
        return;
    };
    Backend::maybe_composite(&mut fixture.backend).expect("accept the cursor show under direct");
    fixture.retire_output(0, 60);
    let old_binding = applied_binding(&fixture).expect("accepted HW binding under direct");
    let old_direct = fixture
        .backend
        .pstate
        .scanout_m2
        .current
        .as_ref()
        .expect("held direct primary")
        .source_id;
    let receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("baseline accepted HW receipt");
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 145, 115);
    let before = live_records(&fixture.recorders.atomic).len();
    for result in [
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Ok(()),
        Err(std::io::Error::from_raw_os_error(libc::ENOMEM)),
    ] {
        fixture.recorders.atomic.script_result(result);
    }
    Backend::unmap_subwindow(&mut fixture.backend, None, direct.window_xid)
        .expect("request composed unflip through Backend window unmap");
    Backend::maybe_composite(&mut fixture.backend)
        .expect("ordinary composed flip path follows a failed atomic unflip");

    let live = live_records(&fixture.recorders.atomic);
    assert_eq!(
        live.len(),
        before + 3,
        "rejected unflip, failed retry, then ordinary composed flip",
    );
    assert_eq!(live[before].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(live[before + 1].kind, AtomicCommitKind::Scene);
    assert_eq!(
        live[before + 1].result,
        ScriptedResult::Error {
            kind: std::io::Error::from_raw_os_error(libc::ENOMEM).kind(),
            raw_os_error: Some(libc::ENOMEM),
        },
    );
    let handles = output_cursor_handles(&fixture.backend);
    let cursor = cursor_only_handles(&handles);
    assert_no_cursor_properties(&live[before + 1], &cursor);
    let primary_fb = fixture.backend.platform.outputs[0].output.plane_fb_id_prop;
    assert_eq!(
        property_value(&live[before], primary_fb),
        property_value(&live[before + 1], primary_fb),
        "failed retry keeps the submitted composed primary",
    );
    let unflip_output = &fixture.backend.platform.outputs[0];
    let unflip_epoch = fixture
        .backend
        .platform
        .device_for_key(unflip_output.key.device_key)
        .expect("unflip cursor device")
        .cursor
        .cursor_topology_epoch;
    assert!(
        fixture
            .backend
            .platform
            .device_for_key(unflip_output.key.device_key)
            .expect("unflip cursor device")
            .cursor
            .crtc_states[&unflip_output.output.crtc]
            .qualification
            .rejected_in_epoch(unflip_epoch, old_binding.lease.sprite_key()),
        "unflip cursor rejection is remembered even when its cursorless retry fails",
    );
    assert_eq!(live[before + 2].kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(live[before + 2].result, ScriptedResult::Ok);
    assert_eq!(live[before + 2].events.len(), 1);
    assert_eq!(
        property_value(&live[before + 2], cursor[0]),
        Some(0),
        "the later software frame explicitly detaches the rejected plane",
    );
    assert_eq!(property_value(&live[before + 2], cursor[1]), Some(0));
    assert!(fixture.backend.scene.has_pending_page_flip(0));
    let followup_receipt = fixture
        .backend
        .scene
        .for_tests_cursor_commit_receipt(0)
        .expect("only the later accepted ordinary scene frame has a new receipt");
    assert_eq!(
        followup_receipt.mode,
        crate::kms::render::scene::CursorAcceptedMode::Software
    );
    assert!(matches!(
        followup_receipt.primary_cursor,
        crate::kms::render::scene::CursorPrimaryContent::Software { .. }
    ));
    assert_ne!(followup_receipt.frame, receipt.frame);
    assert_eq!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .current
            .as_ref()
            .map(|frame| frame.source_id),
        Some(old_direct),
        "the old direct primary remains retained until the ordinary flip retires",
    );
    let output = &fixture.backend.platform.outputs[0];
    let state = fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
        .expect("cursor state remains available during accepted ordinary flip");
    assert_eq!(
        state.in_flight.as_ref().and_then(|commit| commit.prior),
        Some(old_binding)
    );
    assert_eq!(applied_binding(&fixture), None);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert!(fixture.backend.platform.any_cursor_commit_in_flight());

    fixture.retire_output(0, 63);
    assert!(!fixture.backend.scene.has_pending_page_flip(0));
    assert!(fixture.backend.pstate.scanout_m2.current.is_none());
    assert_eq!(applied_binding(&fixture), None);
    assert!(
        fixture
            .backend
            .pstate
            .scanout_m2
            .unflip_awaiting_outputs
            .is_empty()
    );
    assert!(!fixture.backend.platform.any_cursor_commit_in_flight());
    assert!(fixture.backend.platform.staged_cursor_frames.is_empty());
    assert_eq!(
        fixture
            .backend
            .platform
            .for_tests_pending_scanout_render_completion_count(),
        0,
    );

    teardown_after_unflip(fixture);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations (each run and seen to fail): restore the cached pre-On sprite instead of the latest live sprite, or accept a rejected Vsync update without preserving its prior lease and identity.
fn atomic_cursor_only_tearing_exit_restores_latest_intent_after_async_retirement() {
    use super::{
        super::tearing::{
            damage_root, live_scene_backend, qualify_test_direct_async, recording_sink,
            retire_composed_frame, retire_direct_present, try_direct_present_with_live_storage,
        },
        round_b_producers::create_argb_cursor_extent,
    };
    use yserver_core::{backend::Backend, server::ServerState};

    const WINDOW: u32 = 0x7802;
    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let mut state = ServerState::new();
    qualify_test_direct_async(&mut backend, &recorder, WINDOW, 12_600);
    super::super::tearing::install_atomic_cursor_plane_stub(&mut backend);
    let root = backend.core.window_id;
    let cached_pre_on_sprite = backend
        .effective_cursor_xid
        .expect("the tearing fixture has a cursor before the newer intent");
    let cached_pre_on_key = backend
        .cursor_records
        .get(&cached_pre_on_sprite)
        .map(|record| (record.width, record.height, record.hot_x, record.hot_y));
    let latest_sprite =
        create_argb_cursor_extent(&mut backend, [0x20, 0x30, 0xe0, 0xff], 32, 32, 1, 0);
    Backend::define_cursor(&mut backend, None, root, latest_sprite.as_raw())
        .expect("queue a newer cursor sprite while tearing is On");
    assert_ne!(cached_pre_on_key, Some((32, 32, 1, 0)));
    Backend::warp_pointer_root(&mut backend, &mut state, 160, 140);
    let old_shape = try_direct_present_with_live_storage(
        &mut backend,
        12_699,
        WINDOW,
        800,
        600,
        yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
    )
    .expect("submit the old Async primary while tearing is On");
    assert_eq!(
        recorder.records().last().unwrap().kind,
        AtomicCommitKind::Async
    );
    assert!(backend.pstate.scanout_m2.pending.is_some());

    let cursor_handles = output_cursor_handles(&backend);
    let cursor = cursor_only_handles(&cursor_handles);
    let output_crtc = backend.platform.outputs[0].output.crtc;
    let primary_fb = backend.platform.outputs[0].output.plane_fb_id_prop;
    let crtc_x = cursor_handles
        .iter()
        .find(|(name, _)| *name == "crtc_x")
        .unwrap()
        .1;
    let primary_async = recorder
        .records()
        .last()
        .cloned()
        .expect("Async request record");
    assert_eq!(primary_async.kind, AtomicCommitKind::Async);
    assert_no_cursor_properties(&primary_async, &cursor);
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert!(
        backend.platform.devices[0]
            .cursor
            .crtc_states
            .get(&output_crtc)
            .is_none_or(|state| state.in_flight.is_none())
    );

    // Showing the cursor and unmapping its direct owner are real Backend
    // policy edges. The unmap exits tearing while the older Async event still
    // owns the CRTC; neither transition may submit overlapping cursor work.
    Backend::unmap_subwindow(&mut backend, None, WINDOW)
        .expect("unmap the tearing owner through Backend");
    assert!(!backend.pstate.tearing.is_on());
    let records_before_drain = recorder.records().len();
    Backend::maybe_composite(&mut backend).expect("keep the Async CRTC event as the boundary");
    assert_eq!(recorder.records().len(), records_before_drain);
    assert!(!backend.platform.any_cursor_commit_in_flight());
    assert!(backend.pstate.scanout_m2.pending.is_some());

    retire_direct_present(&mut backend, &mut state, 41);
    assert!(backend.pstate.scanout_m2.current.is_some());
    assert!(backend.pstate.scanout_m2.unflip_requested);
    Backend::maybe_composite(&mut backend).expect("submit the Vsync composed handoff");
    assert!(!recorder.records().is_empty());
    let handoff = recorder.records().last().cloned().expect("handoff request");
    assert_eq!(handoff.kind, AtomicCommitKind::Scene);
    assert_no_cursor_properties(&handoff, &cursor);
    assert!(backend.pstate.scanout_m2.current.is_some());
    retire_composed_frame(&mut backend, &mut state, 42);
    assert!(backend.pstate.scanout_m2.current.is_none());

    damage_root(&mut backend);
    Backend::set_cursor_hidden(&mut backend, false);
    Backend::maybe_composite(&mut backend).expect("restore the latest visible cursor");
    assert!(backend.scene.has_pending_page_flip(0));
    let show = recorder
        .records()
        .last()
        .cloned()
        .expect("accepted SW-to-HW frame");
    assert_eq!(show.kind, AtomicCommitKind::SceneWithCursor);
    assert!(!show.flags.contains(AtomicCommitFlags::TEST_ONLY));
    assert_eq!(show.events.len(), 1);
    let latest_binding = backend.platform.devices[0]
        .cursor
        .crtc_states
        .get(&output_crtc)
        .and_then(|state| state.applied)
        .unwrap_or_else(|| panic!("accepted scene show installs the latest hardware binding: records={:?}, state={:?}", recorder.records(), backend.platform.devices[0].cursor.crtc_states.get(&output_crtc)));
    assert_eq!(latest_binding.lease.sprite_key(), Some((32, 32, 1, 0)));
    assert_eq!((latest_binding.x, latest_binding.y), (159, 140));
    assert_eq!(
        show.properties
            .iter()
            .find(|update| update.property == crtc_x)
            .map(|update| update.value),
        Some(159),
        "the accepted scene show uses the latest queued motion",
    );
    retire_composed_frame(&mut backend, &mut state, 43);
    assert!(!backend.scene.has_pending_page_flip(0));
    assert!(backend.platform.async_ledger.for_tests_is_empty());

    Backend::warp_pointer_root(&mut backend, &mut state, 190, 160);
    damage_root(&mut backend);
    for result in [
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
        Ok(()),
        Ok(()),
    ] {
        recorder.script_result(result);
    }
    let before_vsync = live_records(&recorder).len();
    Backend::maybe_composite(&mut backend).expect("retry the rejected Vsync hardware update");
    let live = live_records(&recorder);
    assert_eq!(live.len(), before_vsync + 2);
    let rejected = &live[before_vsync];
    let retry = &live[before_vsync + 1];
    assert_eq!(rejected.kind, AtomicCommitKind::SceneWithCursor);
    assert_eq!(retry.kind, AtomicCommitKind::Scene);
    assert_eq!(retry.result, ScriptedResult::Ok);
    assert_no_cursor_properties(retry, &cursor);
    assert_eq!(
        property_value(rejected, crtc_x),
        Some(189),
        "the rejected request uses the latest live cursor position",
    );
    assert_eq!(
        property_value(rejected, primary_fb),
        property_value(retry, primary_fb),
        "Vsync cursorless retry retains the original primary",
    );
    assert_eq!(applied_binding_for_backend(&backend), Some(latest_binding));
    let fallback = backend.platform.devices[0]
        .cursor
        .crtc_states
        .get(&output_crtc)
        .map(|state| state.fallback);
    assert_eq!(
        fallback,
        Some(
            crate::kms::render::platform::cursor_state::CursorFallbackState::DetachPending {
                rejected_key: Some((32, 32, 1, 0)),
            }
        ),
        "recovery retains the rejected submitted sprite key",
    );
    assert!(backend.scene.has_pending_page_flip(0));
    retire_composed_frame(&mut backend, &mut state, 44);
    assert!(backend.platform.cursor_commit_in_flight_for_output(0));
    let detach = live_records(&recorder)
        .last()
        .cloned()
        .expect("accepted detach");
    assert_eq!(detach.kind, AtomicCommitKind::CursorOnly);
    assert_eq!(detach.result, ScriptedResult::Ok);
    let fb_id = cursor_handles
        .iter()
        .find(|(name, _)| *name == "fb_id")
        .unwrap()
        .1;
    let crtc_id = cursor_handles
        .iter()
        .find(|(name, _)| *name == "crtc_id")
        .unwrap()
        .1;
    assert_eq!(property_value(&detach, fb_id), Some(0));
    assert_eq!(property_value(&detach, crtc_id), Some(0));
    retire_composed_frame(&mut backend, &mut state, 45);
    assert!(!backend.platform.cursor_commit_in_flight_for_output(0));
    assert_eq!(applied_binding_for_backend(&backend), None);
    assert!(backend.platform.staged_cursor_frames.is_empty());
    assert!(backend.platform.async_ledger.for_tests_is_empty());
    assert_ne!(old_shape.width, 0);

    backend.platform.for_tests_accept_output_power_commits();
    backend
        .disable_output()
        .expect("Backend teardown after tearing recovery");
    let _ = Backend::drain_completed_present_events(&mut backend);
    let _ = Backend::drain_retired_present_idle_events(&mut backend);
    assert!(!backend.scene.has_pending_page_flips());
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert!(backend.pstate.scanout_m2.current.is_none());
    let _ledger = Rc::clone(
        &backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording services")
            .cursor_buffer_ledger,
    );
    drop(backend);
    assert_eq!(_ledger.end_state(), (0, 0, 0));
}

fn applied_binding_for_backend(
    backend: &KmsBackend,
) -> Option<crate::kms::render::platform::cursor_state::CursorBinding> {
    let output = &backend.platform.outputs[0];
    backend
        .platform
        .device_for_key(output.key.device_key)
        .and_then(|device| device.cursor.crtc_states.get(&output.output.crtc))
        .and_then(|state| state.applied)
}

/// Teardown for a held direct frame: the blocking disable, then the present
/// events it retires, before the cursor is checked for quiescence.
fn teardown_after_unflip(mut fixture: AtomicCursorFixture) {
    use yserver_core::backend::Backend;

    fixture.recorders.atomic.clear_property_rejections();
    fixture
        .backend
        .disable_output()
        .expect("blocking Backend teardown detaches the cursor");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);
    fixture.assert_cursor_quiescent();
    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    fixture.shutdown_drawables();
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
