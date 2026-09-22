#[allow(unused_imports)]
use std::{fmt, io, num::NonZeroU32, os::fd::OwnedFd, rc::Rc, sync::Arc};

use ash::vk;
use drm::{buffer::Handle as DrmBufferHandle, control::framebuffer};

use super::{
    AllocationLease, ResourceError,
    drm_cleanup::{DrmCleanupRegistry, DrmCleanupRight, GemOwner},
};
use crate::kms::{
    render::platform::CrtcKey,
    vk::{
        device::VkContext,
        scanout::{
            CopiedRenderSourceBacking, CopiedSourceOwnership, ExportSemaphoreReuseState,
            RetainedSyncFile, ScanoutBoBacking, TransferResources, destroy_transfer_resources,
        },
        target::{DrawableImage, ExportableImage},
    },
};

/// File-owned half of a managed scanout allocation. Owned exclusively by
/// the DRM open file description and discharged by the Task-2 right or Task-9 barrier.
#[allow(dead_code)]
pub(crate) struct FileOwnedBacking {
    right: DrmCleanupRight,
    gbm_bo: Option<gbm::BufferObject<()>>,
    device: Rc<crate::drm::Device>,
}

impl fmt::Debug for FileOwnedBacking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileOwnedBacking")
            .field("right", &self.right)
            .field("has_gbm_bo", &self.gbm_bo.is_some())
            .finish()
    }
}

#[allow(dead_code)]
impl FileOwnedBacking {
    /// The only constructor. `GemOwner::Gbm` requires `Some(gbm_bo)` and
    /// `GemOwner::Right` requires `None`; any other pairing is rejected here,
    /// so two closers of one handle cannot be assembled by mistake.
    pub(crate) fn new(
        right: DrmCleanupRight,
        gbm_bo: Option<gbm::BufferObject<()>>,
        device: Rc<crate::drm::Device>,
    ) -> Result<Self, ResourceError> {
        match (right.gem_owner(), &gbm_bo) {
            (GemOwner::Gbm, Some(_)) => Ok(Self {
                right,
                gbm_bo,
                device,
            }),
            (GemOwner::Right, None) => Ok(Self {
                right,
                gbm_bo,
                device,
            }),
            _ => Err(ResourceError::InvalidState),
        }
    }

    pub(crate) fn right(&self) -> &DrmCleanupRight {
        &self.right
    }

    pub(crate) fn right_mut(&mut self) -> &mut DrmCleanupRight {
        &mut self.right
    }

    pub(crate) fn gbm_bo(&self) -> Option<&gbm::BufferObject<()>> {
        self.gbm_bo.as_ref()
    }

    pub(crate) fn device(&self) -> &Rc<crate::drm::Device> {
        &self.device
    }

    pub(crate) fn fb_handle(&self) -> Option<framebuffer::Handle> {
        NonZeroU32::new(self.right.fb()).map(framebuffer::Handle::from)
    }

    pub(crate) fn discharge(
        self,
        registry: &mut DrmCleanupRegistry,
    ) -> Result<(), (io::Error, Self)> {
        let Self {
            right,
            gbm_bo,
            device,
        } = self;
        match registry.consume(right) {
            Ok(()) => {
                // Table order (M-22): the GEM handle closes (gbm_bo's own
                // drop, when GemOwner::Gbm) before the Rc<drm::Device> alias
                // -- a bare `let Self { .. } = self;` destructure drops
                // fields in reverse binding order, which is device-before-
                // gbm_bo and exactly backwards.
                drop(gbm_bo);
                drop(device);
                Ok(())
            }
            Err((err, returned_right)) => Err((
                err,
                Self {
                    right: returned_right,
                    gbm_bo,
                    device,
                },
            )),
        }
    }

    pub(crate) fn close_after_family(mut self) {
        self.right.mark_closed();
        let Self {
            right: _,
            gbm_bo,
            device,
        } = self;
        drop(gbm_bo);
        drop(device);
    }
}

