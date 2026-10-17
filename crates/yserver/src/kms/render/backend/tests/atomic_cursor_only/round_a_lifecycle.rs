use super::support::AtomicCursorFixture;
use crate::drm::atomic::tests_support::AtomicCommitRecord;
use std::rc::Rc;

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

fn assert_property(
    record: &AtomicCommitRecord,
    object: impl Into<::drm::control::RawResourceHandle>,
    property: ::drm::control::property::Handle,
    value: u64,
) {
    let object = object.into();
    assert!(
        record.properties.iter().any(|update| {
            update.object == object && update.property == property && update.value == value
        }),
        "commit {:?} omitted property object={object:?} property={property:?} value={value}",
        record.kind,
    );
}

fn assert_full_attach(
    fixture: &AtomicCursorFixture,
    record: &AtomicCommitRecord,
    framebuffer: u32,
    x: i32,
    y: i32,
) {
    let output = &fixture.backend.platform.outputs[0].output;
    let key = fixture.backend.platform.outputs[0].key.device_key;
    let crtc = output.crtc;
    let plane = fixture
        .backend
        .platform
        .device_for_key(key)
        .and_then(|device| device.cursor.plane.as_ref())
        .expect("cursor plane")
        .properties_for(crtc)
        .expect("eligible cursor properties");
    let cursor_plane: ::drm::control::plane::Handle = plane.plane;
    for (property, value) in [
        (plane.fb_id, u64::from(framebuffer)),
        (plane.crtc_id, u64::from(u32::from(crtc))),
        (plane.crtc_x, i64::from(x) as u64),
        (plane.crtc_y, i64::from(y) as u64),
        (plane.src_x, 0),
        (plane.src_y, 0),
        (
            plane.src_w,
            u64::from(
                fixture
                    .backend
                    .platform
                    .device_for_key(key)
                    .unwrap()
                    .cursor
                    .plane
                    .as_ref()
                    .unwrap()
                    .width(),
            ) << 16,
        ),
        (
            plane.src_h,
            u64::from(
                fixture
                    .backend
                    .platform
                    .device_for_key(key)
                    .unwrap()
                    .cursor
                    .plane
                    .as_ref()
                    .unwrap()
                    .height(),
            ) << 16,
        ),
        (
            plane.crtc_w,
            u64::from(
                fixture
                    .backend
                    .platform
                    .device_for_key(key)
                    .unwrap()
                    .cursor
                    .plane
                    .as_ref()
                    .unwrap()
                    .width(),
            ),
        ),
        (
            plane.crtc_h,
            u64::from(
                fixture
                    .backend
                    .platform
                    .device_for_key(key)
                    .unwrap()
                    .cursor
                    .plane
                    .as_ref()
                    .unwrap()
                    .height(),
            ),
        ),
    ] {
        assert_property(record, cursor_plane, property, value);
    }
    if let Some(property) = plane.hotspot_x {
        assert_property(record, cursor_plane, property, 0);
    }
    if let Some(property) = plane.hotspot_y {
        assert_property(record, cursor_plane, property, 0);
    }
}

fn assert_detached_ids(
    record: &AtomicCommitRecord,
    properties: crate::kms::cursor_plane::topology::CursorPlaneProperties,
) {
    let plane: ::drm::control::plane::Handle = properties.plane;
    assert_property(record, plane, properties.fb_id, 0);
    assert_property(record, plane, properties.crtc_id, 0);
}

fn attached_fixture() -> Option<AtomicCursorFixture> {
    use yserver_core::backend::Backend;

    let mut fixture = AtomicCursorFixture::new().expect("build cursor fixture")?;
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    fixture
        .backend
        .for_tests_set_vt_resume_executor(Rc::new(LogicalVtResume));
    fixture.define_visible_cursor();
    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, 100, 90);
    super::round_a::damage_cursor_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.backend).expect("submit initial cursor scene");
    for frame in 1..8 {
        if fixture.backend.scene.has_pending_page_flips() {
            fixture.retire_output(0, frame);
        }
        if fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
        {
            fixture.retire_output(0, frame + 10);
        }
        if fixture.backend.platform.cursor_plane_visible_for_output(0)
            && !fixture.backend.scene.has_pending_page_flips()
            && !fixture
                .backend
                .platform
                .cursor_atomic_event_pending_for_output(0)
        {
            break;
        }
        Backend::maybe_composite(&mut fixture.backend).expect("continue cursor scene");
    }
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    Some(fixture)
}

