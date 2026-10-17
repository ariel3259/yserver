use std::{rc::Rc, sync::Arc};

use crate::kms::render::backend::KmsBackend;

pub(crate) struct AtomicCursorFixture {
    pub(crate) backend: KmsBackend,
    pub(crate) recorders: Rc<crate::kms::render::platform::services::KmsTestRecorders>,
    pub(crate) state: yserver_core::server::ServerState,
}

impl AtomicCursorFixture {
    pub(crate) fn new() -> Result<Option<Self>, String> {
        let backend = KmsBackend::for_tests_with_vk().map_err(|error| error.to_string())?;
        Self::from_live_backend(backend, Some(64), Some(64))
    }

    pub(crate) fn new_with_scanout_acquisition_script(
        unavailable: impl IntoIterator<Item = bool>,
    ) -> Result<Option<Self>, String> {
        let Some(mut fixture) = Self::new()? else {
            return Ok(None);
        };
        let pool = fixture.backend.platform.scanout_pools[0]
            .as_mut()
            .expect("Vulkan fixture owns a shared scanout pool")
            .display_pool_mut();
        pool.for_tests_script_unavailable_acquisitions(unavailable);
        Ok(Some(fixture))
    }

    pub(crate) fn new_with_cursor_lease_failure(
        boundary: crate::kms::cursor_plane::CursorLeaseBoundary,
    ) -> Result<Option<Self>, String> {
        Self::new_with_cursor_lease_failure_after(boundary, 0)
    }

    pub(crate) fn new_with_cursor_lease_failure_after(
        boundary: crate::kms::cursor_plane::CursorLeaseBoundary,
        matching_calls_to_skip: usize,
    ) -> Result<Option<Self>, String> {
        let Some(mut fixture) = Self::new()? else {
            return Ok(None);
        };
        fixture.script_cursor_lease_failure(boundary, matching_calls_to_skip);
        Ok(Some(fixture))
    }

    fn script_cursor_lease_failure(
        &mut self,
        boundary: crate::kms::cursor_plane::CursorLeaseBoundary,
        matching_calls_to_skip: usize,
    ) {
        self.backend.platform.devices[0]
            .cursor
            .plane
            .as_mut()
            .expect("fixture initializes an eligible cursor plane")
            .for_tests_script_lease_failure_after(boundary, matching_calls_to_skip);
    }

    pub(crate) fn new_with_hotspot_properties() -> Result<Option<Self>, String> {
        let backend = KmsBackend::for_tests_with_vk().map_err(|error| error.to_string())?;
        Self::from_live_backend_with_hotspot_properties(backend, Some(64), Some(64))
    }

    pub(crate) fn new_with_cursor_capabilities(
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) -> Result<Option<Self>, String> {
        let backend = KmsBackend::for_tests_with_vk().map_err(|error| error.to_string())?;
        Self::from_live_backend(backend, cursor_width, cursor_height)
    }

    pub(crate) fn new_with_two_outputs() -> Result<Option<Self>, String> {
        let mut backend = KmsBackend::for_tests_with_vk_dimensions(1600, 600)
            .map_err(|error| error.to_string())?;
        super::super::push_test_output(&mut backend, 2);
        Self::from_live_backend(backend, Some(64), Some(64))
    }

    pub(crate) fn new_with_two_outputs_and_cursor_lease_failure_after(
        boundary: crate::kms::cursor_plane::CursorLeaseBoundary,
        matching_calls_to_skip: usize,
    ) -> Result<Option<Self>, String> {
        let mut backend = KmsBackend::for_tests_with_vk_dimensions(1600, 600)
            .map_err(|error| error.to_string())?;
        super::super::push_test_output(&mut backend, 2);
        let Some(mut fixture) = Self::from_live_backend(backend, Some(64), Some(64))? else {
            return Ok(None);
        };
        fixture.script_cursor_lease_failure(boundary, matching_calls_to_skip);
        Ok(Some(fixture))
    }

    pub(crate) fn new_with_overlapping_outputs() -> Result<Option<Self>, String> {
        let mut backend = KmsBackend::for_tests_with_vk_dimensions(800, 600)
            .map_err(|error| error.to_string())?;
        super::super::push_test_output(&mut backend, 2);
        backend.platform.outputs[1].x = 0;
        Self::from_live_backend(backend, Some(64), Some(64))
    }

