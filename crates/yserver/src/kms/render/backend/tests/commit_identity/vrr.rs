use crate::{
    drm::atomic::{AtomicCommitSink, tests_support::RecordingAtomicCommitExecutor},
    kms::{
        render::{
            backend::KmsBackend,
            platform::{ConnectorSnapshot, KmsDevice, PlatformBackend, RenderDeviceId},
        },
        scanout_route::{RenderKmsRelationship, ScanoutRoute},
    },
};
use std::{cell::RefCell, io, rc::Rc};
use yserver_core::backend::{Backend, ModeSpec};

fn handle<H: ::drm::control::ResourceHandle>(value: u32) -> H {
    ::drm::control::from_u32(value).expect("nonzero VRR test handle")
}

fn vrr_backend_with_recorder() -> (KmsBackend, Rc<RecordingAtomicCommitExecutor>) {
    let mut backend = KmsBackend::for_tests();
    let display_key = backend.platform.devices[0].key;
    let render_id = RenderDeviceId::DrmRender(super::super::test_device_key(128));
    backend.platform.render_devices = vec![super::super::randr::test_render_device(
        render_id,
        Some(display_key),
    )];
    backend.platform.selected_render_device = Some(render_id);
    backend.platform.outputs[0].scanout_route =
        ScanoutRoute::new(render_id, display_key, RenderKmsRelationship::Same);
    let recorder = Rc::new(RecordingAtomicCommitExecutor::default());
    backend.platform.devices[0].atomic_commit_sink =
        Rc::new(AtomicCommitSink::new(recorder.clone()));
    let output = &mut backend.platform.outputs[0].output;
    output.crtc_vrr_enabled_prop = Some(handle(75));
    output.connector_vrr_capable = true;
    output.vrr_committed.set(false);
    output.vrr_disabled.set(false);
    backend.scene.scene_structure_dirty = false;
    backend
        .initialize_provider_output_sources()
        .expect("authorize the test KMS output");
    backend.platform.for_tests_accept_output_power_commits();
    (backend, recorder)
}

fn randr_output_id(backend: &mut KmsBackend, connector: &str) -> u32 {
    Backend::randr_outputs_and_modes(backend)
        .0
        .into_iter()
        .find(|output| output.name == connector)
        .expect("test output is in the RandR projection")
        .output_id
}

fn apply_modeset_replacement(backend: &mut KmsBackend, mode_vrefresh: u32, crtc: u32) {
    let (connector, output_key, x, y) = {
        let layout = &backend.platform.outputs[0];
        (
            layout.key.connector_name.clone(),
            layout.key.clone(),
            layout.x,
            layout.y,
        )
    };
    let output_id = randr_output_id(backend, &connector);
    let mode = ModeSpec {
        width: 800,
        height: 600,
        vrefresh: mode_vrefresh,
    };
    let mut replacement =
        super::super::randr::clone_test_drm_output(&backend.platform.outputs[0].output);
    replacement.crtc = handle(crtc);
    let advertised = super::super::test_advertised_mode(800, 600, mode_vrefresh, false);
    replacement.picked = advertised.clone();
    replacement.modes.push(advertised.clone());
    backend
        .pstate
        .randr_id_alloc
        .entry_mut(&output_key)
        .modes
        .push(advertised);
    backend.pstate.crtc_config_discovery_override = Some(replacement);
    backend
        .initialize_provider_output_sources()
        .expect("authorize the test KMS output");
    assert!(
        Backend::apply_crtc_config(backend, output_id, &connector, Some(mode), x, y,)
            .expect("apply RandR modeset through Backend")
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: drop the same-CRTC VRR state carry-over in connector replacement.
fn vrr_state_survives_output_replacement_on_same_crtc() {
    let Some(fixture) = super::recording_scene::RecordingSceneFixture::new(1)
        .expect("recording scene fixture construction")
    else {
        eprintln!("skip: no Vulkan scene fixture available");
        return;
    };
    let mut fixture = fixture.cleanup_guard();
    let backend = &mut fixture.events.backend;
    let output = &mut backend.platform.outputs[0].output;
    output.crtc_vrr_enabled_prop = Some(handle(75));
    output.connector_vrr_capable = true;
    backend.platform.for_tests_accept_output_power_commits();
    let crtc = u32::from(backend.platform.outputs[0].output.crtc);
    backend.platform.outputs[0].output.vrr_committed.set(true);
    backend.platform.outputs[0].output.vrr_disabled.set(true);

    apply_modeset_replacement(backend, 59, crtc);

    let output = &backend.platform.outputs[0].output;
    assert_eq!(
        u32::from(output.crtc),
        crtc,
        "the replacement keeps its CRTC"
    );
    assert!(
        output.vrr_committed.get(),
        "the kernel-held value carries over"
    );
    assert!(output.vrr_disabled.get(), "the EINVAL latch carries over");

    output.vrr_committed.set(true);
    output.vrr_disabled.set(true);
    apply_modeset_replacement(backend, 58, crtc + 1);
    let output = &backend.platform.outputs[0].output;
    assert_eq!(
        u32::from(output.crtc),
        crtc + 1,
        "the replacement uses a new CRTC"
    );
    assert!(
        !output.vrr_committed.get(),
        "a new CRTC starts with kernel state 0"
    );
    assert!(
        !output.vrr_disabled.get(),
        "a new CRTC starts without an EINVAL latch"
    );
    fixture.finish_after_normal_drain();
}

struct VrrSnapshot(RefCell<Vec<ConnectorSnapshot>>);

impl crate::kms::render::backend::kms::session::VtResumeExecutor for VrrSnapshot {
    fn acquire_master(&self, _device: &KmsDevice) -> io::Result<()> {
        Ok(())
    }

    fn release_master(&self, _device: &KmsDevice) -> io::Result<()> {
        Ok(())
    }

    fn probe_connector_snapshot(
        &self,
        _platform: &PlatformBackend,
    ) -> io::Result<Vec<ConnectorSnapshot>> {
        Ok(self.0.borrow().clone())
    }
}

#[test]
// Mutation: skip refreshing connector_vrr_capable from the rescan snapshot.
fn vrr_capable_follows_connector_rescan() {
    let (mut backend, _recorder) = vrr_backend_with_recorder();
    let live = &backend.platform.outputs[0];
    let snapshot = ConnectorSnapshot {
        key: live.key.clone(),
        modes: vec![live.output.picked.clone()],
        mm_width: live.output.mm_width,
        mm_height: live.output.mm_height,
        edid: live.output.edid.clone(),
        connector_type: live.output.connector_type.clone(),
        vrr_capable: false,
    };
    backend.for_tests_set_vt_resume_executor(Rc::new(VrrSnapshot(RefCell::new(vec![snapshot]))));
    backend.pstate.hotplug_rescan_deadline = Some(std::time::Instant::now());
    let mut state = yserver_core::server::ServerState::new();

    backend.backend_session_poll_deferred_input(&mut state);

    let output = &backend.platform.outputs[0].output;
    assert!(
        !output.connector_vrr_capable,
        "the live output adopts snapshot value 0"
    );
    assert!(
        !output.vrr_capable(),
        "VRR is unavailable after the monitor replacement"
    );
}