fn queue_pending_cursor_event(fixture: &AtomicCursorFixture, frame: u32) {
    let output = &fixture.backend.platform.outputs[0];
    let device = &fixture
        .backend
        .platform
        .device_for_key(output.key.device_key)
        .expect("output device")
        .device;
    fixture
        .recorders
        .page_flip_events
        .queue_flip_for_device(device, output.output.crtc, frame);
}

fn make_pending_cursor_move(fixture: &mut AtomicCursorFixture, x: i32, y: i32) {
    use yserver_core::backend::Backend;

    Backend::warp_pointer_root(&mut fixture.backend, &mut fixture.state, x, y);
    Backend::maybe_composite(&mut fixture.backend).expect("schedule cursor movement");
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0),
        "Backend motion accepts a cursor-only commit before the lifecycle boundary",
    );
}

fn retire_cursor_event(fixture: &mut AtomicCursorFixture, frame: u32) {
    if fixture
        .backend
        .platform
        .cursor_atomic_event_pending_for_output(0)
    {
        fixture.retire_output(0, frame);
    }
}

fn transparent_cursor(fixture: &mut AtomicCursorFixture) -> u32 {
    use yserver_core::backend::{AnyHandle, Backend};

    let source = Backend::create_pixmap(&mut fixture.backend, None, 32, 1, 1)
        .expect("create transparent cursor source");
    Backend::put_image(
        &mut fixture.backend,
        None,
        source.as_raw(),
        32,
        1,
        1,
        0,
        0,
        &[0, 0, 0, 0],
    )
    .expect("write transparent cursor pixel");
    let picture = Backend::render_create_picture(
        &mut fixture.backend,
        None,
        AnyHandle::Pixmap(source),
        0,
        0,
        &[],
    )
    .expect("create transparent cursor picture")
    .expect("cursor picture");
    Backend::render_create_cursor(&mut fixture.backend, None, picture, 0, 0)
        .expect("create transparent cursor")
        .expect("cursor handle")
        .as_raw()
}