    pub(crate) fn new_with_colliding_device_handles() -> Result<Option<Self>, String> {
        let mut backend = KmsBackend::for_tests_with_vk_dimensions(800, 600)
            .map_err(|error| error.to_string())?;
        super::super::push_test_output(&mut backend, 1);
        backend.platform.outputs[1].x = 0;

        let second_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 1,
        };
        let recorders = Rc::clone(
            backend
                .platform
                .services
                .test_recorders
                .as_ref()
                .expect("for_tests selects recording KMS services"),
        );
        let dependency: Arc<dyn crate::drm::client_caps::DeviceControl + Send + Sync> =
            crate::drm::client_caps::recording_control("atomic-cursor-second-device");
        let device = Rc::new(
            crate::drm::Device::for_tests_with_control(dependency)
                .expect("open second recording DRM device"),
        );
        recorders.page_flip_events.register_device(&device);
        let atomic_commit_sink = Rc::clone(&backend.platform.devices[0].atomic_commit_sink);
        let page_flip_event_source = Rc::clone(&backend.platform.devices[0].page_flip_event_source);
        let legacy_cursor_io = Rc::clone(&backend.platform.devices[0].legacy_cursor_io);
        backend
            .platform
            .devices
            .push(crate::kms::render::platform::KmsDevice {
                key: second_key,
                device,
                async_page_flip: false,
                atomic_commit_sink,
                page_flip_event_source,
                cursor_buffer_io: Rc::new(
                    crate::kms::cursor_plane::ops::RecordingCursorBufferIo::with_shared_ledger(
                        Rc::clone(&recorders.cursor_buffer_ledger),
                        256,
                    ),
                ),
                legacy_cursor_io,
                test_page_flip_event_source: Some(Rc::clone(&recorders.page_flip_events)),
                cursor: crate::kms::render::platform::KmsCursorState::new(),
            });
        let route = backend
            .platform
            .scanout_route_for_kms(second_key)
            .expect("second device has a scanout route");
        backend.platform.outputs[1].key.device_key = second_key;
        backend.platform.outputs[1].scanout_route = route;

