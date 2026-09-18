use std::{
    cell::RefCell,
    os::fd::{AsFd, AsRawFd, BorrowedFd},
    rc::Rc,
    time::{Duration, Instant},
};

use yserver_core::{backend::Backend, server::ServerState};

use super::KmsBackend;
use crate::kms::{
    owner::identity::IncarnationId,
    render::{
        backend::LiveKmsFixture,
        resources::{
            AllocationKey, DrmCleanupRegistry, ResourceService, UseKind, tests::MockCleanupIo,
        },
    },
    vk::scanout::BoPhase,
};

struct ManagedScanoutFlip {
    fixture: LiveKmsFixture,
    // The registry owns the cleanup interface used when the managed
    // allocation is discharged. Keep it alive for the whole live fixture.
    _registry: DrmCleanupRegistry,
    pool_idx: usize,
    bo_idx: usize,
    output: ::drm::control::crtc::Handle,
    device: Rc<crate::drm::Device>,
    drm_fd: std::os::fd::RawFd,
    source_key: AllocationKey,
    managed_framebuffer: u32,
}

fn prepare_managed_scanout_flip() -> ManagedScanoutFlip {
    let mut fixture = KmsBackend::for_tests_with_live_kms()
        .expect("live-KMS fixture requires a usable primary DRM device and Vulkan ICD");
    let backend = &mut fixture.backend;
    let pool_idx = 0;
    let bo_idx = 1;
    let output = backend.platform.outputs[pool_idx].output.crtc;
    let device_key = backend.platform.outputs[pool_idx].key.device_key;
    let device = backend
        .platform
        .device_for_key(device_key)
        .expect("live output has a KMS device")
        .device
        .clone();
    let drm_fd = device.as_fd().as_raw_fd();

    let incarnation = IncarnationId::first();
    let mut service = ResourceService::new(device_key, incarnation);
    let cleanup_calls = Rc::new(RefCell::new(Vec::new()));
    let mut registry = DrmCleanupRegistry::new_with_device_and_io(
        Rc::clone(&device),
        device_key,
        incarnation,
        Box::new(MockCleanupIo::new(Rc::clone(&cleanup_calls))),
    );

    // This is the same managed conversion and pool-lease rooting used by
    // c0_2ci_scene_managed_shared_compose_vulkan. The returned key is rooted
    // by the pool BO's managed lease, not by the local read lease below.
    let source_key = backend
        .platform
        .register_managed_scanout_bo(&mut service, &mut registry, pool_idx, bo_idx)
        .expect("register the free scanout BO as managed");

    // Read the framebuffer handle from the managed allocation itself. The
    // converted pool BO is a husk and must not be used as the source of truth.
    let read_lease = service
        .reserve(source_key, UseKind::Read)
        .expect("reserve the managed scanout allocation for readback");
    let managed_framebuffer = service
        .with_scanout_read(&read_lease, |allocation| {
            allocation
                .file_owned()
                .and_then(|file_owned| file_owned.fb_handle())
                .map(u32::from)
        })
        .expect("read the managed scanout allocation")
        .expect("managed scanout allocation has a framebuffer");
    drop(read_lease);

    backend.resource_service = Some(service);

    ManagedScanoutFlip {
        fixture,
        _registry: registry,
        pool_idx,
        bo_idx,
        output,
        device,
        drm_fd,
        source_key,
        managed_framebuffer,
    }
}

impl ManagedScanoutFlip {
    fn submit(&mut self) {
        self.fixture.backend.tick_maybe_composite_for_tests();
    }

