use std::{rc::Rc, sync::Arc};

use crate::kms::render::backend::KmsBackend;

pub(crate) struct AtomicCursorFixture {
    pub(crate) backend: KmsBackend,
    pub(crate) recorders: Rc<crate::kms::render::platform::services::KmsTestRecorders>,
    pub(crate) state: yserver_core::server::ServerState,
}

impl AtomicCursorFixture {
    pub(crate) fn new() -> Result<Option<Self>, String> {
        let mut backend = KmsBackend::for_tests_with_vk().map_err(|error| error.to_string())?;
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
        backend.platform.for_tests_initialize_cursor_planes();
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
        let mut backend = KmsBackend::for_tests();
        backend.platform.for_tests_initialize_cursor_planes();
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

        let output = &self.backend.platform.outputs[output_idx].output;
        self.recorders
            .page_flip_events
            .queue_flip(output.crtc, frame);
        let drm_fd = self.backend.platform.devices[0].device.as_fd().as_raw_fd();
        Backend::on_page_flip_ready(&mut self.backend, &mut self.state, drm_fd);
    }

    pub(crate) fn assert_cursor_quiescent(&self) {
        for output in &self.backend.platform.outputs {
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
