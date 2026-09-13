use std::sync::Arc;

use ash::vk;

use super::{
    availability::{AllocationKey, ResourceError},
    lease::AllocationLease,
};
use crate::kms::render::{PlatformBackend, store::ImportedDmabufMetadata, target::PaintTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PixelIdentity {
    pub target: PaintTarget,
    pub allocation: AllocationKey,
    pub content_offset: (i32, i32),
    pub extent: vk::Extent2D,
    pub format: vk::Format,
    pub image_view: vk::ImageView,
    pub sample_view: vk::ImageView,
    pub image: vk::Image,
}

#[derive(Debug)]
pub(crate) struct StorageLease {
    pub allocation: AllocationLease,
    pub pixels: PixelIdentity,
    pub current_layout: std::cell::Cell<vk::ImageLayout>,
}

#[derive(Debug)]
pub(crate) enum StorageAccessError {
    Resource(ResourceError),
    Vulkan(vk::Result),
}

impl std::fmt::Display for StorageAccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resource(err) => write!(f, "resource error: {err:?}"),
            Self::Vulkan(res) => write!(f, "vulkan error: {res}"),
        }
    }
}

impl std::error::Error for StorageAccessError {}

impl From<ResourceError> for StorageAccessError {
    fn from(err: ResourceError) -> Self {
        Self::Resource(err)
    }
}

impl From<vk::Result> for StorageAccessError {
    fn from(res: vk::Result) -> Self {
        Self::Vulkan(res)
    }
}

pub(crate) struct StorageAllocation {
    pub(crate) image: vk::Image,
    pub(crate) memory: vk::DeviceMemory,
    pub(crate) image_view: vk::ImageView,
    pub(crate) sample_view: vk::ImageView,
    pub(crate) extent: vk::Extent2D,
    pub(crate) format: vk::Format,
    pub(crate) depth: u8,
    pub(crate) current_layout: vk::ImageLayout,
    pub(crate) is_test_stub: bool,
    pub(crate) imported_drawable: Option<crate::kms::vk::target::DrawableImage>,
    pub(crate) imported_dmabuf: Option<ImportedDmabufMetadata>,
    pub(crate) promoted_exportable: bool,
    pub(crate) export_stride: u32,
    pub(crate) export_size: u64,
    pub(crate) export_modifier: u64,
    pub(crate) vk: Option<Arc<crate::kms::vk::device::VkContext>>,
    pub(crate) pixmap_pool: Option<Arc<crate::kms::vk::pixmap_pool::PixmapPool>>,
}

impl std::fmt::Debug for StorageAllocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageAllocation")
            .field("image", &self.image)
            .field("extent", &self.extent)
            .field("format", &self.format)
            .field("depth", &self.depth)
            .field("current_layout", &self.current_layout)
            .field("is_test_stub", &self.is_test_stub)
            .field("promoted_exportable", &self.promoted_exportable)
            .field("export_stride", &self.export_stride)
            .field("export_size", &self.export_size)
            .field("export_modifier", &self.export_modifier)
            .finish()
    }
}

impl StorageAllocation {
    pub(crate) fn is_exportable(&self) -> bool {
        self.imported_drawable.is_some() || self.promoted_exportable
    }

    pub(crate) fn destroy(&mut self, platform: &PlatformBackend) {
        if self.vk.is_none() {
            self.vk = platform.vk.clone();
        }
        if self.pixmap_pool.is_none() {
            self.pixmap_pool = platform.pixmap_pool.clone();
        }
        self.cleanup_handles();
    }

    fn cleanup_handles(&mut self) {
        if self.is_test_stub {
            return;
        }
        if self.imported_drawable.is_some() {
            if let Some(vk) = self.vk.as_ref()
                && self.sample_view != vk::ImageView::null()
            {
                unsafe { vk.device.destroy_image_view(self.sample_view, None) };
            }
            self.sample_view = vk::ImageView::null();
            self.image = vk::Image::null();
            self.image_view = vk::ImageView::null();
            self.memory = vk::DeviceMemory::null();
            self.imported_drawable = None;
            return;
        }

        let Some(vk) = self.vk.as_ref() else {
            return;
        };

        if self.sample_view != vk::ImageView::null() {
            unsafe { vk.device.destroy_image_view(self.sample_view, None) };
            self.sample_view = vk::ImageView::null();
        }

        if !self.promoted_exportable
            && self.image != vk::Image::null()
            && self.image_view != vk::ImageView::null()
            && self.memory != vk::DeviceMemory::null()
            && let Some(pool) = self.pixmap_pool.as_ref()
        {
            let key = crate::kms::vk::pixmap_pool::PixmapPoolKey {
                width: self.extent.width,
                height: self.extent.height,
                format: self.format,
            };
            let entry = crate::kms::vk::pixmap_pool::PooledPixmapImage {
                image: self.image,
                view: self.image_view,
                memory: self.memory,
                current_layout: self.current_layout,
            };
            if pool.try_return(key, entry).is_ok() {
                self.image = vk::Image::null();
                self.image_view = vk::ImageView::null();
                self.memory = vk::DeviceMemory::null();
                return;
            }
        }

        unsafe {
            if self.image_view != vk::ImageView::null() {
                vk.device.destroy_image_view(self.image_view, None);
                self.image_view = vk::ImageView::null();
            }
            if self.image != vk::Image::null() {
                vk.device.destroy_image(self.image, None);
                self.image = vk::Image::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                vk.device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
        }
    }
}

impl Drop for StorageAllocation {
    fn drop(&mut self) {
        self.cleanup_handles();
    }
}

#[derive(Debug)]
pub(crate) enum StorageBacking {
    Legacy(StorageAllocation),
    Managed(StorageLease),
    Detached,
}