    fn consume_matching_page_flip(&mut self) {
        // Wait only for the matching page-flip completion, with a hard
        // deadline. The production event hook drains the DRM fd and advances
        // this exact BO Pending -> OnScreen; no timing-only sleep or fixture
        // bookkeeping is used.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = ServerState::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for the matching page-flip completion"
            );
            let timeout_ms = i32::try_from(remaining.as_millis().min(i32::MAX as u128))
                .expect("bounded poll timeout fits in i32")
                .max(1);
            let mut poll_fd = libc::pollfd {
                fd: self.drm_fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `poll_fd` is a valid stack value and `drm_fd` is owned
            // by the live fixture for the duration of this test.
            let poll_result = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
            if poll_result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                panic!("polling the live DRM fd for page-flip completion failed: {error}");
            }
            assert_ne!(
                poll_result, 0,
                "timed out waiting for the matching page-flip completion"
            );

            Backend::on_page_flip_ready(&mut self.fixture.backend, &mut state, self.drm_fd);
            let phase = self.fixture.backend.platform.scanout_pools[self.pool_idx]
                .as_ref()
                .expect("live output has a scanout pool")
                .display_pool()
                .bos[self.bo_idx]
                .state
                .phase;
            if phase == BoPhase::OnScreen {
                break;
            }
        }
    }
}

#[test]
#[ignore = "needs live DRM master and Vulkan ICD"]
fn c0_2ci_managed_scanout_flip_accepted_drm() {
    use ::drm::control::Device as DrmControlDevice;

    let mut flip = prepare_managed_scanout_flip();
    let pool_idx = flip.pool_idx;
    let bo_idx = flip.bo_idx;
    let output = flip.output;
    let source_key = flip.source_key;
    let managed_framebuffer = flip.managed_framebuffer;

    let framebuffer_before_tick;
    {
        let backend = &mut flip.fixture.backend;
        assert_eq!(
            backend.platform.scanout_pools[pool_idx]
                .as_ref()
                .expect("live output has a scanout pool")
                .display_pool()
                .bos[bo_idx]
                .managed_key(),
            Some(source_key),
            "managed scanout conversion must root the allocation by the BO lease"
        );

        let front_framebuffer = backend.platform.scanout_pools[pool_idx]
            .as_ref()
            .expect("live output has a scanout pool")
            .display_pool()
            .bos[0]
            .fb_handle
            .map(u32::from)
            .expect("initial front BO has a framebuffer");
        framebuffer_before_tick = flip
            .device
            .get_crtc(output)
            .expect("read the live CRTC before the page-flip tick")
            .framebuffer()
            .map(u32::from);
        assert_eq!(
            framebuffer_before_tick,
            Some(front_framebuffer),
            "initial modeset must leave the first pool BO scanning out before the tick"
        );
        assert_ne!(
            front_framebuffer, managed_framebuffer,
            "the managed BO must have a different framebuffer from the initial front BO"
        );
    }

    // This is the real scene submission path. It reaches
    // SceneCompositor::submit_shared_scanout_frame, whose accepted branch
    // calls crate::drm::page_flip::submit_flip_with_fences and only then
    // transitions this BO to Pending.
    flip.submit();

    {
        let backend = &mut flip.fixture.backend;
        let pending_indices: Vec<_> = backend.platform.scanout_pools[pool_idx]
            .as_ref()
            .expect("live output has a scanout pool")
            .display_pool()
            .bos
            .iter()
            .enumerate()
            .filter_map(|(index, bo)| (bo.state.phase == BoPhase::Pending).then_some(index))
            .collect();
        assert_eq!(
            pending_indices,
            vec![bo_idx],
            "an accepted kernel flip must leave exactly the managed BO Pending; this state is reached only after submit_flip_with_fences returned Ok"
        );
        assert_eq!(
            backend.platform.scanout_pools[pool_idx]
                .as_ref()
                .expect("live output has a scanout pool")
                .display_pool()
                .bos[bo_idx]
                .managed_key(),
            Some(source_key)
        );
        // The release fence is owned by this BO's state after acceptance. The
        // test deliberately never closes or otherwise takes it.
        assert!(
            backend.platform.scanout_pools[pool_idx]
                .as_ref()
                .expect("live output has a scanout pool")
                .display_pool()
                .bos[bo_idx]
                .state
                .release_fence_fd
                .is_some(),
            "accepted live flip must retain its out-fence in BO state"
        );
    }

    flip.consume_matching_page_flip();

    // Only after the matching completion was consumed do we ask the device
    // which framebuffer the CRTC currently scans out.
    let current_framebuffer = flip
        .device
        .get_crtc(output)
        .expect("read the live CRTC after matching page-flip completion")
        .framebuffer()
        .map(u32::from);
    assert_eq!(
        current_framebuffer,
        Some(managed_framebuffer),
        "kernel CRTC readback must identify the submitted managed framebuffer"
    );
    assert_ne!(
        framebuffer_before_tick, current_framebuffer,
        "the matching completion must change the framebuffer reported by the CRTC"
    );

    let backend = &mut flip.fixture.backend;
    let mut service = backend
        .resource_service
        .take()
        .expect("resource service was preserved through the live submission");
    assert_eq!(
        service.pending_batches().len(),
        1,
        "matching completion must register the managed GPU retirement batch"
    );
    let ticket = service.pending_batches()[0]
        .obligation
        .as_ref()
        .expect("managed retirement batch carries a GPU obligation")
        .ticket()
        .clone();
    let vk_context = backend
        .platform
        .vk
        .clone()
        .expect("live scene has a Vulkan context");
    ticket
        .wait(&vk_context)
        .expect("wait for compose completion");
    service
        .poll_gpu(Instant::now())
        .expect("poll the completed managed GPU retirement batch");
    assert!(service.pending_batches().is_empty());
}