/// Shared half of a managed scanout allocation. Independent of the DRM description;
/// released only by GPU/read/FOREIGN proofs.
pub(crate) struct SharedBacking {
    pub(crate) image: vk::Image,
    pub(crate) memory: vk::DeviceMemory,
    pub(crate) view: vk::ImageView,
    pub(crate) transfer: TransferResources,
    pub(crate) vk: Option<Arc<VkContext>>,
    pub(crate) dmabuf: Option<OwnedFd>,
}

impl fmt::Debug for SharedBacking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedBacking")
            .field("image", &self.image)
            .field("memory", &self.memory)
            .field("view", &self.view)
            .field("has_vk", &self.vk.is_some())
            .field("has_dmabuf", &self.dmabuf.is_some())
            .finish()
    }
}

#[allow(dead_code)]
impl SharedBacking {
    pub(crate) fn new(
        image: vk::Image,
        memory: vk::DeviceMemory,
        view: vk::ImageView,
        transfer: TransferResources,
        vk: Arc<VkContext>,
        dmabuf: Option<OwnedFd>,
    ) -> Self {
        Self {
            image,
            memory,
            view,
            transfer,
            vk: Some(vk),
            dmabuf,
        }
    }

    /// Test-only: a `SharedBacking` with no Vulkan context. Never reachable
    /// from production (M-23) -- there is no `VkContext` test fixture
    /// (`VkContext::new()` requires a live Vulkan ICD), so a deterministic
    /// unit test cannot supply a real one; every real allocation goes
    /// through `new` and always carries `Some`.
    #[cfg(test)]
    pub(crate) fn mock(
        image: vk::Image,
        memory: vk::DeviceMemory,
        view: vk::ImageView,
        transfer: TransferResources,
        dmabuf: Option<OwnedFd>,
    ) -> Self {
        Self {
            image,
            memory,
            view,
            transfer,
            vk: None,
            dmabuf,
        }
    }
}

impl Drop for SharedBacking {
    fn drop(&mut self) {
        if let Some(vk) = &self.vk {
            unsafe {
                let t = std::mem::replace(&mut self.transfer, TransferResources::empty());
                if t.command_pool != vk::CommandPool::null() {
                    vk.device.unmap_memory(t.staging_memory);
                    vk.device.destroy_buffer(t.staging_buffer, None);
                    vk.device.free_memory(t.staging_memory, None);
                    vk.device.destroy_command_pool(t.command_pool, None);
                    if t.timestamp_pool != vk::QueryPool::null() {
                        vk.device.destroy_query_pool(t.timestamp_pool, None);
                    }
                }
                if self.view != vk::ImageView::null() {
                    vk.device.destroy_image_view(self.view, None);
                }
                if self.image != vk::Image::null() {
                    vk.device.destroy_image(self.image, None);
                }
                if self.memory != vk::DeviceMemory::null() {
                    vk.device.free_memory(self.memory, None);
                }
            }
        }
    }
}

/// Physical ownership of one managed scanout allocation: file-owned DRM half
/// plus shared Vulkan/read half.
#[allow(dead_code)]
pub(crate) struct ScanoutAllocation {
    pub(crate) file_owned: Option<FileOwnedBacking>,
    pub(crate) shared: SharedBacking,
}

impl fmt::Debug for ScanoutAllocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScanoutAllocation")
            .field("file_owned", &self.file_owned)
            .field("shared", &self.shared)
            .finish()
    }
}

#[allow(dead_code)]
impl ScanoutAllocation {
    pub(crate) fn new(file_owned: Option<FileOwnedBacking>, shared: SharedBacking) -> Self {
        Self { file_owned, shared }
    }

    pub(crate) fn file_owned(&self) -> Option<&FileOwnedBacking> {
        self.file_owned.as_ref()
    }

    pub(crate) fn file_owned_mut(&mut self) -> Option<&mut FileOwnedBacking> {
        self.file_owned.as_mut()
    }

    pub(crate) fn shared(&self) -> &SharedBacking {
        &self.shared
    }

    pub(crate) fn shared_mut(&mut self) -> &mut SharedBacking {
        &mut self.shared
    }