        Self::from_live_backend(backend, Some(64), Some(64))
    }

    pub(crate) fn new_with_gap_outputs() -> Result<Option<Self>, String> {
        let mut backend = KmsBackend::for_tests_with_vk_dimensions(1700, 600)
            .map_err(|error| error.to_string())?;
        super::super::push_test_output(&mut backend, 2);
        backend.platform.outputs[1].x = 900;
        Self::from_live_backend(backend, Some(64), Some(64))
    }

    fn from_live_backend(
        backend: KmsBackend,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) -> Result<Option<Self>, String> {
        Self::from_live_backend_with_cursor_configuration(
            backend,
            cursor_width,
            cursor_height,
            false,
        )
    }

    fn from_live_backend_with_hotspot_properties(
        backend: KmsBackend,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) -> Result<Option<Self>, String> {
        Self::from_live_backend_with_cursor_configuration(
            backend,
            cursor_width,
            cursor_height,
            true,
        )
    }

    fn from_live_backend_with_cursor_configuration(
        mut backend: KmsBackend,
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
        hotspot_properties: bool,
    ) -> Result<Option<Self>, String> {
        install_recording_device_caps(&mut backend);
        let vk = Arc::clone(
            backend
                .platform
                .gpu
                .vk
                .as_ref()
                .expect("Vk fixture owns its Vk context"),
        );
        if !vk.supports_sync_fd() {
            eprintln!(
                "skipping: Vulkan device lacks SYNC_FD semaphore export; cursor scene fixture needs it"
            );
            return Ok(None);
        }
        if hotspot_properties {
            backend
                .platform
                .for_tests_initialize_cursor_planes_with_hotspot_properties(
                    cursor_width,
                    cursor_height,
                );
        } else {
            backend
                .platform
                .for_tests_initialize_cursor_planes_with_capabilities(cursor_width, cursor_height);
        }
        install_recording_scanout_pools(&mut backend, vk)?;
        let layouts = backend.platform.scene_output_layouts();
        backend.scene =
            crate::kms::render::scene::SceneCompositor::new(&backend.platform, &layouts)
                .map_err(|error| format!("construct recording Vulkan scene: {error:?}"))?;
        let recorders = Rc::clone(
            backend
                .platform
                .services
                .test_recorders
                .as_ref()
                .expect("for_tests selects recording KMS services"),
        );
        Ok(Some(Self {
            backend,
            recorders,
            state: yserver_core::server::ServerState::new(),
        }))
    }

    pub(crate) fn without_scene() -> Result<Self, String> {
        Self::without_scene_with_cursor_capabilities(Some(64), Some(64))
    }

    pub(crate) fn without_scene_with_cursor_capabilities(
        cursor_width: Option<u64>,
        cursor_height: Option<u64>,
    ) -> Result<Self, String> {
        let mut backend = KmsBackend::for_tests();
        install_recording_device_caps(&mut backend);
        backend
            .platform
            .for_tests_initialize_cursor_planes_with_capabilities(cursor_width, cursor_height);
        let recorders = Rc::clone(
            backend
                .platform
                .services
                .test_recorders
                .as_ref()
                .expect("for_tests selects recording KMS services"),
        );
        Ok(Self {
            backend,
            recorders,
            state: yserver_core::server::ServerState::new(),
        })
    }

    pub(crate) fn headless_for_lifecycle() -> Result<
        Option<(
            Self,
            crate::kms::backend::OutputKey,
            crate::platform::drm::Output,
        )>,
        String,
    > {
        let mut backend = crate::kms::render::backend::KmsBackend::for_tests();
        let device_key = backend.platform.devices[0].key;
        let render_device_id = crate::kms::scanout_route::RenderDeviceId::DrmRender(device_key);
        install_recording_device_caps(&mut backend);
        let active = backend
            .platform
            .outputs
            .pop()
            .expect("test seed has one output to retain as an enable template");
        backend
            .platform
            .active_cursor_crtcs
            .remove(&crate::kms::render::platform::CrtcKey::for_output(&active));
        let key = active.key;
        let output = active.output;
        backend.platform.render_devices = vec![crate::kms::render::platform::RenderDevice {
            id: render_device_id,
            physical_device: ash::vk::PhysicalDevice::default(),
            selector: crate::kms::vk::device::VulkanDeviceSelector::for_tests(1),
            advertised_primary_node: Some(device_key),
            advertised_render_node: Some(device_key),
            render_node: None,
            render_node_device: None,
            syncobj_timeline: false,
        }];
        backend.platform.selected_render_device = Some(render_device_id);
        backend
            .initialize_provider_output_sources()
            .map_err(|error| format!("initialize lifecycle fixture provider source: {error}"))?;
        backend.platform.scanout_pools.clear();
        backend.platform.bo_generations.clear();
        backend.platform.first_pageflip_logged.clear();
        backend.pstate.randr_id_alloc = crate::kms::render::backend::RandrIdAllocator::default();
        backend.pstate.output_identity_by_id.clear();
        backend.pstate.output_key_by_id.clear();
        backend.pstate.crtc_key_by_id.clear();
        backend.kms_outputs_active = false;

        let recorders = Rc::clone(
            backend
                .platform
                .services
                .test_recorders
                .as_ref()
                .expect("for_tests selects recording KMS services"),
        );
        Ok(Some((
            Self {
                backend,
                recorders,
                state: yserver_core::server::ServerState::new(),
            },
            key,
            output,
        )))
    }

    pub(crate) fn define_visible_cursor(&mut self) -> u32 {
        use yserver_core::backend::Backend;

        let source = Backend::create_pixmap(&mut self.backend, None, 1, 1, 1)
            .expect("create cursor source pixmap");
        Backend::put_image(
            &mut self.backend,
            None,
            source.as_raw(),
            1,
            1,
            1,
            0,
            0,
            &[1, 0, 0, 0],
        )
        .expect("paint cursor source");
        let cursor = Backend::create_cursor(
            &mut self.backend,
            None,
            source,
            None,
            (u16::MAX, u16::MAX, u16::MAX),
            (0, 0, 0),
            0,
            0,
        )
        .expect("create cursor through Backend");
        let root = self.backend.core.window_id;
        Backend::define_cursor(&mut self.backend, None, root, cursor.as_raw())
            .expect("define cursor through Backend");
        cursor.as_raw()
    }

    pub(crate) fn retire_output(&mut self, output_idx: usize, frame: u32) {
        use std::os::fd::{AsFd as _, AsRawFd as _};
        use yserver_core::backend::Backend;

        let (crtc, device_key) = {
            let output = &self.backend.platform.outputs[output_idx];
            (output.output.crtc, output.key.device_key)
        };
        let device = Rc::clone(
            &self
                .backend
                .platform
                .device_for_key(device_key)
                .expect("fixture output has a KMS device")
                .device,
        );
        self.recorders.page_flip_events.register_device(&device);
        self.recorders
            .page_flip_events
            .queue_flip_for_device(&device, crtc, frame);
        let drm_fd = device.as_fd().as_raw_fd();
        Backend::on_page_flip_ready(&mut self.backend, &mut self.state, drm_fd);
    }

    pub(crate) fn assert_cursor_quiescent(&self) {
        assert!(
            !self.backend.scene.has_pending_page_flips(),
            "scene page-flip obligations remain",
        );
        assert!(self.backend.pstate.scanout_m2.pending.is_none());
        assert!(self.backend.pstate.scanout_m2.current.is_none());
        assert!(self.backend.pstate.scanout_m2.queued_successor.is_none());
        assert!(
            self.backend
                .pstate
                .scanout_m2
                .deferred_successor_skips
                .is_empty()
        );
        assert!(
            self.backend
                .pstate
                .scanout_m2
                .unflip_awaiting_outputs
                .is_empty()
        );
        assert!(self.backend.pstate.scanout_m2.completed.is_empty());
        assert!(self.backend.pstate.scanout_m2.idled.is_empty());
        assert!(
            self.backend.platform.async_ledger.for_tests_is_empty(),
            "async flip ledger retains an output obligation",
        );
        assert_eq!(
            self.recorders.page_flip_events.queued_flip_count(),
            0,
            "recording page-flip events remain undrained",
        );
        for output in &self.backend.platform.outputs {
            let device = self
                .backend
                .platform
                .device_for_key(output.key.device_key)
                .expect("fixture output has a KMS device");
            let state = device.cursor.crtc_states.get(&output.output.crtc);
            assert!(
                state.is_none_or(|state| {
                    state.applied.is_none()
                        && state.lifecycle_pending.is_none()
                        && state.intent.is_none()
                        && state.in_flight.is_none()
                }),
                "cursor binding, lifecycle owner, intent, or event obligation remains for {:?}",
                output.output.crtc,
            );
            assert!(
                device
                    .cursor
                    .plane
                    .as_ref()
                    .is_none_or(|plane| plane.sprite_pool_users_are_zero()),
                "cursor sprite pool retains a framebuffer user for {:?}",
                output.output.crtc,
            );
            assert!(
                !self
                    .backend
                    .platform
                    .for_tests_atomic_cursor_commit_is_in_flight(
                        output.key.device_key,
                        output.output.crtc,
                    ),
                "cursor commit remains in flight for {:?}",
                output.output.crtc,
            );
            assert!(
                self.backend
                    .platform
                    .for_tests_atomic_cursor_pending_position(
                        output.key.device_key,
                        output.output.crtc,
                    )
                    .is_none(),
                "pending cursor intent remains for {:?}",
                output.output.crtc,
            );
        }
    }

    pub(crate) fn install_copied_recording_route(
        &mut self,
        route: crate::kms::scanout_route::ScanoutRoute,
    ) -> Result<(), String> {
        use crate::kms::vk::scanout::OutputScanout;

        let vk = Arc::clone(
            self.backend
                .platform
                .gpu
                .vk
                .as_ref()
                .expect("Vulkan cursor fixture owns its Vk context"),
        );
        let Some(Some(OutputScanout::Shared(mut destinations))) = self
            .backend
            .platform
            .scanout_pools
            .get_mut(0)
            .map(Option::take)
        else {
            return Err("copied recording route requires the shared test destination pool".into());
        };
        destinations.route = route;
        let copied =
            crate::kms::vk::scanout::CopiedScanoutPool::for_tests_recording_pool(vk, destinations)
                .map_err(|error| format!("allocate copied recording route: {error}"))?;
        self.backend.platform.scanout_pools[0] = Some(OutputScanout::Copied(copied));
        Ok(())
    }
}

