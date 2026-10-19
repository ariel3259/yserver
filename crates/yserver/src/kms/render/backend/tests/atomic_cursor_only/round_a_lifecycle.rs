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

fn cursor_properties_for_output(
    fixture: &AtomicCursorFixture,
    output_idx: usize,
) -> crate::kms::cursor_plane::topology::CursorPlaneProperties {
    let layout = &fixture.backend.platform.outputs[output_idx];
    fixture
        .backend
        .platform
        .device_for_key(layout.key.device_key)
        .and_then(|device| device.cursor.plane.as_ref())
        .and_then(|plane| plane.properties_for(layout.output.crtc))
        .expect("eligible output cursor properties")
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
    assert!(
        entered,
        "Backend direct Presents reach held direct scanout: mode={:?}, transition_pending={}, scene_flip={}, primary_ready={}, cursor_receipt={:?}, cursor_visible={}, scene_wants={}",
        fixture.backend.scene.cursor_mode(),
        fixture.backend.scene.cursor_transition_pending(),
        fixture.backend.scene.has_pending_page_flips(),
        fixture.backend.scene.direct_scanout_primary_ready(),
        fixture.backend.scene.for_tests_cursor_commit_receipt(0),
        fixture.backend.platform.cursor_plane_visible_for_output(0),
        fixture.backend.for_tests_scene_wants_scene(),
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: skip lifecycle/modeset cursor invalidation, hide reconciliation when direct is inactive, omit its full detached state, clear a lease before the old-event drain, or rearm through a legacy cursor operation.
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
    assert!(fixture.recorders.mode_blobs.live_count() == 0);
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));

    // RandR set_crtc_config uses the same production modeset builder as DPMS.
    // This fixture has an initialized cursor plane; the accepted primary
    // modeset must be assembled and recorded before the route is re-lit.
    let Some(mut modeset_fixture) = attached_fixture() else {
        return;
    };
    let output =
        clone_output_for_recorded_discovery(&modeset_fixture.backend.platform.outputs[0].output);
    let connector = output.connector_name.clone();
    let mode = yserver_core::backend::ModeSpec {
        width: output.picked.width,
        height: output.picked.height,
        vrefresh: output.picked.vrefresh,
    };
    let output_id = Backend::randr_outputs_and_modes(&mut modeset_fixture.backend)
        .0
        .into_iter()
        .find(|candidate| candidate.name == connector)
        .expect("RandR output")
        .output_id;
    let providers = Backend::randr_providers(&mut modeset_fixture.backend);
    let source_id = providers
        .iter()
        .find(|provider| {
            !provider.is_gpu
                && provider.outputs.is_empty()
                && provider.capabilities
                    & yserver_protocol::x11::randr::PROVIDER_CAPABILITY_SOURCE_OUTPUT
                    != 0
        })
        .expect("selected render source provider")
        .provider_id;
    let sink_id = providers
        .iter()
        .find(|provider| provider.outputs.contains(&output_id))
        .expect("KMS sink provider")
        .provider_id;
    modeset_fixture.state.randr.set_providers(providers);
    assert!(
        Backend::set_provider_output_source(
            &mut modeset_fixture.backend,
            &mut modeset_fixture.state,
            sink_id,
            Some(source_id),
        )
        .expect("attach output to selected renderer through Backend")
    );
    modeset_fixture
        .recorders
        .crtc_output_discovery
        .queue_output(output);
    let before_modeset = modeset_fixture.recorders.atomic.records().len();
    let before_blobs = modeset_fixture.recorders.mode_blobs.records().len();
    assert!(
        Backend::apply_crtc_config(
            &mut modeset_fixture.backend,
            output_id,
            &connector,
            Some(mode),
            13,
            0,
        )
        .expect("Backend RandR set_crtc_config reaches the shared production modeset")
    );
    assert_eq!(modeset_fixture.backend.platform.outputs[0].x, 13);
    let records = modeset_fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_modeset + 3);
    assert!(records[before_modeset..].iter().all(|record| {
        record.kind == crate::drm::atomic::AtomicCommitKind::Scene
            && record.flags == ::drm::control::AtomicCommitFlags::ALLOW_MODESET
            && record.events.is_empty()
    }));
    assert!(
        records[before_modeset]
            .properties
            .iter()
            .all(|update| update.value == 0)
    );
    let created_blobs: Vec<_> = modeset_fixture.recorders.mode_blobs.records()[before_blobs..]
        .iter()
        .filter_map(|record| match record {
            crate::drm::io::ModeBlobRecord::Create { blob, .. } => Some(*blob),
            crate::drm::io::ModeBlobRecord::Destroy { .. } => None,
        })
        .collect();
    assert_eq!(
        created_blobs.len(),
        2,
        "enable and relight each create a mode blob"
    );
    let crtc = modeset_fixture.backend.platform.outputs[0].output.crtc;
    let mode_id = ::drm::control::from_u32(0xD102).expect("recorded CRTC MODE_ID property");
    assert_property(
        &records[before_modeset + 1],
        crtc,
        mode_id,
        created_blobs[0],
    );
    assert_property(
        &records[before_modeset + 2],
        crtc,
        mode_id,
        created_blobs[1],
    );
    assert_eq!(modeset_fixture.recorders.mode_blobs.live_count(), 0);
    let modeset_state = &modeset_fixture.backend.platform.devices[0]
        .cursor
        .crtc_states[&crtc];
    assert!(
        !modeset_state.valid,
        "modeset leaves cursor comparison invalidated"
    );
    assert!(modeset_state.applied.is_none());

    Backend::set_dpms_power(&mut modeset_fixture.backend, 3)
        .expect("modeset fixture final all-off teardown");
    assert_eq!(modeset_fixture.recorders.mode_blobs.live_count(), 0);
    modeset_fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&modeset_fixture.recorders.cursor_buffer_ledger);
    drop(modeset_fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: reconcile hidden cursor intent on a CRTC whose wake failed, or omit the enabled-CRTC submission guard; failed-route intent/event assertions must fail.
fn atomic_cursor_only_partial_wake_reconciles_only_enabled_routes() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) =
        AtomicCursorFixture::new_with_two_outputs().expect("build two-output partial-wake fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    Backend::set_cursor_hidden(&mut fixture.backend, true);
    let cursor0 = cursor_properties_for_output(&fixture, 0);
    let cursor1 = cursor_properties_for_output(&fixture, 1);

    let before_off = fixture.recorders.atomic.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("disable both outputs before wake");
    let off_records = fixture.recorders.atomic.records();
    assert_eq!(off_records.len(), before_off + 2);
    assert_detached_ids(&off_records[before_off], cursor0);
    assert_detached_ids(&off_records[before_off + 1], cursor1);

    fixture.recorders.atomic.script_result(Ok(()));
    fixture
        .recorders
        .atomic
        .script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    let before_wake = fixture.recorders.atomic.records().len();
    let error = Backend::set_dpms_power(&mut fixture.backend, 0)
        .expect_err("second output wake is rejected after the first succeeds");
    assert!(error.to_string().contains("atomic modeset commit rejected"));
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_wake + 3);
    assert_eq!(
        records[before_wake].kind,
        crate::drm::atomic::AtomicCommitKind::Scene
    );
    assert_eq!(
        records[before_wake + 1].kind,
        crate::drm::atomic::AtomicCommitKind::Scene
    );
    assert_eq!(
        records[before_wake + 2].kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert_detached_ids(&records[before_wake + 2], cursor0);
    assert!(
        !records[before_wake + 2]
            .properties
            .iter()
            .any(|update| update.object == cursor1.plane.into())
    );
    assert!(fixture.backend.platform.cursor_output_is_active(0));
    assert!(!fixture.backend.platform.cursor_output_is_active(1));
    assert!(
        fixture.backend.platform.devices[0].cursor.crtc_states
            [&fixture.backend.platform.outputs[1].output.crtc]
            .intent
            .is_none()
    );
    assert!(
        fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(0)
    );
    assert!(
        !fixture
            .backend
            .platform
            .cursor_atomic_event_pending_for_output(1)
    );
    fixture.retire_output(0, 61);

    let before_retry = fixture.recorders.atomic.records().len();
    Backend::set_dpms_power(&mut fixture.backend, 0)
        .expect("retry both output modesets after partial wake");
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_retry + 4);
    assert_eq!(
        records[before_retry + 2].kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert_eq!(
        records[before_retry + 3].kind,
        crate::drm::atomic::AtomicCommitKind::CursorOnly
    );
    assert_detached_ids(&records[before_retry + 2], cursor0);
    assert_detached_ids(&records[before_retry + 3], cursor1);
    fixture.retire_output(0, 62);
    fixture.retire_output(1, 63);
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("final all-off after partial wake");
    fixture.assert_cursor_quiescent();
    assert_eq!(fixture.recorders.mode_blobs.live_count(), 0);
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: remove the cursor detach from connector-removal's blocking disable; the recorded same-request IDs and framebuffer end state must fail.
fn atomic_cursor_only_reprobe_removal_detaches_before_reclaim() {
    use yserver_core::backend::Backend;

    let Some(mut fixture) = attached_fixture() else {
        return;
    };
    let cursor = cursor_properties_for_output(&fixture, 0);
    let base = std::time::Instant::now();
    let elapsed_ms = Rc::new(std::cell::Cell::new(0_u64));
    let clock_ms = Rc::clone(&elapsed_ms);
    fixture
        .backend
        .for_tests_set_monotonic_clock(Rc::new(move || {
            base + std::time::Duration::from_millis(clock_ms.get())
        }));
    fixture
        .recorders
        .connector_snapshot
        .queue_snapshot(Vec::new());
    fixture
        .recorders
        .hotplug
        .queue_change()
        .expect("queue output removal edge");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    elapsed_ms.set(150);
    let before_removal = fixture.recorders.atomic.records().len();
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert!(fixture.backend.platform.outputs.is_empty());
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_removal + 1);
    let disable = records.last().expect("blocking removal disable");
    assert_eq!(disable.kind, crate::drm::atomic::AtomicCommitKind::Scene);
    assert_eq!(
        disable.flags,
        ::drm::control::AtomicCommitFlags::ALLOW_MODESET
    );
    assert!(disable.events.is_empty());
    assert!(disable.properties.iter().all(|update| update.value == 0));
    assert_detached_ids(disable, cursor);
    fixture.assert_cursor_quiescent();
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: clear actual cursor users when a blocking disable fails, clear lifecycle/cache references before old events drain, or wake before the successful recovery disable; retained ownership, order, and retry assertions must fail.
fn atomic_cursor_only_failed_disable_retains_users() {
    use yserver_core::{backend::Backend, core_loop::input_inventory::InputInventory};

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

    // Retry and drain the blocking disable before any wake. A failed
    // lifecycle detach never becomes resumable just because the caller asks
    // for DPMS On.
    let before_recovery_disable = fixture.recorders.atomic.records().len();
    Backend::finish_vt_release(
        &mut fixture.backend,
        &mut fixture.state,
        &InputInventory::default(),
    );
    let recovery_disable = fixture.recorders.atomic.records();
    assert_eq!(recovery_disable.len(), before_recovery_disable + 1);
    let recovered_state = &fixture.backend.platform.devices[0].cursor.crtc_states[&crtc];
    assert!(
        recovery_disable
            .last()
            .unwrap()
            .properties
            .iter()
            .all(|update| update.value == 0)
    );
    assert_eq!(recovered_state.applied, None);
    assert!(recovered_state.lifecycle_pending.is_none());
    assert!(
        !recovered_state.valid,
        "VT master loss invalidates comparison after the successful blocking detach"
    );
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
    assert_eq!(
        fixture.backend.pstate.vt_state,
        crate::vt::state::VtState::Suspended,
        "successful recovery disable completes before resume",
    );
    fixture.assert_cursor_quiescent();

    let before_recovery_wake = recovery_disable.len();
    Backend::on_vt_acquire(&mut fixture.backend, &mut fixture.state);
    let records = fixture.recorders.atomic.records();
    assert_eq!(records.len(), before_recovery_wake + 2);
    let rebound = fixture.backend.platform.devices[0].cursor.crtc_states[&crtc]
        .applied
        .expect("successful recovery wake rebinds cursor");
    assert_full_attach(
        &fixture,
        &records[before_recovery_wake + 1],
        rebound.framebuffer,
        initial.x,
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
        initial.x,
        initial.y,
    );
    retire_cursor_event(&mut fixture, 54);
    Backend::set_dpms_power(&mut fixture.backend, 3).expect("final all-off teardown");
    fixture.assert_cursor_quiescent();
    assert!(fixture.backend.platform.device_for_key(key).is_some());
    let resources = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(resources.end_state(), (0, 0, 0));
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: skip gamma restore after a first-output modeset; the re-enable must record the cached LUT again.
fn atomic_cursor_only_headless_reactivation_reapplies_cached_gamma() {
    use super::round_a::{
        ATOMIC_CURSOR_PROPERTIES, clone_drm_output, cursor_topology_snapshot, schedule_rescan_clock,
    };
    use std::time::Duration;
    use yserver_core::backend::Backend;

    let Some((mut fixture, key, output)) =
        AtomicCursorFixture::headless_for_lifecycle().expect("build headless fixture")
    else {
        return;
    };
    fixture
        .backend
        .platform
        .for_tests_record_complete_output_power_commits();
    let crtc = output.crtc;
    let factory = Rc::clone(&fixture.recorders.cursor_plane_factory);
    factory.queue_error(libc::EIO);
    factory.queue_snapshot(cursor_topology_snapshot(
        "amdgpu",
        false,
        &[crtc.into()],
        &[(10, vec![crtc.into()], ATOMIC_CURSOR_PROPERTIES.to_vec())],
    ));
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
        .expect("queue headless connector discovery edge");
    Backend::on_display_hotplug(&mut fixture.backend, &mut fixture.state);
    let deadline = fixture
        .backend
        .pstate
        .hotplug_rescan_deadline
        .expect("Backend schedules headless connector rescan");
    assert_eq!(deadline, base + Duration::from_millis(150));
    assert!(Backend::reprobe_connectors(&mut fixture.backend, &mut fixture.state).is_err());
    elapsed_ms.set(150);
    Backend::poll_deferred_input(&mut fixture.backend, &mut fixture.state);
    assert!(fixture.backend.platform.outputs.is_empty());

    let (outputs, _) = Backend::randr_outputs_and_modes(&mut fixture.backend);
    let randr_output = outputs
        .into_iter()
        .find(|candidate| candidate.name == key.connector_name)
        .expect("connected headless RANDR output");
    let output_id = randr_output.output_id;
    let crtc_id = randr_output.crtc_id;
    let mode = yserver_core::backend::ModeSpec {
        width: output.picked.width,
        height: output.picked.height,
        vrefresh: output.picked.vrefresh,
    };
    let red: Vec<_> = (0..256).map(|value| value as u16).collect();
    let green: Vec<_> = (0..256).map(|value| (value * 2) as u16).collect();
    let blue: Vec<_> = (0..256).map(|value| u16::MAX - value as u16).collect();
    let atomic_before = fixture.recorders.atomic.records().len();

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
        .expect("activate the first output from headless")
    );
    assert_eq!(fixture.backend.platform.outputs.len(), 1);
    assert_eq!(
        factory.calls(),
        1,
        "first cursor initialization is transiently failed"
    );
    Backend::set_crtc_gamma(&mut fixture.backend, crtc_id, &red, &green, &blue)
        .expect("set gamma through Backend while the output is active");
    assert_eq!(fixture.recorders.gamma.commits().len(), 2);
    assert_eq!(fixture.recorders.gamma.commits()[1].red, red);

    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            output_id,
            &key.connector_name,
            None,
            0,
            0,
        )
        .expect("disable the last output")
    );
    assert!(fixture.backend.platform.outputs.is_empty());

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
        .expect("reactivate the first output from zero outputs")
    );
    assert_eq!(fixture.backend.platform.outputs.len(), 1);
    assert_eq!(
        factory.calls(),
        2,
        "active topology retries cursor initialization"
    );
    let gamma_commits = fixture.recorders.gamma.commits();
    assert_eq!(
        gamma_commits.len(),
        3,
        "the cached gamma LUT is applied after reactivation's modeset",
    );
    assert_eq!(gamma_commits[2].crtc, u32::from(crtc));
    assert_eq!(gamma_commits[2].red, red);
    assert_eq!(gamma_commits[2].green, green);
    assert_eq!(gamma_commits[2].blue, blue);

    assert!(
        Backend::apply_crtc_config(
            &mut fixture.backend,
            output_id,
            &key.connector_name,
            None,
            0,
            0,
        )
        .expect("Backend teardown disables the reactivated output")
    );
    fixture.assert_cursor_quiescent();

    let records = fixture.recorders.atomic.records();
    let records = &records[atomic_before..];
    assert_eq!(records.len(), 4);
    assert!(
        records
            .iter()
            .all(|record| record.kind == crate::drm::atomic::AtomicCommitKind::Scene)
    );
    let mode_id = ::drm::control::from_u32(0xD102).expect("recorded MODE_ID property");
    let modesets: Vec<_> = records
        .iter()
        .filter(|record| {
            record
                .properties
                .iter()
                .any(|update| update.object == crtc.into() && update.property == mode_id)
        })
        .collect();
    assert_eq!(modesets.len(), 2, "one modeset per headless activation");
    let blocking_disables: Vec<_> = records
        .iter()
        .filter(|record| record.properties.iter().all(|update| update.value == 0))
        .collect();
    assert_eq!(
        blocking_disables.len(),
        2,
        "one disable at each zero-output boundary"
    );
    assert!(records.iter().all(|record| record.events.is_empty()));
    assert!(
        records
            .iter()
            .all(|record| record.result == crate::drm::atomic::tests_support::ScriptedResult::Ok)
    );

    let ledger = Rc::clone(&fixture.recorders.cursor_buffer_ledger);
    drop(fixture);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