fn clone_output_for_recorded_discovery(
    output: &crate::platform::drm::Output,
) -> crate::platform::drm::Output {
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

fn enter_direct_scanout(fixture: &mut AtomicCursorFixture) {
    use yserver_core::backend::Backend;

    let setup = super::round_a::setup_cursor_only_direct_present(fixture);
    let mut entered = false;
    for attempt in 0..16_u64 {
        let (candidate, event) = super::round_a::direct_present_for_id(&setup, 0xa800 + attempt);
        Backend::note_present_scanout_candidate(&mut fixture.backend, candidate);
        if Backend::try_present_direct(&mut fixture.backend, candidate, event)
            .expect("submit direct scanout probe through Backend")
        {
            fixture.retire_output(0, 30 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle direct scanout probe through Backend");
            if fixture.backend.pstate.scanout_m2.active() {
                entered = true;
                break;
            }
        } else if fixture.backend.scene.has_pending_page_flips() {
            fixture.retire_output(0, 60 + u32::try_from(attempt).expect("frame fits"));
            Backend::maybe_composite(&mut fixture.backend)
                .expect("settle composed direct-probe frame");
        }
    }
    assert!(entered, "Backend direct Presents reach held direct scanout");
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: skip lifecycle cursor invalidation, hide reconciliation when direct is inactive, omit its full detached state, clear a lease before the old-event drain, or rearm through a legacy cursor operation.
fn atomic_cursor_only_lifecycle_invalidates_and_drains() {
    use yserver_core::{backend::Backend, core_loop::input_inventory::InputInventory};

    let Some(mut fixture) = attached_fixture() else {
        return;
    };
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let cursor_properties = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .expect("cursor plane")
        .properties_for(crtc)
        .expect("cursor properties");
    let prior = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("Backend attached visible cursor");

    // A cursor-only move remains in flight when DPMS disables the CRTC. Its
    // event must drain before the lifecycle owner can release the framebuffer.
    make_pending_cursor_move(&mut fixture, 101, 90);
    let pre_dpms_binding = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("accepted pre-DPMS cursor move");
    queue_pending_cursor_event(&fixture, 41);
    let before_dpms_off = fixture.recorders.atomic.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("DPMS blocking disable");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_dpms_off + 1);
    let disable = records.last().expect("blocking disable");
    assert_eq!(disable.kind, crate::drm::atomic::AtomicCommitKind::Scene);
    assert_eq!(
        disable.flags,
        ::drm::control::AtomicCommitFlags::ALLOW_MODESET
    );
    assert!(
        disable.events.is_empty(),
        "blocking all-off expects no replacement event"
    );
    assert!(disable.properties.iter().all(|update| update.value == 0));
    assert_detached_ids(disable, cursor_properties);
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(prior.lease),
        Some(0),
        "the pending cursor event drains before its final lifecycle owner releases",
    );
    fixture.assert_cursor_quiescent();

    // DPMS re-lights the primary CRTC, then the accepted visible cursor is
    // rebound with every applicable property after comparison invalidation.
    let before_dpms_on = records.len();
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("DPMS enable and cursor rebind");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_dpms_on + 2);
    assert_eq!(
        records[before_dpms_on].kind,
        crate::drm::atomic::AtomicCommitKind::Scene
    );
    let wake_cursor = &records[before_dpms_on + 1];
    assert_eq!(
        wake_cursor.kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    let rebound = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("DPMS wake accepts the cursor binding");
    assert_full_attach(
        &fixture,
        wake_cursor,
        rebound.framebuffer,
        pre_dpms_binding.x,
        pre_dpms_binding.y,
    );
    retire_cursor_event(&mut fixture, 42);

    // Reach held direct scanout through Backend. VT release must run direct
    // teardown, atomically detach in the same primary disable, and drain the
    // old cursor event before resume can reclaim/reuse its lease.
    enter_direct_scanout(&mut fixture);
    make_pending_cursor_move(&mut fixture, 140, 100);
    queue_pending_cursor_event(&fixture, 43);
    let before_vt_release = fixture.recorders.atomic.records().len();
    Backend::finish_vt_release(
        &mut fixture.backend,
        &mut fixture.state,
        &InputInventory::default(),
    );
    assert!(!fixture.backend.pstate.scanout_m2.active());
    assert_eq!(
        fixture.backend.pstate.vt_state,
        crate::vt::state::VtState::Suspended
    );
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_vt_release + 1);
    let vt_disable = records.last().expect("VT blocking disable");
    assert_eq!(vt_disable.kind, crate::drm::atomic::AtomicCommitKind::Scene);
    assert_eq!(
        vt_disable.flags,
        ::drm::control::AtomicCommitFlags::ALLOW_MODESET
    );
    assert!(vt_disable.events.is_empty());
    assert!(vt_disable.properties.iter().all(|update| update.value == 0));
    assert_detached_ids(vt_disable, cursor_properties);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(prior.lease),
        Some(0),
        "VT lifecycle drains the old cursor event before releasing its owner",
    );

    // C1 regression: select a fully transparent cursor while the VT is
    // suspended, after real direct teardown has removed direct ownership.
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    let transparent = transparent_cursor(&mut fixture);
    let root = fixture.backend.core.window_id;
    Backend::define_cursor(&mut fixture.backend, None, root, transparent)
        .expect("define transparent cursor while suspended");
    assert!(fixture.backend.effective_cursor_hidden());
    assert_eq!(fixture.recorders.atomic.records().len(), records.len());

    let before_vt_resume = fixture.recorders.atomic.records().len();
    Backend::on_vt_acquire(&mut fixture.backend, &mut fixture.state);
    assert_eq!(
        fixture.backend.pstate.vt_state,
        crate::vt::state::VtState::Active
    );
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_vt_resume + 2);
    assert_eq!(
        records[before_vt_resume].kind,
        crate::drm::atomic::AtomicCommitKind::Scene
    );
    let hidden_resume = &records[before_vt_resume + 1];
    assert_eq!(
        hidden_resume.kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert_detached_ids(hidden_resume, cursor_properties);
    assert!(!fixture.backend.platform.cursor_plane_visible_for_output(0));
    retire_cursor_event(&mut fixture, 44);

    // DPMS wake follows the same hidden atomic reconciliation even with
    // direct scanout inactive. Both ID properties are explicitly zero.
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("DPMS off after VT resume");
    let before_hidden_wake = fixture.recorders.atomic.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("hidden DPMS wake");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_hidden_wake + 2);
    assert_eq!(
        records[before_hidden_wake].kind,
        crate::drm::atomic::AtomicCommitKind::Scene
    );
    let hidden_wake = &records[before_hidden_wake + 1];
    assert_eq!(
        hidden_wake.kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert_detached_ids(hidden_wake, cursor_properties);
    retire_cursor_event(&mut fixture, 45);
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("final blocking disable");
    let _ = Backend::drain_completed_present_events(&mut fixture.backend);
    let _ = Backend::drain_retired_present_idle_events(&mut fixture.backend);

    fixture.assert_cursor_quiescent();
    assert!(fixture.recorders.legacy_cursor.records().is_empty());
    assert!(fixture.recorders.mode_blobs.live_count() == 0);
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));

    // Exercise the real Backend RandR modeset entry with the production
    // output-power path. This Vk fixture intentionally has no cursor plane,
    // so the synthetic DRM error cannot quarantine a test cursor framebuffer.
    // The entry reaches direct object-property lookup (ENOTTY); real mode-blob
    // identity through a modeset remains C11 hardware validation.
    let backend = crate::kms::render::backend::KmsBackend::for_tests_with_vk()
        .expect("build Vulkan modeset fixture");
    let recorders = Rc::clone(
        backend
            .platform
            .services
            .test_recorders
            .as_ref()
            .expect("recording KMS services"),
    );
    let mut modeset_fixture = AtomicCursorFixture {
        backend,
        recorders,
        state: yserver_core::server::ServerState::new(),
    };
    modeset_fixture
        .backend
        .initialize_provider_output_sources()
        .expect("initialize provider route before RANDR modeset");
    let output =
        clone_output_for_recorded_discovery(&modeset_fixture.backend.platform.outputs[0].output);
    let connector = output.connector_name.clone();
    let output_id = Backend::randr_outputs_and_modes(&mut modeset_fixture.backend)
        .0
        .into_iter()
        .find(|candidate| candidate.name == connector)
        .expect("RandR output")
        .output_id;
    modeset_fixture
        .recorders
        .crtc_output_discovery
        .queue_output(output);
    let before_attempt = modeset_fixture.recorders.atomic.records().len();
    let error = Backend::apply_crtc_config(
        &mut modeset_fixture.backend,
        output_id,
        &connector,
        Some(yserver_core::backend::ModeSpec {
            width: 800,
            height: 600,
            vrefresh: 60,
        }),
        13,
        0,
    )
    .expect_err("synthetic DRM stops at direct property lookup");
    assert_eq!(error.raw_os_error(), Some(libc::ENOTTY));
    assert_eq!(modeset_fixture.recorders.crtc_output_discovery.calls(), 1);
    assert_eq!(
        modeset_fixture.recorders.atomic.records().len(),
        before_attempt
    );
    assert_eq!(modeset_fixture.backend.platform.outputs.len(), 1);
    assert_eq!(modeset_fixture.backend.platform.outputs[0].x, 0);
    assert_eq!(modeset_fixture.recorders.mode_blobs.live_count(), 0);
    modeset_fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&modeset_fixture.recorders.cursor_buffer_ledger);
    drop(modeset_fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: clear actual cursor users when a blocking disable fails, or clear lifecycle/cache references before old events drain; retained ownership and retry assertions must fail.
fn atomic_cursor_only_failed_disable_retains_users() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = attached_fixture() else {
        return;
    };
    let crtc = fixture.backend.platform.outputs[0].output.crtc;
    let key = fixture.backend.platform.outputs[0].key.device_key;
    let initial = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("visible cursor binding");
    let pool = fixture.backend.platform.devices[0]
        .cursor
        .plane
        .as_ref()
        .unwrap();
    let initial_users = pool.sprite_user_count(initial.lease);
    assert_eq!(initial_users, Some(1));

    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    let before_failed_disable = fixture.recorders.atomic.records().len();
    let error = Backend::set_dpms_power(&mut fixture.backend, 3)
        .expect_err("injected lifecycle disable failure propagates");
    assert!(
        error
            .to_string()
            .contains("disable_output atomic commit rejected")
    );
    assert_eq!(
        fixture.recorders.atomic.records().len(),
        before_failed_disable + 1
    );
    assert!(matches!(
        fixture.recorders.atomic.records().last().unwrap().result,
        crate::drm::atomic::tests_support::ScriptedResult::Error {
            raw_os_error: Some(libc::EIO),
            ..
        }
    ));
    let failed_state = &fixture.backend.platform.devices[0].cursor.crtc_states[&crtc];
    assert_eq!(failed_state.applied, Some(initial));
    assert!(
        failed_state.valid,
        "rejected disable retains the accepted comparison"
    );
    assert!(failed_state.lifecycle_pending.is_none());
    assert!(fixture.backend.platform.cursor_plane_visible_for_output(0));
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(initial.lease),
        initial_users,
        "rejected disable retains the on-screen framebuffer user",
    );

    // Recover the active CRTC through Backend, then make a successful blocking
    // disable/drain before treating the route as resumable.
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("recover active output after failure");
    let recovered_state = &fixture.backend.platform.devices[0].cursor.crtc_states[&crtc];
    assert_eq!(recovered_state.applied, Some(initial));
    assert!(
        !recovered_state.valid,
        "accepted modeset invalidates comparison only"
    );
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(initial.lease),
        initial_users,
        "modeset invalidation preserves the current owner's lease",
    );

    make_pending_cursor_move(&mut fixture, initial.x + 1, initial.y);
    let full_state_move = fixture.recorders.atomic.records().last().unwrap().clone();
    assert!(matches!(
        full_state_move.kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
            | crate::drm::atomic::AtomicCommitKind::SceneWithCursor
    ));
    assert_full_attach(
        &fixture,
        &full_state_move,
        initial.framebuffer,
        initial.x + 1,
        initial.y,
    );
    queue_pending_cursor_event(&fixture, 51);
    Backend::set_dpms_power(&mut fixture.backend, 3)
        .expect("successful recovery disable and drain");
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(initial.lease),
        Some(0),
    );
    fixture.assert_cursor_quiescent();

    let before_recovery_wake = fixture.recorders.atomic.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 0)
        .expect("resume after successful disable/drain");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_recovery_wake + 2);
    let rebound = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("successful recovery wake rebinds cursor");
    assert_full_attach(
        &fixture,
        &records[before_recovery_wake + 1],
        rebound.framebuffer,
        initial.x + 1,
        initial.y,
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );

    // A failed old-event drain must also retain the lifecycle owner. A later
    // successful disable drains that event and only then releases its user.
    queue_pending_cursor_event(&fixture, 52);
    fixture
        .recorders
        .page_flip_events
        .fail_next_drain(libc::EIO);
    let error = Backend::set_dpms_power(&mut fixture.backend, 3)
        .expect_err("injected old-event drain failure propagates");
    assert_eq!(error.raw_os_error(), Some(libc::EIO));
    let drain_failed = &fixture.backend.platform.devices[0].cursor.crtc_states[&crtc];
    assert!(drain_failed.lifecycle_pending.is_some());
    assert!(drain_failed.in_flight.is_some());
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 1);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(rebound.lease),
        Some(1),
        "failed event drain retains the cursor framebuffer until retry",
    );

    Backend::set_dpms_power(&mut fixture.backend, 3)
        .expect("successful repeated disable drains old cursor event");
    assert_eq!(fixture.recorders.page_flip_events.queued_flip_count(), 0);
    assert_eq!(
        fixture.backend.platform.devices[0]
            .cursor
            .plane
            .as_ref()
            .unwrap()
            .sprite_user_count(rebound.lease),
        Some(0),
        "successful retry releases the lifecycle owner only after drain",
    );

    let before_final_wake = fixture.recorders.atomic.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 0).expect("resume after successful retry");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_final_wake + 2);
    let final_binding = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("final wake cursor binding");
    assert_full_attach(
        &fixture,
        &records[before_final_wake + 1],
        final_binding.framebuffer,
        initial.x + 1,
        initial.y,
    );
    retire_cursor_event(&mut fixture, 54);
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("final all-off teardown");
    fixture.assert_cursor_quiescent();
    assert!(fixture.recorders.legacy_cursor.records().is_empty());
    assert!(fixture.backend.platform.device_for_key(key).is_some());
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}