fn install_recording_device_caps(backend: &mut KmsBackend) {
    use crate::drm::client_caps::{DeviceControl, recording_control};

    let control = recording_control("atomic-cursor-fixture");
    let dependency: Arc<dyn DeviceControl + Send + Sync> = control;
    let device = Rc::new(
        crate::drm::Device::for_tests_with_control(dependency)
            .expect("record capabilities on synthetic DRM device"),
    );
    backend
        .platform
        .services
        .test_recorders
        .as_ref()
        .expect("for_tests selects recording KMS services")
        .page_flip_events
        .register_device(&device);
    backend.platform.devices[0].device = device;
}

fn install_recording_scanout_pools(
    backend: &mut KmsBackend,
    vk: Arc<crate::kms::vk::device::VkContext>,
) -> Result<(), String> {
    use crate::kms::{
        render::store::DrawableKind,
        vk::scanout::{OutputScanout, ScanoutBo, ScanoutBoPool},
    };

    backend.platform.scanout_pools.clear();
    backend.platform.bo_generations.clear();
    for output_idx in 0..backend.platform.outputs.len() {
        let (width, height, device_key, route) = {
            let layout = &backend.platform.outputs[output_idx];
            (
                u32::from(layout.width),
                u32::from(layout.height),
                layout.key.device_key,
                layout.scanout_route,
            )
        };
        let output = &mut backend.platform.outputs[output_idx].output;
        output.plane_in_fence_fd_prop =
            Some(::drm::control::from_u32(81).expect("test IN_FENCE_FD property handle"));
        output.crtc_out_fence_ptr_prop =
            Some(::drm::control::from_u32(82).expect("test OUT_FENCE_PTR property handle"));
        let device = backend
            .platform
            .device_for_key(device_key)
            .expect("test output has a KMS device");
        let mut bos = Vec::with_capacity(2);
        for bo_idx in 0_usize..2 {
            let xid = 0x00f0_0000
                + u32::try_from(output_idx).map_err(|error| error.to_string())? * 0x10
                + u32::try_from(bo_idx).map_err(|error| error.to_string())?;
            let storage = backend
                .platform
                .gpu
                .allocate_drawable_storage(
                    u16::try_from(width).map_err(|error| error.to_string())?,
                    u16::try_from(height).map_err(|error| error.to_string())?,
                    32,
                )
                .map_err(|error| format!("allocate recording scene target: {error:?}"))?;
            let image = storage.image;
            let image_view = storage.image_view;
            backend
                .store
                .allocate(xid, DrawableKind::Pixmap, 32, false, storage)
                .map_err(|error| format!("register recording scene target: {error:?}"))?;
            let fb = 0x00d0_0000
                + u32::try_from(output_idx).map_err(|error| error.to_string())? * 0x10
                + u32::try_from(bo_idx).map_err(|error| error.to_string())?;
            let mut bo = ScanoutBo::for_tests_compose_target(
                Arc::clone(&vk),
                Rc::clone(&device.device),
                ::drm::control::from_u32(fb).expect("synthetic test FB handle"),
                width,
                height,
                image,
                image_view,
            )
            .map_err(|error| format!("create recording scene BO: {error}"))?;
            if bo_idx == 0 {
                bo.state.phase = crate::kms::vk::scanout::BoPhase::OnScreen;
            }
            bos.push(bo);
        }
        let pool = ScanoutBoPool::for_tests_recording_pool(route, width, height, bos);
        backend
            .platform
            .scanout_pools
            .push(Some(OutputScanout::Shared(pool)));
        backend.platform.bo_generations.push(vec![
                crate::kms::render::platform::BoGenerationEntry::default();
                2
            ]);
    }
    Ok(())
}