    /// F2-B3: on failure, keeps the backing in `self.file_owned` (matching
    /// `DirectFramebufferAllocation::discharge_file_owned`) rather than
    /// reinstalling it and immediately taking it right back out into the
    /// `Err` -- which left `self.file_owned` `None` after a *failed*
    /// discharge, so the caller's retry found nothing to retry and the
    /// entry's next tick destroyed it with the right (at
    /// `FramebufferRemoved`), the gbm_bo and the device alias silently
    /// dropped.
    pub(crate) fn discharge_file_owned(
        &mut self,
        registry: &mut DrmCleanupRegistry,
    ) -> Result<(), io::Error> {
        if let Some(fo) = self.file_owned.take() {
            match fo.discharge(registry) {
                Ok(()) => {}
                Err((err, returned)) => {
                    self.file_owned = Some(returned);
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn close_file_owned_after_family(&mut self) {
        if let Some(file_owned) = self.file_owned.take() {
            file_owned.close_after_family();
        }
    }

    /// Inverse of `from_scanout_bo_backing`, for the F2-M2 rollback when a
    /// paired managed adoption fails after this side already succeeded.
    /// Never touches the registry: `register_right` has no persistent
    /// side effect until `consume`/`register_payload_alias` observes the
    /// right, and neither has happened yet for a payload that was only
    /// ever adopted, never discharged.
    pub(crate) fn into_scanout_bo_backing(mut self) -> ScanoutBoBacking {
        // `SharedBacking` has a `Drop`, so it (and this whole struct) cannot
        // be destructured by value; extract each field via replace/take
        // instead, same as `take_physical_backing` does on `&mut self`. The
        // leftover `self` (image/memory/view null, transfer empty, vk
        // `None`) drops normally at the end of this function -- its `Drop`
        // guards make that a safe no-op, with nothing left to close.
        let FileOwnedBacking {
            right,
            gbm_bo,
            device,
        } = self
            .file_owned
            .take()
            .expect("rollback is only ever called on a file-owned scanout allocation");
        let image = std::mem::replace(&mut self.shared.image, vk::Image::null());
        let memory = std::mem::replace(&mut self.shared.memory, vk::DeviceMemory::null());
        let view = std::mem::replace(&mut self.shared.view, vk::ImageView::null());
        let transfer = std::mem::replace(&mut self.shared.transfer, TransferResources::empty());
        let vk = self
            .shared
            .vk
            .take()
            .expect("a real (non-mock) allocation's shared half always has Some(vk)");
        ScanoutBoBacking {
            fb_handle: Some(framebuffer::Handle::from(
                NonZeroU32::new(right.fb()).expect("a registered right has a nonzero fb id"),
            )),
            gem_handle: Some(DrmBufferHandle::from(
                NonZeroU32::new(right.gem()).expect("a registered right has a nonzero gem id"),
            )),
            gbm_bo,
            drm: device,
            image,
            memory,
            view,
            transfer,
            vk,
        }
    }

    /// Consuming conversion from a live `ScanoutBo` (B-13): mints the FB/GEM
    /// right against `registry` and assembles the extracted physical fields
    /// into a `ScanoutAllocation`. The source bo is left an emptied husk
    /// (see `ScanoutBo::take_physical_backing`) -- its own legacy `Drop`
    /// stays capable of tearing down a *different*, still-owning bo, but has
    /// nothing left to close for this one.
    pub(crate) fn from_scanout_bo_backing(
        backing: ScanoutBoBacking,
        registry: &mut DrmCleanupRegistry,
    ) -> Result<Self, ResourceError> {
        let ScanoutBoBacking {
            fb_handle,
            gem_handle,
            gbm_bo,
            drm,
            image,
            memory,
            view,
            transfer,
            vk,
        } = backing;
        let fb = fb_handle.ok_or(ResourceError::InvalidState)?;
        let gem = gem_handle.ok_or(ResourceError::InvalidState)?;
        let gem_owner = if gbm_bo.is_some() {
            GemOwner::Gbm
        } else {
            GemOwner::Right
        };
        let right = registry.register_right(u32::from(fb), u32::from(gem), gem_owner);
        let file_owned = FileOwnedBacking::new(right, gbm_bo, drm)?;
        let shared = SharedBacking::new(image, memory, view, transfer, vk, None);
        Ok(Self::new(Some(file_owned), shared))
    }
}

/// Renderer-side copied scanout allocation paired with an independent sink-local destination.
#[allow(dead_code)]
pub(crate) struct CopiedSourceAllocation {
    pub(crate) imported_on_sink: Option<DrawableImage>,
    pub(crate) transport_on_renderer: Option<ExportableImage>,
    pub(crate) render_target: Option<DrawableImage>,
    pub(crate) completion_semaphore: vk::Semaphore,
    pub(crate) completion_semaphore_reuse: ExportSemaphoreReuseState,
    pub(crate) transfer: TransferResources,
    pub(crate) last_gpu_render_ns: Option<u64>,
    pub(crate) render_vk: Option<Arc<VkContext>>,
    pub(crate) sink_vk: Option<Arc<VkContext>>,
    pub(crate) sink_wait_semaphore: Option<vk::Semaphore>,
    pub(crate) renderer_wait_semaphore: Option<vk::Semaphore>,
    pub(crate) renderer_return_completion: Option<RetainedSyncFile>,
    pub(crate) ownership: CopiedSourceOwnership,
}

impl fmt::Debug for CopiedSourceAllocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CopiedSourceAllocation")
            .field("completion_semaphore", &self.completion_semaphore)
            .field(
                "completion_semaphore_reuse",
                &self.completion_semaphore_reuse,
            )
            .field("ownership", &self.ownership)
            .finish()
    }
}

#[allow(dead_code)]
impl CopiedSourceAllocation {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        imported_on_sink: Option<DrawableImage>,
        transport_on_renderer: Option<ExportableImage>,
        render_target: Option<DrawableImage>,
        completion_semaphore: vk::Semaphore,
        completion_semaphore_reuse: ExportSemaphoreReuseState,
        transfer: TransferResources,
        last_gpu_render_ns: Option<u64>,
        render_vk: Arc<VkContext>,
        sink_vk: Arc<VkContext>,
        sink_wait_semaphore: Option<vk::Semaphore>,
        renderer_wait_semaphore: Option<vk::Semaphore>,
        renderer_return_completion: Option<RetainedSyncFile>,
        ownership: CopiedSourceOwnership,
    ) -> Self {
        Self {
            imported_on_sink,
            transport_on_renderer,
            render_target,
            completion_semaphore,
            completion_semaphore_reuse,
            transfer,
            last_gpu_render_ns,
            render_vk: Some(render_vk),
            sink_vk: Some(sink_vk),
            sink_wait_semaphore,
            renderer_wait_semaphore,
            renderer_return_completion,
            ownership,
        }
    }

    /// Test-only: a `CopiedSourceAllocation` with no Vulkan context. Never
    /// reachable from production (M-23) -- see `SharedBacking::mock`; every
    /// real allocation goes through `new`/`from_copied_render_source_backing`
    /// and always carries `Some`.
    #[cfg(test)]
    pub(crate) fn mock(
        completion_semaphore: vk::Semaphore,
        transfer: TransferResources,
        ownership: CopiedSourceOwnership,
    ) -> Self {
        Self {
            imported_on_sink: None,
            transport_on_renderer: None,
            render_target: None,
            completion_semaphore,
            completion_semaphore_reuse: ExportSemaphoreReuseState::Reusable,
            transfer,
            last_gpu_render_ns: None,
            render_vk: None,
            sink_vk: None,
            sink_wait_semaphore: None,
            renderer_wait_semaphore: None,
            renderer_return_completion: None,
            ownership,
        }
    }

    /// Consuming conversion from a live `CopiedRenderSource` (B-13): moves
    /// every extracted physical field into a `CopiedSourceAllocation`. The
    /// source is left an emptied husk (see
    /// `CopiedRenderSource::take_physical_backing`).
    pub(crate) fn from_copied_render_source_backing(backing: CopiedRenderSourceBacking) -> Self {
        let CopiedRenderSourceBacking {
            imported_on_sink,
            transport_on_renderer,
            render_target,
            completion_semaphore,
            completion_semaphore_reuse,
            transfer,
            last_gpu_render_ns,
            render_vk,
            sink_vk,
            sink_wait_semaphore,
            renderer_wait_semaphore,
            renderer_return_completion,
            ownership,
        } = backing;
        Self {
            imported_on_sink,
            transport_on_renderer,
            render_target,
            completion_semaphore,
            completion_semaphore_reuse,
            transfer,
            last_gpu_render_ns,
            render_vk: Some(render_vk),
            sink_vk: Some(sink_vk),
            sink_wait_semaphore,
            renderer_wait_semaphore,
            renderer_return_completion,
            ownership,
        }
    }

    /// Inverse of `from_copied_render_source_backing`, for the F2-M2
    /// rollback when a paired managed adoption fails.
    pub(crate) fn into_copied_render_source_backing(mut self) -> CopiedRenderSourceBacking {
        // `Self` has a `Drop`, so extract fields via replace/take instead of
        // destructuring by value (same reasoning as
        // `ScanoutAllocation::into_scanout_bo_backing`).
        let imported_on_sink = self.imported_on_sink.take();
        let transport_on_renderer = self.transport_on_renderer.take();
        let render_target = self.render_target.take();
        let completion_semaphore =
            std::mem::replace(&mut self.completion_semaphore, vk::Semaphore::null());
        let completion_semaphore_reuse = self.completion_semaphore_reuse;
        let transfer = std::mem::replace(&mut self.transfer, TransferResources::empty());
        let last_gpu_render_ns = self.last_gpu_render_ns.take();
        let render_vk = self
            .render_vk
            .take()
            .expect("a real (non-mock) allocation always has Some(render_vk)");
        let sink_vk = self
            .sink_vk
            .take()
            .expect("a real (non-mock) allocation always has Some(sink_vk)");
        let sink_wait_semaphore = self.sink_wait_semaphore.take();
        let renderer_wait_semaphore = self.renderer_wait_semaphore.take();
        let renderer_return_completion = self.renderer_return_completion.take();
        let ownership = self.ownership;
        CopiedRenderSourceBacking {
            imported_on_sink,
            transport_on_renderer,
            render_target,
            completion_semaphore,
            completion_semaphore_reuse,
            transfer,
            last_gpu_render_ns,
            render_vk,
            sink_vk,
            sink_wait_semaphore,
            renderer_wait_semaphore,
            renderer_return_completion,
            ownership,
        }
    }
}

impl Drop for CopiedSourceAllocation {
    fn drop(&mut self) {
        if let Some(render_vk) = &self.render_vk {
            unsafe {
                if let Some(sem) = self.renderer_wait_semaphore.take() {
                    render_vk.device.destroy_semaphore(sem, None);
                }
                if self.completion_semaphore != vk::Semaphore::null() {
                    render_vk
                        .device
                        .destroy_semaphore(self.completion_semaphore, None);
                }
                // M-22: the legacy `CopiedRenderSource::Drop` also destroys
                // its transfer resources; this Drop omitted that entirely.
                if self.transfer.command_pool != vk::CommandPool::null() {
                    destroy_transfer_resources(render_vk, &mut self.transfer);
                }
            }
        }
        if let Some(sink_vk) = &self.sink_vk {
            unsafe {
                if let Some(sem) = self.sink_wait_semaphore.take() {
                    sink_vk.device.destroy_semaphore(sem, None);
                }
            }
        }
    }
}

/// Token returned by `acquire_managed_scanout_bo`. Carries the display and optional
/// renderer allocation leases, output key, and generation metadata. Non-Copy.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct ManagedScanoutToken {
    pub display: AllocationLease,
    pub renderer: Option<AllocationLease>,
    pub output: CrtcKey,
    pub bo_idx: usize,
    pub topology_generation: u64,
    pub last_present_generation: Option<u64>,
    pub content_invalidated: bool,
}