#[test]
#[ignore = "needs live DRM master and Vulkan ICD"]
fn c0_2ci_managed_scanout_out_fence_resolves_drm() {
    let mut flip = prepare_managed_scanout_flip();
    flip.submit();
    {
        let backend = &flip.fixture.backend;
        let bo = &backend.platform.scanout_pools[flip.pool_idx]
            .as_ref()
            .expect("live output has a scanout pool")
            .display_pool()
            .bos[flip.bo_idx];
        assert_eq!(
            bo.managed_key(),
            Some(flip.source_key),
            "the out-fence must belong to the managed scanout BO"
        );
        assert_eq!(
            bo.state.phase,
            BoPhase::Pending,
            "an accepted kernel flip must reach Pending before its matching completion"
        );
    }
    flip.consume_matching_page_flip();

    let backend = &flip.fixture.backend;
    let bo = &backend.platform.scanout_pools[flip.pool_idx]
        .as_ref()
        .expect("live output has a scanout pool")
        .display_pool()
        .bos[flip.bo_idx];
    assert_eq!(
        bo.state.phase,
        BoPhase::OnScreen,
        "matching completion must leave the submitted BO on screen"
    );
    let release_fence_fd = bo
        .state
        .release_fence_fd
        .expect("matching completion must leave the BO's release fence owned by its state");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for managed scanout out-fence to reach Success"
        );
        let status = {
            // SAFETY: the BO state owns this descriptor for the whole
            // observation. BorrowedFd does not take or close that ownership.
            let borrowed_fd = unsafe { BorrowedFd::borrow_raw(release_fence_fd) };
            crate::platform::sync_file::query_status(borrowed_fd)
        }
        .unwrap_or_else(|error| panic!("failed to query managed scanout out-fence: {error}"));

        match status {
            crate::platform::sync_file::FenceStatus::Success => break,
            crate::platform::sync_file::FenceStatus::Error(error) => {
                panic!("managed scanout out-fence reported Error({error})")
            }
            crate::platform::sync_file::FenceStatus::Pending => {
                std::thread::yield_now();
            }
        }
    }

    assert_eq!(
        flip.fixture.backend.platform.scanout_pools[flip.pool_idx]
            .as_ref()
            .expect("live output has a scanout pool")
            .display_pool()
            .bos[flip.bo_idx]
            .state
            .release_fence_fd,
        Some(release_fence_fd),
        "successful fence observation must not take the BO-owned descriptor"
    );
}
