#[allow(unused_imports)]
use std::{fmt, io, os::fd::OwnedFd, rc::Rc, sync::Arc};

use ash::vk;

use super::{
    AllocationLease, ResourceError,
    drm_cleanup::{DrmCleanupRegistry, DrmCleanupRight, GemOwner},
};
use crate::kms::{
    render::platform::CrtcKey,
    vk::{
        device::VkContext,
        scanout::{
            CopiedSourceOwnership, ExportSemaphoreReuseState, RetainedSyncFile, TransferResources,
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
            Ok(()) => Ok(()),
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

    pub(crate) fn discharge_file_owned(
        &mut self,
        registry: &mut DrmCleanupRegistry,
    ) -> Result<(), (io::Error, FileOwnedBacking)> {
        if let Some(fo) = self.file_owned.take()
            && let Err((err, returned)) = fo.discharge(registry)
        {
            self.file_owned = Some(returned);
            return Err((err, self.file_owned.take().unwrap()));
        }
        Ok(())
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
    pub topology_generation: u64,
    pub last_present_generation: u64,
    pub content_invalidated: bool,
}
