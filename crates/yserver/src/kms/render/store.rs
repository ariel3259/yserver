//! `DrawableStore` — drawable storage + lifetime + damage.
//!
//! Per rendering-model-v2 spec § "DrawableStore — drawable storage +
//! lifetime" and Stage 2 plan substage 2b. Owns every drawable's
//! storage handle, refcount, retirement-generation against I6a
//! [`FenceTicket`]s, image-layout state, and the **two damage lists**
//! per I5 (presentation damage with snapshot/ack semantics + protocol
//! damage for the DAMAGE extension).
//!
//! Stage 2b lands the structure + tests. KmsBackend wires a
//! handful of allocation paths through; full wiring (every
//! allocation method on the Backend trait) arrives across
//! Stages 2c–2d as those substages need the metadata side of
//! the drawables they paint into.
//!
//! Storage allocation is **split** from the metadata layer:
//! `PlatformBackend` creates the Vk handles ([`Storage`]) and
//! hands them to `DrawableStore::allocate`. This keeps the
//! store's allocation path uniform across production (real
//! VkContext) and tests (synthesised null handles) — both go
//! through the same metadata bookkeeping.

#![allow(
    dead_code,
    reason = "DrawableStore primitives are consumed by Stages 2c–2e"
)]

use std::{
    collections::{HashMap, HashSet},
    os::fd::OwnedFd,
    sync::Arc,
};

use ash::vk;

#[allow(unused_imports)]
use super::{
    platform::{FenceTicket, PlatformBackend},
    resources::{
        AllocationPayload, ObligationKind, PixelIdentity, ResourceError, ResourceService,
        StorageAccessError, StorageAllocation, StorageBacking, StorageLease, UseKind,
    },
    target::PaintTarget,
};

// ────────────────────────────────────────────────────────────────
// Identity + classification
// ────────────────────────────────────────────────────────────────

/// Opaque per-drawable handle. Stable across resize (the
/// `vk::Image` may change but the id doesn't).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct DrawableId(u64);

impl DrawableId {
    /// Raw value for diagnostic logging (`YSERVER_SUBMIT_TRACE`).
    /// Do not use to build new ids — that's [`DrawableStore`]'s
    /// job.
    #[must_use]
    pub(crate) fn as_u64(self) -> u64 {
        self.0
    }

    /// Test-only constructor. Production callers must allocate via
    /// `DrawableStore::allocate(...)` so the store's bookkeeping stays
    /// consistent.
    #[cfg(test)]
    pub(crate) fn for_tests(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrawableKind {
    /// Single root storage covering the full virtual-screen
    /// extent. Always scene-participating.
    Root,
    /// One per InputOutput window (after `map_subwindow`).
    /// Scene-participation toggles with map state.
    Window,
    /// One per X11 Pixmap. Never scene-participating.
    Pixmap,
    /// One per server-allocated cursor (Cursor / GlyphCursor /
    /// RenderCreateCursor). Scene-participating when active.
    Cursor,
    /// COMPOSITE `NameWindowPixmap` / `AllocateRedirectedBacking`
    /// target. Per I4, never scene-participating in v2 Stage 2
    /// (Stage 4 connects them to the visible scene path).
    RedirectedBacking,
    // COW deferred to Stage 4.
}

/// Immutable dma-buf layout supplied by the DRI3 client at import time.
/// Vulkan consumes this information while importing the image, but retaining
/// it here lets later Present/scanout policy prove KMS compatibility without
/// querying or mutating the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportedDmabufMetadata {
    pub(crate) fourcc: u32,
    pub(crate) vk_format: vk::Format,
    pub(crate) modifier: u64,
    /// True when the client never named the layout (legacy
    /// `PixmapFromBuffer`), so `modifier` above is **our guess**, not a
    /// fact. Any consumer that acts on the layout -- notably the M1
    /// direct-scanout probe, which would hand the buffer to KMS
    /// described as linear -- must refuse such a pixmap rather than
    /// trust the field.
    pub(crate) implicit_layout: bool,
    pub(crate) planes: Vec<ImportedDmabufPlane>,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) depth: u8,
    pub(crate) bpp: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImportedDmabufPlane {
    pub(crate) offset: u64,
    pub(crate) pitch: u32,
}

// ────────────────────────────────────────────────────────────────
// Storage handles — the Vk side of a drawable.
// PlatformBackend creates these; DrawableStore borrows them.
// ────────────────────────────────────────────────────────────────

/// The Vk resources backing one drawable. Logical facade wrapping [`StorageBacking`].
pub(crate) struct Storage {
    pub(crate) backing: StorageBacking,
}

impl std::ops::Deref for Storage {
    type Target = StorageAllocation;

    fn deref(&self) -> &Self::Target {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc,
            StorageBacking::Managed(_) => {
                panic!(
                    "StorageAllocation in Managed backing must be accessed via ResourceService::with_storage_read/write"
                );
            }
            StorageBacking::Detached => {
                panic!("StorageAllocation in Detached backing must not be accessed");
            }
        }
    }
}

impl std::ops::DerefMut for Storage {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match &mut self.backing {
            StorageBacking::Legacy(alloc) => alloc,
            StorageBacking::Managed(_) => {
                panic!(
                    "StorageAllocation in Managed backing must be accessed via ResourceService::with_storage_read/write"
                );
            }
            StorageBacking::Detached => {
                panic!("StorageAllocation in Detached backing must not be accessed");
            }
        }
    }
}

/// Old Vk handles displaced by promotion ([`Storage::adopt_exportable`]).
/// Destroyed by the engine only once the fence guarding the old image's
/// last render has signaled.
#[derive(Debug)]
pub(crate) struct RetiredImage {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub image_view: vk::ImageView,
    pub sample_view: vk::ImageView,
}

impl Storage {
    pub(crate) fn from_backing(backing: StorageBacking) -> Self {
        Self { backing }
    }

    pub(crate) fn backing(&self) -> &StorageBacking {
        &self.backing
    }

    pub(crate) fn backing_mut(&mut self) -> &mut StorageBacking {
        &mut self.backing
    }

    pub(crate) fn is_managed(&self) -> bool {
        matches!(self.backing, StorageBacking::Managed(_))
    }

    pub(crate) fn is_detached(&self) -> bool {
        matches!(self.backing, StorageBacking::Detached)
    }

    pub(crate) fn managed_lease(&self) -> Option<&StorageLease> {
        match &self.backing {
            StorageBacking::Managed(lease) => Some(lease),
            StorageBacking::Legacy(_) | StorageBacking::Detached => None,
        }
    }

    pub(crate) fn extent(&self) -> vk::Extent2D {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.extent,
            StorageBacking::Managed(lease) => lease.pixels.extent,
            StorageBacking::Detached => vk::Extent2D::default(),
        }
    }

    pub(crate) fn depth(&self) -> u8 {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.depth,
            StorageBacking::Managed(lease) => lease.pixels.target.x11_depth(),
            StorageBacking::Detached => 0,
        }
    }

    pub(crate) fn content_offset(&self) -> (i32, i32) {
        match &self.backing {
            StorageBacking::Legacy(_) | StorageBacking::Detached => (0, 0),
            StorageBacking::Managed(lease) => lease.pixels.content_offset,
        }
    }

    pub(crate) fn format(&self) -> vk::Format {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.format,
            StorageBacking::Managed(lease) => lease.pixels.format,
            StorageBacking::Detached => vk::Format::UNDEFINED,
        }
    }

    pub(crate) fn image_view(&self) -> vk::ImageView {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.image_view,
            StorageBacking::Managed(lease) => lease.pixels.image_view,
            StorageBacking::Detached => vk::ImageView::null(),
        }
    }

    pub(crate) fn sample_view(&self) -> vk::ImageView {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.sample_view,
            StorageBacking::Managed(lease) => lease.pixels.sample_view,
            StorageBacking::Detached => vk::ImageView::null(),
        }
    }

    pub(crate) fn has_image_view(&self) -> bool {
        self.image_view() != vk::ImageView::null()
    }

    pub(crate) fn image(&self) -> vk::Image {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.image,
            StorageBacking::Managed(lease) => lease.pixels.image,
            StorageBacking::Detached => vk::Image::null(),
        }
    }

    /// F11-B1: `current_layout` has exactly one copy --
    /// `StorageAllocation::current_layout` behind the entry's
    /// reservation protocol. `Legacy` reads/writes its own field
    /// directly (byte-for-byte the pre-fix behaviour: never needs a
    /// service). `Managed` reserves a `Read` use via
    /// `ResourceService::with_storage_read`, so a live incompatible
    /// use (e.g. a writer) is `Err(Busy)` instead of racing;
    /// `service: None` is `Err(InvalidState)` -- never a silent
    /// fallback value, matching the precedent
    /// `RenderEngine::promote_drawable_exportable` already set
    /// (`engine.rs` ~3650). `Detached` is a no-op success, matching
    /// its other accessors (`extent()`, `format()`, ...).
    pub(crate) fn current_layout(
        &self,
        service: Option<&mut ResourceService>,
    ) -> Result<vk::ImageLayout, ResourceError> {
        match &self.backing {
            StorageBacking::Legacy(alloc) => Ok(alloc.current_layout),
            StorageBacking::Managed(lease) => {
                let svc = service.ok_or(ResourceError::InvalidState)?;
                svc.with_storage_read(lease, |alloc| alloc.current_layout)
            }
            StorageBacking::Detached => Ok(vk::ImageLayout::UNDEFINED),
        }
    }

    /// Write counterpart of [`Self::current_layout`] (F11-B1): `Managed`
    /// reserves a `Write` use via `ResourceService::with_storage_write`
    /// instead of mutating a lease-local shadow with no reservation at
    /// all (the pre-fix bug -- a live `Read` reservation did not stop
    /// this from racing `record_layout_transition_managed`). A refused
    /// reservation (`Busy`) and a missing service (`InvalidState`) both
    /// propagate; neither is swallowed into a silent no-op.
    pub(crate) fn set_current_layout(
        &mut self,
        layout: vk::ImageLayout,
        service: Option<&mut ResourceService>,
    ) -> Result<(), ResourceError> {
        match &mut self.backing {
            StorageBacking::Legacy(alloc) => {
                alloc.current_layout = layout;
                Ok(())
            }
            StorageBacking::Managed(lease) => {
                let svc = service.ok_or(ResourceError::InvalidState)?;
                svc.with_storage_write(lease, |alloc| alloc.current_layout = layout)
            }
            StorageBacking::Detached => Ok(()),
        }
    }

    pub(crate) fn imported_dmabuf(&self) -> Option<&ImportedDmabufMetadata> {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.imported_dmabuf.as_ref(),
            StorageBacking::Managed(_) | StorageBacking::Detached => None,
        }
    }

    pub(crate) fn imported_drawable(&self) -> Option<&crate::kms::vk::target::DrawableImage> {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.imported_drawable.as_ref(),
            StorageBacking::Managed(_) | StorageBacking::Detached => None,
        }
    }

    pub(crate) fn export_metadata(
        &self,
        service: Option<&mut ResourceService>,
    ) -> Option<(vk::DeviceMemory, u32, u64, u64)> {
        match &self.backing {
            StorageBacking::Legacy(alloc) => Some((
                alloc.memory,
                alloc.export_stride,
                alloc.export_size,
                alloc.export_modifier,
            )),
            StorageBacking::Managed(lease) => {
                if let Some(svc) = service {
                    svc.with_storage_read(lease, |alloc| {
                        (
                            alloc.memory,
                            alloc.export_stride,
                            alloc.export_size,
                            alloc.export_modifier,
                        )
                    })
                    .ok()
                } else {
                    None
                }
            }
            StorageBacking::Detached => None,
        }
    }

    pub(crate) fn memory(&self) -> vk::DeviceMemory {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.memory,
            StorageBacking::Managed(_) | StorageBacking::Detached => vk::DeviceMemory::null(),
        }
    }

    pub(crate) fn export_stride(&self) -> u32 {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.export_stride,
            StorageBacking::Managed(_) | StorageBacking::Detached => 0,
        }
    }

    pub(crate) fn export_size(&self) -> u64 {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.export_size,
            StorageBacking::Managed(_) | StorageBacking::Detached => 0,
        }
    }

    pub(crate) fn export_modifier(&self) -> u64 {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.export_modifier,
            StorageBacking::Managed(_) | StorageBacking::Detached => 0,
        }
    }

    /// Production constructor — Vk handles owned by `PlatformBackend::
    /// allocate_drawable_storage`. Initial layout is `UNDEFINED`;
    /// transitions tracked thereafter via
    /// [`Drawable::record_layout_transition`].
    pub(crate) fn new_server_owned(
        image: vk::Image,
        memory: vk::DeviceMemory,
        image_view: vk::ImageView,
        sample_view: vk::ImageView,
        extent: vk::Extent2D,
        format: vk::Format,
        depth: u8,
    ) -> Self {
        Self {
            backing: StorageBacking::Legacy(StorageAllocation {
                image,
                memory,
                image_view,
                sample_view,
                extent,
                format,
                depth,
                current_layout: vk::ImageLayout::UNDEFINED,
                is_test_stub: false,
                imported_drawable: None,
                imported_dmabuf: None,
                promoted_exportable: false,
                export_stride: 0,
                export_size: 0,
                export_modifier: 0,
                vk: None,
                pixmap_pool: None,
            }),
        }
    }

    /// Wrap a DRI3-imported [`DrawableImage`](crate::kms::vk::target::DrawableImage)
    /// as a v2 `Storage`. The handles in `image`/`memory`/`image_view`
    /// alias the inner `DrawableImage`; the inner `Drop` owns the
    /// release of those handles + the imported dma-buf fd.
    /// `current_layout` starts at `UNDEFINED` (`DrawableImage`'s
    /// `from_dmabuf` likewise leaves the image undefined until the
    /// first paint barrier). Used by `dri3_import_pixmap`.
    pub(crate) fn from_imported_drawable_image(
        drawable: crate::kms::vk::target::DrawableImage,
        sample_view: vk::ImageView,
        depth: u8,
        imported_dmabuf: ImportedDmabufMetadata,
    ) -> Self {
        let image = drawable.vk_image;
        let image_view = drawable.vk_image_view;
        let memory = drawable.backing_memory();
        let extent = drawable.extent;
        let format = drawable.format;
        Self {
            backing: StorageBacking::Legacy(StorageAllocation {
                image,
                memory,
                image_view,
                sample_view,
                extent,
                format,
                depth,
                current_layout: vk::ImageLayout::UNDEFINED,
                is_test_stub: false,
                imported_drawable: Some(drawable),
                imported_dmabuf: Some(imported_dmabuf),
                promoted_exportable: false,
                export_stride: 0,
                export_size: 0,
                export_modifier: 0,
                vk: None,
                pixmap_pool: None,
            }),
        }
    }

    /// Stage 3f.10: pool-take constructor. Reuses a recycled
    /// `PooledPixmapImage` triple (image + memory + view) +
    /// inherits the pool entry's tracked layout so subsequent
    /// ops transition from the right source state.
    pub(crate) fn from_pooled(
        pooled: crate::kms::vk::pixmap_pool::PooledPixmapImage,
        sample_view: vk::ImageView,
        extent: vk::Extent2D,
        format: vk::Format,
        depth: u8,
    ) -> Self {
        Self {
            backing: StorageBacking::Legacy(StorageAllocation {
                image: pooled.image,
                memory: pooled.memory,
                image_view: pooled.view,
                sample_view,
                extent,
                format,
                depth,
                current_layout: pooled.current_layout,
                is_test_stub: false,
                imported_drawable: None,
                imported_dmabuf: None,
                promoted_exportable: false,
                export_stride: 0,
                export_size: 0,
                export_modifier: 0,
                vk: None,
                pixmap_pool: None,
            }),
        }
    }

    /// Test-only constructor with null Vk handles. Used by
    /// unit tests that exercise refcount / damage / snapshot
    /// logic without needing a live VkContext.
    #[doc(hidden)]
    pub(crate) fn for_tests_null(extent: vk::Extent2D, format: vk::Format) -> Self {
        let depth = match format {
            vk::Format::R8_UNORM => 8,
            _ => 32,
        };
        Self {
            backing: StorageBacking::Legacy(StorageAllocation {
                image: vk::Image::null(),
                memory: vk::DeviceMemory::null(),
                image_view: vk::ImageView::null(),
                sample_view: vk::ImageView::null(),
                depth,
                extent,
                format,
                current_layout: vk::ImageLayout::UNDEFINED,
                is_test_stub: true,
                imported_drawable: None,
                imported_dmabuf: None,
                promoted_exportable: false,
                export_stride: 0,
                export_size: 0,
                export_modifier: 0,
                vk: None,
                pixmap_pool: None,
            }),
        }
    }

    /// True when this storage's memory is already dma-buf-exportable
    /// (DRI3-imported, or previously promoted). Used to avoid
    /// re-promoting an already-exportable drawable.
    ///
    /// `Legacy` reads its own field directly, no service needed.
    /// `Managed` reserves a `Read` use via `with_storage_read` when a
    /// service is given; a *refused* reservation (`Busy`) is distinct
    /// from a real "not exportable" answer (F12-m1) -- distinguishing
    /// them would mean returning `Result`, which every one of this
    /// method's ~180 call sites across `engine.rs`/`backend.rs` would
    /// have to unwrap for a case that can't occur yet (no production
    /// or test caller reaches `Managed` storage through these paths,
    /// R8), so a refusal is logged (not silently folded into "false"
    /// the way it was pre-fix) and the bool contract is kept; a
    /// missing service also logs and answers `false` rather than
    /// panicking. Use [`Self::is_exportable_managed`] where a caller
    /// already holds a `&mut ResourceService` and wants the refusal
    /// itself, not just a log line.
    pub(crate) fn is_exportable(&self, service: Option<&mut ResourceService>) -> bool {
        match &self.backing {
            StorageBacking::Legacy(alloc) => alloc.is_exportable(),
            StorageBacking::Managed(lease) => match service {
                Some(svc) => match svc.with_storage_read(lease, StorageAllocation::is_exportable) {
                    Ok(exportable) => exportable,
                    Err(e) => {
                        log::warn!(
                            "Storage::is_exportable: managed read reservation refused ({e:?}); \
                             reporting not-exportable rather than retrying"
                        );
                        false
                    }
                },
                None => {
                    log::warn!(
                        "Storage::is_exportable: managed storage queried with no \
                         ResourceService; reporting not-exportable"
                    );
                    false
                }
            },
            StorageBacking::Detached => false,
        }
    }

    /// Managed-storage counterpart of [`Self::is_exportable`] (M-18):
    /// reserves a `Read` use via `ResourceService::with_storage_read`
    /// instead of reaching into `AllocationEntry.payload` directly, so a
    /// concurrent incompatible reservation is caught by
    /// `EntryAvailability::is_compatible` instead of silently racing the
    /// backing `RefCell`. No production or test caller reaches managed
    /// storage through this path yet (R8); it exists so a future caller
    /// need not reintroduce the porous access this replaces.
    pub(crate) fn is_exportable_managed(
        &self,
        service: &mut ResourceService,
    ) -> Result<bool, ResourceError> {
        match &self.backing {
            StorageBacking::Managed(lease) => {
                service.with_storage_read(lease, StorageAllocation::is_exportable)
            }
            StorageBacking::Legacy(_) | StorageBacking::Detached => Err(ResourceError::Detached),
        }
    }

    /// Adopts this storage's backing into `service`. Real (non-stub)
    /// physical memory needs a live `Arc<VkContext>` (and pool, if the
    /// image is pool-eligible) so `StorageAllocation::cleanup_handles`
    /// can actually release it later instead of finding `vk: None` and
    /// silently returning (B-14: pre-fix, `into_managed` never touched
    /// `vk`/`pixmap_pool`, so every real allocation it adopted leaked its
    /// Vulkan handles on eventual `service_ready`). `platform` supplies
    /// both; adoption of a non-stub allocation is refused when
    /// `platform.vk` is `None` rather than adopting a payload nothing can
    /// ever clean up. A stub allocation (`is_test_stub`) has no real
    /// handles to leak, so it is exempt from the refusal.
    #[allow(clippy::result_large_err)]
    pub(crate) fn into_managed(
        self,
        service: &mut ResourceService,
        platform: &PlatformBackend,
        target: PaintTarget,
        content_offset: (i32, i32),
    ) -> Result<StorageLease, (ResourceError, Storage)> {
        match self.backing {
            StorageBacking::Legacy(mut alloc) => {
                if !alloc.is_test_stub && platform.vk.is_none() {
                    return Err((
                        ResourceError::InvalidState,
                        Storage {
                            backing: StorageBacking::Legacy(alloc),
                        },
                    ));
                }
                if alloc.vk.is_none() {
                    alloc.vk = platform.vk.clone();
                }
                if alloc.pixmap_pool.is_none() {
                    alloc.pixmap_pool = platform.pixmap_pool.clone();
                }
                let extent = alloc.extent;
                let format = alloc.format;
                let image_view = alloc.image_view;
                let sample_view = alloc.sample_view;
                let image = alloc.image;
                match service.adopt(AllocationPayload::Storage(alloc)) {
                    Ok(allocation_lease) => {
                        let key = allocation_lease.key();
                        let pixels = PixelIdentity {
                            target,
                            allocation: key,
                            content_offset,
                            extent,
                            format,
                            image_view,
                            sample_view,
                            image,
                        };
                        Ok(StorageLease {
                            allocation: allocation_lease,
                            pixels,
                        })
                    }
                    Err((err, payload)) => {
                        let alloc = match payload {
                            AllocationPayload::Storage(a) => a,
                            _ => unreachable!(),
                        };
                        Err((
                            err,
                            Storage {
                                backing: StorageBacking::Legacy(alloc),
                            },
                        ))
                    }
                }
            }
            StorageBacking::Managed(lease) => match service.retain_storage(&lease) {
                Ok(new_lease) => Ok(new_lease),
                Err(err) => Err((
                    err,
                    Storage {
                        backing: StorageBacking::Managed(lease),
                    },
                )),
            },
            StorageBacking::Detached => Err((
                ResourceError::Detached,
                Storage {
                    backing: StorageBacking::Detached,
                },
            )),
        }
    }

    pub(crate) fn adopt_exportable(
        &mut self,
        new_image: vk::Image,
        new_memory: vk::DeviceMemory,
        new_sample_view: vk::ImageView,
        new_image_view: vk::ImageView,
        new_layout: vk::ImageLayout,
        export_stride: u32,
        export_size: u64,
        export_modifier: u64,
    ) -> RetiredImage {
        match &mut self.backing {
            StorageBacking::Legacy(alloc) => {
                let retired = RetiredImage {
                    image: alloc.image,
                    memory: alloc.memory,
                    image_view: alloc.image_view,
                    sample_view: alloc.sample_view,
                };
                alloc.image = new_image;
                alloc.memory = new_memory;
                alloc.image_view = new_image_view;
                alloc.sample_view = new_sample_view;
                alloc.current_layout = new_layout;
                alloc.promoted_exportable = true;
                alloc.export_stride = export_stride;
                alloc.export_size = export_size;
                alloc.export_modifier = export_modifier;
                retired
            }
            StorageBacking::Managed(_) => {
                panic!(
                    "adopt_exportable called on managed storage without service; use adopt_exportable_managed"
                );
            }
            StorageBacking::Detached => {
                panic!("adopt_exportable called on detached storage");
            }
        }
    }

    pub(crate) fn adopt_exportable_managed(
        &mut self,
        service: &mut ResourceService,
        new_image: vk::Image,
        new_memory: vk::DeviceMemory,
        new_sample_view: vk::ImageView,
        new_image_view: vk::ImageView,
        new_layout: vk::ImageLayout,
        export_stride: u32,
        export_size: u64,
        export_modifier: u64,
        vk: Option<Arc<crate::kms::vk::device::VkContext>>,
    ) -> Result<StorageLease, ResourceError> {
        let (old_lease, target, content_offset, extent, depth, format) = match &self.backing {
            StorageBacking::Managed(lease) => {
                let pixels = &lease.pixels;
                (
                    service.retain_storage(lease)?,
                    pixels.target,
                    pixels.content_offset,
                    pixels.extent,
                    pixels.target.x11_depth(),
                    service.with_storage_read(lease, |a| a.format)?,
                )
            }
            StorageBacking::Legacy(_) | StorageBacking::Detached => {
                return Err(ResourceError::Detached);
            }
        };

        let new_alloc = StorageAllocation {
            image: new_image,
            memory: new_memory,
            image_view: new_image_view,
            sample_view: new_sample_view,
            extent,
            format,
            depth,
            current_layout: new_layout,
            is_test_stub: false,
            imported_drawable: None,
            imported_dmabuf: None,
            promoted_exportable: true,
            export_stride,
            export_size,
            export_modifier,
            vk,
            pixmap_pool: None,
        };

        let new_alloc_lease = service
            .adopt(AllocationPayload::Storage(new_alloc))
            .map_err(|(e, _)| e)?;

        let new_storage_lease = StorageLease {
            pixels: PixelIdentity {
                target,
                allocation: new_alloc_lease.key(),
                content_offset,
                extent,
                format,
                image_view: new_image_view,
                sample_view: new_sample_view,
                image: new_image,
            },
            allocation: new_alloc_lease,
        };

        let _ = std::mem::replace(
            &mut self.backing,
            StorageBacking::Managed(service.retain_storage(&new_storage_lease)?),
        );

        Ok(old_lease)
    }

    /// Idempotent, like the Legacy path: repeat calls are safe (the
    /// second finds a Detached backing and no-ops).
    ///
    /// Managed storage's real Vk handles are the `ResourceService`'s to
    /// reclaim once every use/obligation clears
    /// (`service_ready`/`service_ready_with_registry`, via
    /// `StorageAllocation::Drop`), not this synchronous call's job (R4)
    /// -- but `destroy()` still detaches THIS drawable's own Retain use
    /// right here (M-20), rather than leaving it to whatever the caller
    /// does with `self` afterward. Both current callers
    /// (`DrawableStore::destroy_now`/`shutdown_destroy_all`) happen to
    /// drop `self` immediately after, which made the pre-fix `{}` net
    /// out the same by accident of caller behaviour; a bare `{}` is not
    /// correct on its own terms, and a future caller that keeps `self`
    /// alive past `destroy()` must not depend on that accident.
    pub(crate) fn destroy(&mut self, platform: &PlatformBackend) {
        match &mut self.backing {
            StorageBacking::Legacy(alloc) => alloc.destroy(platform),
            StorageBacking::Managed(_) => {
                // Overwriting `self.backing` drops the old value first
                // (the Managed lease), releasing the Retain use and
                // marking the entry dirty for the service's own
                // servicing walk -- now, not later.
                // F3-m1: transitioning to Detached is honest (does not fabricate
                // a false Legacy stub).
                self.backing = StorageBacking::Detached;
            }
            StorageBacking::Detached => {}
        }
    }
}

// ────────────────────────────────────────────────────────────────
// RegionSet — minimal Vec<Rect2D> with union / subtract.
//
// Stage 2 regions are typically <= a handful of rects per
// drawable, so a Vec-backed set is fast enough. Full pixman-
// style region algebra arrives later if profiling shows it.
// ────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub(crate) struct RegionSet {
    rects: Vec<vk::Rect2D>,
}

impl RegionSet {
    pub(crate) fn new() -> Self {
        Self { rects: Vec::new() }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub(crate) fn rects(&self) -> &[vk::Rect2D] {
        &self.rects
    }

    /// Cap on the number of rects a `RegionSet` retains. Past this the
    /// set collapses to its bounding rect (a safe superset — worst case
    /// is over-paint, never lost damage). Without a cap, a client that
    /// damages faster than page-flips drain the region (Window Maker's
    /// restack/draw storm) grows the set unbounded, and the O(n·m)
    /// [`RegionSet::subtract`] in the page-flip-complete handler then
    /// wedges the single-threaded core loop — the machine stops
    /// responding to input, ZAP included. Capping both operands keeps
    /// `subtract` at O(MAX_RECTS²) ≈ constant.
    const MAX_RECTS: usize = 256;

    /// Add a rect, coalescing to the bounding rect once the set exceeds
    /// [`Self::MAX_RECTS`] so the region stays bounded (see the const's
    /// note for why this is load-bearing, not just an optimization).
    pub(crate) fn add(&mut self, rect: vk::Rect2D) {
        if rect.extent.width == 0 || rect.extent.height == 0 {
            return;
        }
        self.rects.push(rect);
        if self.rects.len() > Self::MAX_RECTS
            && let Some(bounds) = self.bounding_rect()
        {
            self.rects.clear();
            self.rects.push(bounds);
        }
    }

    /// Union with another set. O(n) — for Stage 2's small
    /// region counts this is fine.
    pub(crate) fn union_with(&mut self, other: &RegionSet) {
        for &r in &other.rects {
            self.add(r);
        }
    }

    /// Remove one matching occurrence for each rect in `other`.
    ///
    /// Snapshot/ack users may receive a second identical damage rect while
    /// the first snapshot is in flight. Treating this as set subtraction
    /// would discard both occurrences at retirement and lose the newer
    /// damage; multiset subtraction retains it. Overlapping, non-identical
    /// rectangles remain deliberately conservative until full region algebra
    /// is needed.
    pub(crate) fn subtract(&mut self, other: &RegionSet) {
        if other.rects.is_empty() {
            return;
        }
        let mut pending = other.rects.clone();
        self.rects.retain(|r| {
            if let Some(pos) = pending
                .iter()
                .position(|o| o.offset == r.offset && o.extent == r.extent)
            {
                let _ = pending.swap_remove(pos);
                false
            } else {
                true
            }
        });
    }

    pub(crate) fn clear(&mut self) {
        self.rects.clear();
    }

    /// Bounding rect over every rect in the set. Returns `None`
    /// if the set is empty. Stage 2e uses this for the
    /// buffer-age repaint scissor — we don't yet split the
    /// scissor per-rect; Stage 5 may tighten if profiling shows
    /// over-paint matters.
    pub(crate) fn bounding_rect(&self) -> Option<vk::Rect2D> {
        let mut iter = self.rects.iter().copied();
        let first = iter.next()?;
        let mut x0 = first.offset.x;
        let mut y0 = first.offset.y;
        let mut x1 = first.offset.x.saturating_add_unsigned(first.extent.width);
        let mut y1 = first.offset.y.saturating_add_unsigned(first.extent.height);
        for r in iter {
            x0 = x0.min(r.offset.x);
            y0 = y0.min(r.offset.y);
            x1 = x1.max(r.offset.x.saturating_add_unsigned(r.extent.width));
            y1 = y1.max(r.offset.y.saturating_add_unsigned(r.extent.height));
        }
        Some(vk::Rect2D {
            offset: vk::Offset2D { x: x0, y: y0 },
            extent: vk::Extent2D {
                width: u32::try_from((x1 - x0).max(0)).unwrap_or(0),
                height: u32::try_from((y1 - y0).max(0)).unwrap_or(0),
            },
        })
    }

    /// Clone — used by snapshot/ack capture paths. The
    /// `derive(Clone)` already covers this; this method is just
    /// a name for readability at call sites.
    #[must_use]
    pub(crate) fn snapshot(&self) -> RegionSet {
        self.clone()
    }
}

// ────────────────────────────────────────────────────────────────
// DamageSnapshot — peek/ack token for the I5 snapshot/ack rule.
// ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct DamageSnapshot {
    pub(crate) id: DrawableId,
    pub(crate) epoch: u64,
    pub(crate) region: RegionSet,
}

// ────────────────────────────────────────────────────────────────
// Drawable — one entry in DrawableStore.
// ────────────────────────────────────────────────────────────────

/// Why a scene-participating drawable with pending presentation damage is not
/// arming the compose scheduler. Two reasons, two re-arm rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DormantReason {
    /// The walk emitted no piece of it on any output: fully covered,
    /// off-output, or an empty bounding shape. No paint can become visible
    /// without a structural change, and every structural change wakes the
    /// tick, so this stays dormant across paints — an off-screen window
    /// painting at 60 fps must not cost 60 walks/s (cut 2b).
    NoPieces,
    /// The walk emitted pieces of it, but its captured damage lay entirely
    /// under a cover on every output. The NEXT paint may land in the visible
    /// part, so `DrawableStore::damage` clears this reason: one walk per paint
    /// for such a window (mpv half under a terminal: ~26 walks/s, not the ~1850
    /// of a scheduler that never went dormant), re-flagged if that paint is
    /// hidden again.
    HiddenDamage,
}

pub(crate) struct Drawable {
    pub(crate) id: DrawableId,
    pub(crate) xid: u32,
    pub(crate) kind: DrawableKind,
    pub(crate) depth: u8,
    pub(crate) refcount: u32,
    pub(crate) scene_participating: bool,
    pub(crate) storage: Storage,

    /// I6a: latest render-completion ticket for which this
    /// drawable was a consumer (read or written) in flight. None
    /// = no GPU work has touched it since the last retirement.
    /// Coalesces — overwritten by the newest touch. Per cross-
    /// cutting §5 the underlying Arc keeps prior consumers
    /// alive via their own clones.
    pub(crate) last_render_ticket: Option<FenceTicket>,

    /// Presentation damage — region the scene needs to re-blit.
    /// Accumulates only when `scene_participating` is true.
    /// Drained via [`peek_presentation_damage`] +
    /// [`ack_presentation_damage`].
    pub(crate) presentation_damage: RegionSet,
    pub(crate) presentation_damage_epoch: u64,

    /// Idle free-run fix (cut 2b): set when this drawable is
    /// scene-participating and holds presentation damage but was NOT
    /// drawn by `build_scene` on ANY output last tick — i.e. it clips
    /// to an empty visible box (mapped but off-screen / empty bounding
    /// shape). Such damage can never be composed/ack'd, so it must not
    /// keep arming the compose scheduler (`has_pending_presentation_
    /// damage`) — that busy-spins the core loop at idle. The damage
    /// itself is PRESERVED (not cleared): when a structural change
    /// (map/move/restack/configure/shape/RandR — all mark
    /// `scene_structure_dirty`) brings the drawable on-screen, the
    /// forced full recompose draws it from storage and the ack drains
    /// it, clearing this flag. Reconciled only when every output walked
    /// this tick (see `SceneCompositor::tick`).
    pub(crate) dormant: Option<DormantReason>,

    /// Ungated monotonic content-write counter. Bumped (saturating) on EVERY
    /// write to this drawable's pixels — the eight engine paint entry points —
    /// regardless of `scene_participating`, unlike `presentation_damage_epoch`
    /// which is gated and so misses offscreen clip-mask writes. The clip-mask
    /// cache compares this to detect a genuine mask mutation vs a cheap
    /// clip-install re-toggle. `saturating_add` matters only at `u64::MAX`
    /// (unreachable in practice): it freezes rather than wrapping, which avoids
    /// aliasing a fresh version onto an old cached one.
    pub(crate) content_version: u64,

    /// #133 step 3 — the CONTENT OFFSET this storage was ALLOCATED
    /// with: the client-visible content starts this many pixels inside
    /// it, because a window's storage is the bordered extent placed at
    /// the window's OUTER origin (`compAllocPixmap`,
    /// `composite/compalloc.c:610`).
    ///
    /// Recorded at allocation and never derived from the window's
    /// current `border_width`. Deriving the layout from the live
    /// geometry would move the coordinate system without moving the
    /// pixels, displacing everything already drawn (the xts5 Xlib9
    /// `IncludeInferiors` regression — see
    /// `KmsBackend::storage_content_offset`). It also cannot be
    /// inferred from the extent: a redirect backing may be legitimately
    /// larger than its window.
    ///
    /// #133 step 6 (P8) re-bases this in exactly ONE place —
    /// `KmsBackend::relayout_window_leaf_storage_for_border_change` —
    /// and only together with the pixels, by reallocating and copying
    /// the old content forward or by relocating it inside the storage
    /// it already has. A re-base without that copy is the bug this
    /// field exists to prevent.
    ///
    /// `0` for every pixmap, for the root, and for every `bw == 0`
    /// window — i.e. everything before #133.
    pub(crate) content_offset: i32,

    /// Stage 4a — COMPOSITE redirect routing. When `Some(B_id)`,
    /// paint that resolves through this drawable's xid lands in
    /// `B_id` instead. Pure storage-side state; side effects on
    /// damage / refcount / `scene_participating` are the caller's
    /// responsibility (4c sets those via the dedicated Backend
    /// methods). Default `None` — no redirect.
    pub(crate) redirected_target: Option<DrawableId>,
}

impl Drawable {
    /// Record an image-layout transition on `cb` with full
    /// producer/consumer access masks. Updates
    /// `StorageAllocation::current_layout` (the ONLY copy of the
    /// layout, F11-B1) so subsequent ops see the correct old-layout in
    /// their barrier.
    ///
    /// **Single source of truth** for what the current layout is.
    /// `Legacy` mutates its own field directly (byte-for-byte the
    /// pre-fix behaviour). `Managed` delegates to
    /// [`Self::record_layout_transition_managed`], which reserves a
    /// `Write` use via `ResourceService::with_storage_write` before
    /// touching the payload or recording the barrier — `service: None`
    /// is `Err(InvalidState)` (the same shape
    /// `RenderEngine::promote_drawable_exportable` already uses,
    /// `engine.rs` ~3650), and a live incompatible reservation (e.g. a
    /// reader) is `Err(Busy)`; neither is a silent local write. Pre-fix
    /// (F-11/F-12), this arm mutated a `StorageLease`-local
    /// `Cell<vk::ImageLayout>` unconditionally, with no reservation at
    /// all, so a live reader did not stop it and the payload / the
    /// lease's Cell / a twin lease's Cell could each disagree about
    /// the same image's layout.
    pub(crate) fn record_layout_transition(
        &mut self,
        vk: &crate::kms::vk::device::VkContext,
        cb: vk::CommandBuffer,
        target_layout: vk::ImageLayout,
        src_stage: vk::PipelineStageFlags2,
        src_access: vk::AccessFlags2,
        dst_stage: vk::PipelineStageFlags2,
        dst_access: vk::AccessFlags2,
        service: Option<&mut ResourceService>,
    ) -> Result<(), ResourceError> {
        if matches!(self.storage.backing, StorageBacking::Managed(_)) {
            let svc = service.ok_or(ResourceError::InvalidState)?;
            return self.record_layout_transition_managed(
                svc,
                vk,
                cb,
                target_layout,
                src_stage,
                src_access,
                dst_stage,
                dst_access,
            );
        }
        match &mut self.storage.backing {
            StorageBacking::Legacy(alloc) => {
                if alloc.is_test_stub {
                    // Tests don't issue real Vk; just update the
                    // tracker so logic-side assertions can verify.
                    alloc.current_layout = target_layout;
                    return Ok(());
                }
                let barrier = vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(src_stage)
                    .src_access_mask(src_access)
                    .dst_stage_mask(dst_stage)
                    .dst_access_mask(dst_access)
                    .old_layout(alloc.current_layout)
                    .new_layout(target_layout)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(alloc.image)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    );
                let dep = vk::DependencyInfo::default()
                    .image_memory_barriers(std::slice::from_ref(&barrier));
                unsafe { vk.device.cmd_pipeline_barrier2(cb, &dep) };
                alloc.current_layout = target_layout;
                Ok(())
            }
            StorageBacking::Managed(_) => unreachable!("handled above"),
            StorageBacking::Detached => Ok(()),
        }
    }

    /// Managed-storage counterpart of [`Self::record_layout_transition`]
    /// (M-18, F11-B1). Reserves a `Write` use via
    /// `ResourceService::with_storage_write` before touching
    /// `current_layout` or recording the barrier, so a concurrent
    /// incompatible use (e.g. a live reader) is `Err(Busy)` instead of
    /// going undetected. `current_layout` lives only on the
    /// `StorageAllocation` payload behind that reservation — there is
    /// no lease-local shadow left to keep in sync (the pre-fix
    /// `StorageLease::current_layout: Cell<_>` this replaced could
    /// silently disagree with the payload and with other leases over
    /// the same allocation). [`Self::record_layout_transition`]
    /// delegates to this method on `Managed` whenever it has a
    /// service; this is also reachable directly wherever a caller
    /// already holds a `&mut ResourceService`.
    pub(crate) fn record_layout_transition_managed(
        &mut self,
        service: &mut ResourceService,
        vk: &crate::kms::vk::device::VkContext,
        cb: vk::CommandBuffer,
        target_layout: vk::ImageLayout,
        src_stage: vk::PipelineStageFlags2,
        src_access: vk::AccessFlags2,
        dst_stage: vk::PipelineStageFlags2,
        dst_access: vk::AccessFlags2,
    ) -> Result<(), ResourceError> {
        let StorageBacking::Managed(lease) = &self.storage.backing else {
            return Err(ResourceError::Detached);
        };
        service.with_storage_write(lease, |alloc| {
            if alloc.is_test_stub {
                alloc.current_layout = target_layout;
                return;
            }
            let barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(src_stage)
                .src_access_mask(src_access)
                .dst_stage_mask(dst_stage)
                .dst_access_mask(dst_access)
                .old_layout(alloc.current_layout)
                .new_layout(target_layout)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(alloc.image)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                );
            let dep =
                vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&barrier));
            unsafe { vk.device.cmd_pipeline_barrier2(cb, &dep) };
            alloc.current_layout = target_layout;
        })
    }

    pub(crate) fn extent(&self) -> vk::Extent2D {
        self.storage.extent()
    }

    pub(crate) fn depth(&self) -> u8 {
        self.storage.depth()
    }

    pub(crate) fn content_offset(&self) -> (i32, i32) {
        self.storage.content_offset()
    }

    pub(crate) fn image_view(&self) -> vk::ImageView {
        self.storage.image_view()
    }

    pub(crate) fn sample_view(&self) -> vk::ImageView {
        self.storage.sample_view()
    }

    pub(crate) fn has_image_view(&self) -> bool {
        self.storage.has_image_view()
    }

    pub(crate) fn image(&self) -> vk::Image {
        self.storage.image()
    }

    pub(crate) fn format(&self) -> vk::Format {
        self.storage.format()
    }

    pub(crate) fn current_layout(
        &self,
        service: Option<&mut ResourceService>,
    ) -> Result<vk::ImageLayout, ResourceError> {
        self.storage.current_layout(service)
    }

    pub(crate) fn set_current_layout(
        &mut self,
        layout: vk::ImageLayout,
        service: Option<&mut ResourceService>,
    ) -> Result<(), ResourceError> {
        self.storage.set_current_layout(layout, service)
    }

    pub(crate) fn imported_dmabuf(&self) -> Option<&ImportedDmabufMetadata> {
        self.storage.imported_dmabuf()
    }

    pub(crate) fn imported_drawable(&self) -> Option<&crate::kms::vk::target::DrawableImage> {
        self.storage.imported_drawable()
    }

    pub(crate) fn export_metadata(
        &self,
        service: Option<&mut ResourceService>,
    ) -> Option<(vk::DeviceMemory, u32, u64, u64)> {
        self.storage.export_metadata(service)
    }

    pub(crate) fn memory(&self) -> vk::DeviceMemory {
        self.storage.memory()
    }

    pub(crate) fn is_exportable(&self, service: Option<&mut ResourceService>) -> bool {
        self.storage.is_exportable(service)
    }

    pub(crate) fn managed_lease(&self) -> Option<&StorageLease> {
        self.storage.managed_lease()
    }

    pub(crate) fn is_managed(&self) -> bool {
        self.storage.is_managed()
    }

    pub(crate) fn is_detached(&self) -> bool {
        self.storage.is_detached()
    }

    pub(crate) fn export_stride(&self) -> u32 {
        self.storage.export_stride()
    }

    pub(crate) fn export_size(&self) -> u64 {
        self.storage.export_size()
    }

    pub(crate) fn export_modifier(&self) -> u64 {
        self.storage.export_modifier()
    }
}

// ────────────────────────────────────────────────────────────────
// Errors + retirement decision
// ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) enum AllocError {
    /// Two allocations collided on the same xid; caller is
    /// responsible for picking a fresh one (`KmsCore::next_host_xid`).
    XidInUse,
    /// Caller passed an unsupported `(depth, format)` combo.
    UnsupportedFormat,
    /// VkContext / pool allocation failure.
    Vk(vk::Result),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetireDecision {
    /// Refcount > 0 after decref; storage stays.
    StillReferenced,
    /// Refcount hit zero AND no fence ticket attached (or it
    /// was already signaled). Vk handles destroyed; entry
    /// removed from the map.
    Destroyed,
    /// Refcount hit zero but a fence ticket is still
    /// unsignaled. Entry parked in `pending_retire`; future
    /// `poll_pending_retire` calls will sweep it once the
    /// fence fires.
    PendingFence,
}

// ────────────────────────────────────────────────────────────────
// DrawableStore — the map + accessors.
// ────────────────────────────────────────────────────────────────

pub(crate) struct DrawableStore {
    next_id: u64,
    entries: HashMap<DrawableId, Drawable>,
    by_xid: HashMap<u32, DrawableId>,
    /// Drawables that hit refcount-zero but whose ticket isn't
    /// signaled yet. `poll_pending_retire` drains.
    pending_retire: Vec<DrawableId>,
    /// GLX-TFP (Task 2.3): dma-buf fds for drawables that have been
    /// exported to a GL consumer, indexed by `DrawableId`. The backend
    /// (`KmsBackend::exported_dmabufs`) owns the canonical lifetime
    /// record; this map is a parallel sync-only dup the backend keeps in
    /// lockstep so the engine's flush chokepoint (which sees `store` +
    /// `platform` but NOT the backend) can resolve fds for bidirectional
    /// implicit sync. `Arc` so the engine can cheaply collect borrows
    /// across the `&mut self` flush.
    exported_sync: HashMap<DrawableId, Arc<OwnedFd>>,
    /// GLX-TFP (Task 2.3): exported `DrawableId`s written since the last
    /// flush. `touch_render_fence` pushes here when the stamped id is in
    /// `exported_sync`; `take_exported_writes` drains it at flush.
    exported_writes: Vec<DrawableId>,
    /// Exported destination whose external-reader fence was already awaited
    /// by the deferred Present gate. Consumed by the next flush containing a
    /// write to that drawable; the flush still publishes its new WRITE fence.
    prewaited_exported_writes: std::collections::HashSet<DrawableId>,
}

impl DrawableStore {
    pub(crate) fn new() -> Self {
        Self {
            next_id: 1,
            entries: HashMap::new(),
            by_xid: HashMap::new(),
            pending_retire: Vec::new(),
            exported_sync: HashMap::new(),
            exported_writes: Vec::new(),
            prewaited_exported_writes: std::collections::HashSet::new(),
        }
    }

    /// GLX-TFP (Task 2.3): register/replace the sync-only dma-buf fd dup
    /// for an exported drawable. Called by the backend when an
    /// `ExportedBacking` first gains its fd.
    pub(crate) fn set_exported_sync_fd(&mut self, id: DrawableId, fd: Arc<OwnedFd>) {
        self.exported_sync.insert(id, fd);
    }

    /// GLX-TFP (Task 2.3): drop the sync-only fd dup for `id`. Called by
    /// the backend at export teardown. Also purges any pending
    /// `exported_writes` entry so a torn-down export can't be waited on.
    pub(crate) fn clear_exported_sync_fd(&mut self, id: DrawableId) {
        self.exported_sync.remove(&id);
        self.exported_writes.retain(|&w| w != id);
        self.prewaited_exported_writes.remove(&id);
    }

    /// GLX-TFP (Task 2.3): true iff `id` currently has a sync-only dma-buf
    /// fd registered (i.e. is live-exported to a GL consumer).
    pub(crate) fn is_exported(&self, id: DrawableId) -> bool {
        self.exported_sync.contains_key(&id)
    }

    pub(crate) fn exported_sync_fd(&self, id: DrawableId) -> Option<Arc<OwnedFd>> {
        self.exported_sync.get(&id).cloned()
    }

    /// GLX-TFP (Task 2.3): drain the exported-writes accumulator and
    /// resolve each id to its sync fd `Arc`, deduped. Returned at the
    /// flush chokepoint so the platform can wait/publish around the
    /// `vkQueueSubmit2`. Entries whose fd was cleared between stamp and
    /// flush are skipped.
    pub(crate) fn take_exported_writes(&mut self) -> Vec<(Arc<OwnedFd>, bool)> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for id in self.exported_writes.drain(..) {
            if seen.insert(id)
                && let Some(fd) = self.exported_sync.get(&id)
            {
                out.push((Arc::clone(fd), self.prewaited_exported_writes.remove(&id)));
            }
        }
        out
    }

    /// Authorize one exported write to bypass the queue-level old-reader
    /// wait. Refuse when an earlier write to the same backing is already
    /// pending in the current submit group: that older write was not covered
    /// by the Present gate's fence snapshot.
    pub(crate) fn begin_prewaited_exported_write(&mut self, id: DrawableId) {
        if self.exported_sync.contains_key(&id) && !self.exported_writes.contains(&id) {
            self.prewaited_exported_writes.insert(id);
        }
    }

    /// Revoke an unused authorization (for example, a fully clipped Present).
    pub(crate) fn end_prewaited_exported_write(&mut self, id: DrawableId) {
        if !self.exported_writes.contains(&id) {
            self.prewaited_exported_writes.remove(&id);
        }
    }

    /// Allocate a fresh drawable. The caller has already built
    /// the `Storage` (via PlatformBackend in production, or
    /// `Storage::for_tests_null` in unit tests). Refcount
    /// starts at 1; layout is `UNDEFINED`; both damage lists
    /// empty.
    ///
    /// # Errors
    ///
    /// - `XidInUse` if `xid` already maps to a drawable.
    pub(crate) fn allocate(
        &mut self,
        xid: u32,
        kind: DrawableKind,
        depth: u8,
        scene_participating: bool,
        storage: Storage,
    ) -> Result<DrawableId, AllocError> {
        if self.by_xid.contains_key(&xid) {
            return Err(AllocError::XidInUse);
        }
        let id = DrawableId(self.next_id);
        self.next_id = self.next_id.checked_add(1).expect("DrawableId overflow");
        let drawable = Drawable {
            id,
            xid,
            kind,
            depth,
            refcount: 1,
            scene_participating,
            storage,
            last_render_ticket: None,
            presentation_damage: RegionSet::new(),
            presentation_damage_epoch: 0,
            dormant: None,
            content_version: 0,
            content_offset: 0,
            redirected_target: None,
        };
        self.entries.insert(id, drawable);
        self.by_xid.insert(xid, id);
        Ok(id)
    }

    pub(crate) fn lookup(&self, xid: u32) -> Option<DrawableId> {
        self.by_xid.get(&xid).copied()
    }

    /// #133 step 3 — record the content offset this storage was
    /// allocated with. Called right after allocating a window's storage
    /// or a redirect backing, with the border width in force at that
    /// moment; see [`Drawable::content_offset`].
    pub(crate) fn set_content_offset(&mut self, id: DrawableId, offset: i32) {
        if let Some(d) = self.entries.get_mut(&id) {
            d.content_offset = offset;
        }
    }

    /// Diagnostic-only: iterate every `(host_xid, DrawableId)` pair
    /// currently registered. Used by the Ctrl-Alt-F12 drawables dump to
    /// sweep pixmaps that aren't reachable through `windows` /
    /// backings (e.g. e16's menu-item background pixmaps).
    pub(crate) fn xid_entries(&self) -> impl Iterator<Item = (u32, DrawableId)> + '_ {
        self.by_xid.iter().map(|(&xid, &id)| (xid, id))
    }

    /// Shutdown-only: destroy every remaining drawable's Vk
    /// storage. The runtime release path (`destroy_now`) walks
    /// from `decref` → `poll_pending_retire`, so any drawable
    /// still in `entries` at shutdown was alive at SIGTERM (e.g.
    /// MATE's resident pixmaps when the session quit). Without
    /// this call the entries are dropped silently and their
    /// `VkImage` / `VkImageView` / `VkDeviceMemory` handles leak
    /// to `vkDestroyDevice`'s `has N leaked objects` warning
    /// (948 VkDeviceMemory observed on bee/MATE 2026-05-31 after
    /// the FenceTicket fix). Idempotent: `Storage::destroy`
    /// nulls handles after pool-return and the direct-destroy
    /// path is null-guarded throughout, so re-calling is safe.
    /// Caller is `KmsBackend::shutdown_destroy_drawables` from
    /// `lib.rs`'s explicit shutdown block.
    pub(crate) fn shutdown_destroy_all(&mut self, platform: &PlatformBackend) {
        self.by_xid.clear();
        self.pending_retire.clear();
        for (_, mut drawable) in self.entries.drain() {
            drawable.storage.destroy(platform);
        }
    }

    pub(crate) fn get(&self, id: DrawableId) -> Option<&Drawable> {
        self.entries.get(&id)
    }

    pub(crate) fn get_mut(&mut self, id: DrawableId) -> Option<&mut Drawable> {
        self.entries.get_mut(&id)
    }

    pub(crate) fn get_by_xid(&self, xid: u32) -> Option<&Drawable> {
        self.lookup(xid).and_then(|id| self.get(id))
    }

    pub(crate) fn get_by_xid_mut(&mut self, xid: u32) -> Option<&mut Drawable> {
        let id = self.lookup(xid)?;
        self.entries.get_mut(&id)
    }

    pub(crate) fn incref(&mut self, id: DrawableId) {
        if let Some(d) = self.entries.get_mut(&id) {
            d.refcount = d.refcount.saturating_add(1);
        }
    }

    /// Detach the xid → DrawableId mapping for `xid`, without
    /// touching the drawable's refcount. The drawable stays alive
    /// in `entries` for any holders that captured the id (Pictures,
    /// in-flight compose ops). Used by `configure_subwindow`'s
    /// resize path: the window's storage is being replaced with a
    /// fresh allocation, so the xid map needs to retarget, but
    /// existing Picture refcounts on the old storage must not be
    /// dropped (the picture's next `store.lookup(xid)` will return
    /// the new id — which is what the caller installs next).
    ///
    /// Idempotent: missing mappings are silently ignored.
    pub(crate) fn detach_xid(&mut self, xid: u32) {
        self.by_xid.remove(&xid);
    }

    /// Drop one reference. If refcount hits zero, decide
    /// retirement: synchronous-destroy if no fence is
    /// pending; otherwise park in `pending_retire`.
    ///
    /// **xid detachment on PendingFence**: when the storage parks
    /// because of an in-flight GPU ticket, the `by_xid` mapping is
    /// removed immediately. The drawable stays alive in `entries`
    /// (and `pending_retire`) until the ticket signals, but the
    /// xid is now free for re-allocation — needed by, e.g.,
    /// `configure_subwindow`'s resize path which calls
    /// `decref` then `allocate(same_xid, …)`. Without this
    /// detachment the re-allocate would fail with `XidInUse` and
    /// the caller would silently keep the old (now-orphaned) storage.
    /// Id-based access stays valid (any in-flight op captured the id
    /// before this point), so this only affects xid-based lookups.
    pub(crate) fn decref<F>(
        &mut self,
        platform: &mut PlatformBackend,
        id: DrawableId,
        on_destroyed: F,
    ) -> RetireDecision
    where
        F: FnOnce(DrawableId),
    {
        let Some(drawable) = self.entries.get_mut(&id) else {
            return RetireDecision::Destroyed;
        };
        if drawable.refcount > 1 {
            drawable.refcount -= 1;
            return RetireDecision::StillReferenced;
        }
        drawable.refcount = 0;
        let ticket_ready = match drawable.last_render_ticket.as_ref() {
            None => true,
            Some(t) => match platform.vk.as_ref() {
                Some(vk) => t.poll_signaled(vk),
                None => true, // no Vk (tests) — treat as signaled
            },
        };
        if ticket_ready {
            // Engine-cache invalidation must fire BEFORE
            // `destroy_now` so cached `VkImageView`s are
            // destroyed while their underlying `VkImage` is still
            // alive (the Vulkan-spec-clean ordering — view
            // destruction is technically independent of the image
            // but VUID-best-practice + most validation layers
            // expect view-first). Caller threads the closure
            // through to bridge `&mut DrawableStore` with
            // `&mut RenderEngine` (disjoint sibling fields on
            // `KmsBackend`).
            on_destroyed(id);
            self.destroy_now(platform, id);
            RetireDecision::Destroyed
        } else {
            // Detach from xid map so the xid is free for re-alloc
            // (configure_subwindow resize). entries[id] persists for
            // pending_retire poll.
            //
            // Only when the mapping still points at THIS drawable —
            // the same guard `destroy_now` carries, and for the same
            // reason. #133 step 6 (P8) retains the old storage ACROSS
            // the re-allocate so a border-width change can copy the
            // content forward, so by the time this decref runs the xid
            // already resolves to the NEW drawable; a blanket remove
            // here orphaned the window (`storage_extent_for_tests`
            // came back `None` and nothing sampled it again).
            let xid = drawable.xid;
            if self.by_xid.get(&xid).copied() == Some(id) {
                self.by_xid.remove(&xid);
            }
            self.pending_retire.push(id);
            RetireDecision::PendingFence
        }
    }

    /// Internal: destroy storage and remove from maps.
    ///
    /// Only removes the `by_xid[xid]` mapping if it currently
    /// points to **this** DrawableId. Necessary because
    /// `decref → PendingFence` already detaches the xid map and
    /// the same xid may have been re-allocated (e.g.
    /// `configure_subwindow`'s resize: decref → alloc with same
    /// xid → new DrawableId installed). When the parked old
    /// drawable's fence eventually signals and destroy_now runs,
    /// a blanket `by_xid.remove(xid)` would nuke the NEW
    /// drawable's lookup, "orphaning" the resized window.
    fn destroy_now(&mut self, platform: &mut PlatformBackend, id: DrawableId) {
        let Some(mut drawable) = self.entries.remove(&id) else {
            return;
        };
        if self.by_xid.get(&drawable.xid).copied() == Some(id) {
            self.by_xid.remove(&drawable.xid);
        }
        drawable.storage.destroy(platform);
        // last_render_ticket drops here; its Rc inner refcount
        // ensures the underlying fence handle stays alive until
        // every consumer that cloned it has also dropped.
    }

    /// Flip scene-participation. When set to false, **clears
    /// unpresented presentation damage and bumps the epoch**
    /// (per codex round 1 point 5): an unmap means the scene
    /// shouldn't repaint from this storage; any in-flight
    /// snapshot that ack's against the new epoch will become
    /// a no-op subtract against an empty set. Protocol damage
    /// unaffected.
    pub(crate) fn set_scene_participating(&mut self, id: DrawableId, v: bool) {
        let Some(d) = self.entries.get_mut(&id) else {
            return;
        };
        let was = d.scene_participating;
        d.scene_participating = v;
        if was && !v {
            d.presentation_damage.clear();
            d.presentation_damage_epoch = d.presentation_damage_epoch.checked_add(1).unwrap_or(0);
        }
    }

    /// Accumulate presentation damage. No-op on non-scene-
    /// participating drawables (pixmaps, Manual-redirected
    /// backings). Bumps `presentation_damage_epoch` whenever
    /// damage was actually appended.
    ///
    /// Protocol-side `DamageNotify` fanout is handled by
    /// `yserver-core::core_loop::damage_fanout` at the request
    /// layer (see spec §I5 amendment), independent of this
    /// store.
    pub(crate) fn damage(&mut self, id: DrawableId, rect: vk::Rect2D) {
        let Some(d) = self.entries.get_mut(&id) else {
            return;
        };
        if d.scene_participating {
            d.presentation_damage.add(rect);
            d.presentation_damage_epoch = d.presentation_damage_epoch.checked_add(1).unwrap_or(0);
            // A window whose last paint was under a cover may be painting its
            // visible part now: re-arm. A window nothing of which is drawn
            // cannot become visible by painting, so it stays dormant.
            if d.dormant == Some(DormantReason::HiddenDamage) {
                d.dormant = None;
            }
        }
    }

    /// True if any scene-participating drawable has undrained
    /// presentation damage — i.e. a client painted a visible window
    /// and the SceneCompositor has not composed it yet. `next_wakeup`
    /// / `maybe_composite` consult this so that *content* paint arms a
    /// compose, not only structural map/unmap/restack changes
    /// (`scene_structure_dirty`). Without it, a window painted after
    /// its map-compose already ran is stranded until an unrelated
    /// event pokes the loop (the xfce submenu bug). Cheap: short-
    /// circuits on the first match, no allocation.
    pub(crate) fn has_pending_presentation_damage(&self) -> bool {
        self.entries.values().any(|d| {
            d.scene_participating && !d.presentation_damage.is_empty() && d.dormant.is_none()
        })
    }

    /// The drawables [`Self::has_pending_presentation_damage`] counts, by id —
    /// armed, scene-participating, with damage waiting. The tick's pre-walk
    /// predicate tests these against each output's retained set of drawables
    /// that emitted pieces there, so an output the damaged window is not on
    /// need not walk. Small in practice: at most the windows that painted since
    /// the last compose.
    pub(crate) fn armed_damaged_ids(&self) -> Vec<DrawableId> {
        self.entries
            .iter()
            .filter(|(_, d)| {
                d.scene_participating && !d.presentation_damage.is_empty() && d.dormant.is_none()
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Reconcile dormancy from what the walk did on ALL outputs this tick.
    ///
    /// `presented`: sampled sources whose pending damage reached the screen on
    /// some output (projected `Visible`), was off-output (forces a compose that
    /// acks it), or had no damage. `had_pieces`: sources that emitted at least
    /// one piece on some output. A scene-participating drawable with undrained
    /// damage that is in neither set is `NoPieces`; one that emitted pieces but
    /// presented none of its damage is `HiddenDamage` — see [`DormantReason`]
    /// for the two re-arm rules. The damage itself is PRESERVED. MUST be called
    /// only when every output walked (`build_scene` ran), so a drawable visible
    /// only on a mid-flip output isn't mis-flagged. Ids are *sampled source*
    /// ids (a redirected Automatic window is sampled via its backing).
    ///
    /// 2026-09-04: `presented` is really "presented by a walked output, OR
    /// possibly presentable by an output that did not walk this tick" and
    /// `had_pieces` is "has pieces on some output, retained" — the tick builds
    /// both with `scene::dormancy_inputs`, so this runs on any tick where at
    /// least one output walked, not only when all did.
    /// Returns the drawables whose dormancy CHANGED, so the caller can log the
    /// transitions. A stranded window — damage pending, dormancy wrong — shows
    /// on screen as content that only heals where something else happens to
    /// compose, and the reason (`NoPieces` never re-arms on paint,
    /// `HiddenDamage` does) is what separates a correct verdict from a bug.
    pub(crate) fn reconcile_offscreen_no_draw(
        &mut self,
        presented: &HashSet<DrawableId>,
        had_pieces: &HashSet<DrawableId>,
    ) -> Vec<(DrawableId, Option<DormantReason>)> {
        let mut changed = Vec::new();
        for (id, d) in &mut self.entries {
            let before = d.dormant;
            if !d.scene_participating || d.presentation_damage.is_empty() || presented.contains(id)
            {
                d.dormant = None;
            } else if had_pieces.contains(id) {
                d.dormant = Some(DormantReason::HiddenDamage);
            } else {
                d.dormant = Some(DormantReason::NoPieces);
            }
            if before != d.dormant {
                changed.push((*id, d.dormant));
            }
        }
        changed
    }

    /// Snapshot for the SceneCompositor to ack later.
    /// Returns `None` if the drawable doesn't exist or isn't
    /// scene-participating (no presentation damage to peek).
    pub(crate) fn peek_presentation_damage(&self, id: DrawableId) -> Option<DamageSnapshot> {
        let d = self.entries.get(&id)?;
        if !d.scene_participating {
            return None;
        }
        Some(DamageSnapshot {
            id,
            epoch: d.presentation_damage_epoch,
            region: d.presentation_damage.clone(),
        })
    }

    /// Ack: if `snap.epoch == current_epoch`, clear live
    /// damage. If `snap.epoch < current_epoch`, paint arrived
    /// between peek and ack — subtract only the snapshot's
    /// region so post-peek damage survives.
    ///
    /// Per codex round 1 point 5: if scene_participating
    /// flipped to false since the snapshot, the live damage
    /// is already empty and the subtract is a no-op.
    pub(crate) fn ack_presentation_damage(&mut self, snap: DamageSnapshot) {
        let Some(d) = self.entries.get_mut(&snap.id) else {
            return;
        };
        if snap.epoch == d.presentation_damage_epoch {
            d.presentation_damage.clear();
        } else {
            d.presentation_damage.subtract(&snap.region);
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot_region_is_pending_for_tests(&self, snap: &DamageSnapshot) -> bool {
        let Some(drawable) = self.entries.get(&snap.id) else {
            return false;
        };
        !snap.region.is_empty()
            && snap
                .region
                .rects()
                .iter()
                .all(|rect| drawable.presentation_damage.rects().contains(rect))
    }

    /// Stage 4a — set or clear a window's COMPOSITE redirect
    /// routing. `Some(backing_id)` routes future paint resolution
    /// against `window_id`'s xid (or any descendant whose nearest
    /// redirected ancestor is this drawable) into `backing_id`;
    /// `None` un-redirects. No side effects on damage / refcount /
    /// `scene_participating` — those flips belong to the protocol
    /// handler in 4c via the dedicated Backend methods.
    pub(crate) fn set_redirected_target(
        &mut self,
        window_id: DrawableId,
        backing_id: Option<DrawableId>,
    ) {
        // Diagnostic trace (TEMP — Stage 4d "opaque black backing"
        // investigation). Every redirect-route mutation matters
        // because clearing the route makes future paints land on
        // W's own storage instead of B. Volume is bounded by the
        // small number of redirected windows per session, so the
        // trace is left at log::trace! and gated by the
        // `yserver::kms::render::store` target.
        if log::log_enabled!(target: "yserver::kms::render::store", log::Level::Trace) {
            let old = self
                .entries
                .get(&window_id)
                .and_then(|d| d.redirected_target);
            log::trace!(
                target: "yserver::kms::render::store",
                "set_redirected_target window={window_id:?} old={old:?} new={backing_id:?}",
            );
        }
        if let Some(d) = self.entries.get_mut(&window_id) {
            d.redirected_target = backing_id;
        }
    }

    /// Stage 4a — leaf accessor returning the per-drawable
    /// `redirected_target`. Returns `None` if the drawable
    /// doesn't exist or isn't redirected. The full ancestor walk
    /// lives on `KmsBackend::resolve_paint_target` because it
    /// needs window-geometry metadata (`windows`) that isn't
    /// in the store.
    pub(crate) fn redirected_target(&self, id: DrawableId) -> Option<DrawableId> {
        self.entries.get(&id)?.redirected_target
    }

    /// True when some live window drawable currently routes paint into
    /// `candidate` via `set_redirected_target(..., Some(candidate))`.
    /// This captures real redirected-backing identity even when the
    /// backing was allocated through the generic pixmap path.
    pub(crate) fn is_active_redirect_target(&self, candidate: DrawableId) -> bool {
        self.entries
            .values()
            .any(|d| d.redirected_target == Some(candidate))
    }

    /// Record an in-flight ticket. Coalesces: replaces any
    /// prior ticket. The Rc inner stays alive in other
    /// holders (e.g. SceneCompositor's pending-ack list) per
    /// cross-cutting §5.
    pub(crate) fn touch_render_fence(&mut self, id: DrawableId, ticket: FenceTicket) {
        if let Some(d) = self.entries.get_mut(&id) {
            d.last_render_ticket = Some(ticket);
        }
        // GLX-TFP (Task 2.3): every write stamps its destination here, so
        // this is the single point that catches ALL mutation paths
        // (fills/clears/copies/composite/uploads). Record exported
        // destinations for the bidirectional implicit-sync wait/publish
        // at the flush chokepoint.
        if self.exported_sync.contains_key(&id) {
            self.exported_writes.push(id);
        }
    }

    /// Sweep `pending_retire`. Drawables whose ticket has
    /// signaled (or never had one) get destroyed and removed.
    /// `on_destroyed` is invoked per drawable BEFORE its storage
    /// is destroyed, so engine-side caches keyed on `DrawableId`
    /// (e.g. `RenderEngine::drawable_view_cache`) can drop their
    /// `VkImageView`s while the underlying `VkImage` is still
    /// alive.
    pub(crate) fn poll_pending_retire<F>(
        &mut self,
        platform: &mut PlatformBackend,
        mut on_destroyed: F,
    ) where
        F: FnMut(DrawableId),
    {
        let mut survivors = Vec::with_capacity(self.pending_retire.len());
        let mut to_destroy = Vec::new();
        for id in std::mem::take(&mut self.pending_retire) {
            let ready = match self.entries.get(&id) {
                None => true, // already gone
                Some(d) => match d.last_render_ticket.as_ref() {
                    None => true,
                    Some(t) => match platform.vk.as_ref() {
                        Some(vk) => t.poll_signaled(vk),
                        None => true,
                    },
                },
            };
            if ready {
                to_destroy.push(id);
            } else {
                survivors.push(id);
            }
        }
        for id in to_destroy {
            on_destroyed(id);
            self.destroy_now(platform, id);
        }
        self.pending_retire = survivors;
    }

    /// Number of live entries (test introspection).
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Number of entries pending retirement.
    pub(crate) fn pending_retire_count(&self) -> usize {
        self.pending_retire.len()
    }

    /// Bump a drawable's `content_version` (saturating). Call on every
    /// successful pixel write. No-op for an unknown id.
    pub(crate) fn mark_contents_modified(&mut self, id: DrawableId) {
        if let Some(d) = self.get_mut(id) {
            d.content_version = d.content_version.saturating_add(1);
        }
    }

    /// Stage-1b-era compatibility constructor (was `stub()`).
    /// Kept so existing callers in `kms::render::backend` continue
    /// to compile until they're updated to call `new()`.
    pub(crate) fn stub() -> Self {
        Self::new()
    }
}

// ────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn stub_storage() -> Storage {
        Storage::for_tests_null(
            vk::Extent2D {
                width: 16,
                height: 16,
            },
            vk::Format::B8G8R8A8_UNORM,
        )
    }

    fn rect(x: i32, y: i32, w: u32, h: u32) -> vk::Rect2D {
        vk::Rect2D {
            offset: vk::Offset2D { x, y },
            extent: vk::Extent2D {
                width: w,
                height: h,
            },
        }
    }

    #[test]
    fn region_set_add_stays_bounded_and_covers_all_damage() {
        // Regression (wmaker restack/draw storm → machine lock-up, no ZAP):
        // presentation damage accumulated unbounded, and O(n·m)
        // RegionSet::subtract in the page-flip handler then wedged the
        // single-threaded core loop. `add` must keep the set bounded
        // (coalescing to the bounding rect past a cap) while never losing
        // damage — the bounding rect must still cover everything added.
        let mut rs = RegionSet::new();
        for i in 0..10_000i32 {
            rs.add(rect(i, 0, 1, 1));
        }
        assert!(
            rs.rects().len() <= 256,
            "RegionSet must stay bounded under heavy add, got {}",
            rs.rects().len(),
        );
        let b = rs.bounding_rect().expect("non-empty");
        assert_eq!(b.offset.x, 0, "bounding rect starts at first damage");
        assert!(
            b.offset.x.saturating_add_unsigned(b.extent.width) >= 10_000,
            "bounding rect must still cover ALL added damage (no lost repaint)",
        );
    }

    #[test]
    fn region_set_subtract_still_removes_exact_matches_under_cap() {
        // Guard: the common small-set path (exact-match removal) is intact.
        let mut a = RegionSet::new();
        a.add(rect(0, 0, 10, 10));
        a.add(rect(20, 0, 10, 10));
        let mut b = RegionSet::new();
        b.add(rect(0, 0, 10, 10));
        a.subtract(&b);
        assert_eq!(a.rects().len(), 1);
        assert_eq!(a.rects()[0], rect(20, 0, 10, 10));
    }

    #[test]
    fn region_set_subtract_preserves_damage_added_after_snapshot() {
        let mut live = RegionSet::new();
        let full = rect(0, 0, 1920, 1080);
        live.add(full);
        let submitted = live.snapshot();
        // A second Configure/Map arrives while the first flip is pending.
        live.add(full);

        live.subtract(&submitted);

        assert_eq!(live.rects(), &[full]);
    }

    #[test]
    fn allocate_and_lookup() {
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1234, DrawableKind::Pixmap, 32, false, stub_storage())
            .expect("allocate");
        assert_eq!(s.lookup(0x1234), Some(id));
        let d = s.get(id).expect("get");
        assert_eq!(d.xid, 0x1234);
        assert_eq!(d.kind, DrawableKind::Pixmap);
        assert_eq!(d.depth, 32);
        assert_eq!(d.refcount, 1);
        assert!(!d.scene_participating);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn prewait_authorization_is_one_shot_and_rejects_earlier_writes() {
        use nix::sys::eventfd::{EfdFlags, EventFd};

        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1234, DrawableKind::Pixmap, 32, false, stub_storage())
            .expect("allocate");
        let fd: OwnedFd =
            EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
                .expect("eventfd")
                .into();
        s.set_exported_sync_fd(id, Arc::new(fd));

        s.begin_prewaited_exported_write(id);
        s.exported_writes.push(id);
        s.end_prewaited_exported_write(id);
        let first = s.take_exported_writes();
        assert_eq!(first.len(), 1);
        assert!(first[0].1, "authorization survives until the write flush");

        s.exported_writes.push(id);
        s.begin_prewaited_exported_write(id);
        let second = s.take_exported_writes();
        assert_eq!(second.len(), 1);
        assert!(!second[0].1, "an earlier pending write cannot be covered");
    }

    #[test]
    fn allocate_rejects_xid_collision() {
        let mut s = DrawableStore::new();
        s.allocate(0x1, DrawableKind::Pixmap, 32, false, stub_storage())
            .expect("first");
        let err = s
            .allocate(0x1, DrawableKind::Pixmap, 32, false, stub_storage())
            .expect_err("collision");
        assert!(matches!(err, AllocError::XidInUse));
    }

    #[test]
    fn decref_destroys_immediately_when_no_ticket() {
        let mut s = DrawableStore::new();
        let mut platform = PlatformBackend::for_tests();
        let id = s
            .allocate(0x1, DrawableKind::Pixmap, 32, false, stub_storage())
            .unwrap();
        assert_eq!(
            s.decref(&mut platform, id, |_| {}),
            RetireDecision::Destroyed
        );
        assert!(s.lookup(0x1).is_none());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn incref_then_decref_keeps_alive() {
        let mut s = DrawableStore::new();
        let mut platform = PlatformBackend::for_tests();
        let id = s
            .allocate(0x1, DrawableKind::Pixmap, 32, false, stub_storage())
            .unwrap();
        s.incref(id);
        assert_eq!(
            s.decref(&mut platform, id, |_| {}),
            RetireDecision::StillReferenced
        );
        assert!(s.lookup(0x1).is_some());
        assert_eq!(
            s.decref(&mut platform, id, |_| {}),
            RetireDecision::Destroyed
        );
        assert!(s.lookup(0x1).is_none());
    }

    #[test]
    fn damage_pixmap_is_no_op() {
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1, DrawableKind::Pixmap, 32, false, stub_storage())
            .unwrap();
        s.damage(id, rect(0, 0, 4, 4));
        let d = s.get(id).unwrap();
        assert!(d.presentation_damage.is_empty());
        assert_eq!(d.presentation_damage_epoch, 0);
    }

    #[test]
    fn damage_window_accumulates_presentation_and_bumps_epoch() {
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        s.damage(id, rect(0, 0, 4, 4));
        let d = s.get(id).unwrap();
        assert_eq!(d.presentation_damage.rects().len(), 1);
        assert_eq!(d.presentation_damage_epoch, 1);
        s.damage(id, rect(8, 8, 2, 2));
        assert_eq!(s.get(id).unwrap().presentation_damage_epoch, 2);
    }

    #[test]
    fn peek_and_ack_clears_when_epoch_matches() {
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        s.damage(id, rect(0, 0, 4, 4));
        let snap = s.peek_presentation_damage(id).expect("snap");
        assert_eq!(snap.epoch, 1);
        s.ack_presentation_damage(snap);
        assert!(s.get(id).unwrap().presentation_damage.is_empty());
    }

    #[test]
    fn paint_between_peek_and_ack_survives() {
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        s.damage(id, rect(0, 0, 4, 4));
        let snap = s.peek_presentation_damage(id).unwrap();
        // Paint arrives between peek and ack.
        s.damage(id, rect(8, 8, 2, 2));
        s.ack_presentation_damage(snap);
        // The post-peek paint survives.
        let live = &s.get(id).unwrap().presentation_damage;
        assert_eq!(live.rects().len(), 1);
        assert_eq!(live.rects()[0].offset, vk::Offset2D { x: 8, y: 8 });
    }

    #[test]
    fn set_scene_participating_false_clears_unpresented_damage() {
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        s.damage(id, rect(0, 0, 4, 4));
        assert_eq!(s.get(id).unwrap().presentation_damage.rects().len(), 1);
        let epoch_before = s.get(id).unwrap().presentation_damage_epoch;
        s.set_scene_participating(id, false);
        let d = s.get(id).unwrap();
        assert!(d.presentation_damage.is_empty());
        assert!(d.presentation_damage_epoch > epoch_before);
    }

    /// Idle free-run fix (cut 2b): a scene-participating window with
    /// damage that `build_scene` did NOT draw (not in the `drawn` set)
    /// is flagged out of the compose scheduler — but its damage is
    /// PRESERVED, and being drawn again re-includes it.
    #[test]
    fn reconcile_offscreen_no_draw_gates_scheduler_but_preserves_damage() {
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        s.damage(id, rect(0, 0, 4, 4));
        assert!(
            s.has_pending_presentation_damage(),
            "fresh damage on a scene window arms the scheduler",
        );

        // Not drawn on any output this tick → flag off-screen.
        s.reconcile_offscreen_no_draw(&HashSet::new(), &HashSet::new());
        assert!(
            !s.has_pending_presentation_damage(),
            "un-drawable off-screen damage must not arm the scheduler (idle spin)",
        );
        assert_eq!(s.get(id).unwrap().dormant, Some(DormantReason::NoPieces));
        assert_eq!(
            s.get(id).unwrap().presentation_damage.rects().len(),
            1,
            "damage must be PRESERVED, not cleared",
        );

        // Drawn this tick (e.g. it came on-screen) → flag cleared →
        // counted again so its damage composes + drains.
        let drawn: HashSet<DrawableId> = std::iter::once(id).collect();
        s.reconcile_offscreen_no_draw(&drawn, &drawn);
        assert!(
            s.has_pending_presentation_damage(),
            "a drawn window's damage re-arms the scheduler",
        );
    }

    /// Two drawables with pending damage, only one presented: the other is
    /// flagged, and the scheduler stays armed for the presented one alone.
    #[test]
    fn reconcile_flags_only_the_drawables_left_out_of_the_presented_set() {
        let mut s = DrawableStore::new();
        let shown = s
            .allocate(0x1, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        let hidden = s
            .allocate(0x2, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        s.damage(shown, rect(0, 0, 4, 4));
        s.damage(hidden, rect(0, 0, 4, 4));
        let presented: HashSet<DrawableId> = std::iter::once(shown).collect();
        s.reconcile_offscreen_no_draw(&presented, &presented);
        assert!(s.get(shown).unwrap().dormant.is_none());
        assert_eq!(
            s.get(hidden).unwrap().dormant,
            Some(DormantReason::NoPieces)
        );
        assert!(
            s.has_pending_presentation_damage(),
            "the presented drawable still arms the scheduler"
        );
        s.reconcile_offscreen_no_draw(&HashSet::new(), &HashSet::new());
        assert!(!s.has_pending_presentation_damage());
        assert_eq!(s.get(hidden).unwrap().presentation_damage.rects().len(), 1);
    }

    /// Coordinator review of fix 1 (2026-09-04): a partially covered window
    /// whose paint was hidden once must re-arm on its NEXT paint, which may
    /// land in the visible part. A fully un-drawable window must not.
    #[test]
    fn a_paint_rearms_hidden_damage_dormancy_but_not_no_pieces() {
        let mut s = DrawableStore::new();
        let half = s
            .allocate(0x1, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        let gone = s
            .allocate(0x2, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        s.damage(half, rect(0, 0, 4, 4));
        s.damage(gone, rect(0, 0, 4, 4));
        // `half` emitted pieces but presented none of its damage; `gone`
        // emitted nothing at all.
        let pieces: HashSet<DrawableId> = std::iter::once(half).collect();
        s.reconcile_offscreen_no_draw(&HashSet::new(), &pieces);
        assert_eq!(
            s.get(half).unwrap().dormant,
            Some(DormantReason::HiddenDamage)
        );
        assert_eq!(s.get(gone).unwrap().dormant, Some(DormantReason::NoPieces));
        assert!(!s.has_pending_presentation_damage());
        // A new paint into the hidden-damage window re-arms it …
        s.damage(half, rect(1, 1, 2, 2));
        assert!(s.get(half).unwrap().dormant.is_none());
        assert!(
            s.has_pending_presentation_damage(),
            "the next paint may be visible: it must wake the tick"
        );
        // … and a new paint into the no-pieces window does not.
        s.reconcile_offscreen_no_draw(&HashSet::new(), &HashSet::new());
        assert!(!s.has_pending_presentation_damage());
        s.damage(gone, rect(1, 1, 2, 2));
        assert_eq!(s.get(gone).unwrap().dormant, Some(DormantReason::NoPieces));
        assert!(
            !s.has_pending_presentation_damage(),
            "nothing of it can appear without a structural change; stay dormant"
        );
    }

    #[test]
    fn ack_after_unmap_is_noop_against_empty_live_damage() {
        // Codex round 1 point 5: peek; unmap; ack against the
        // stale snapshot. Live damage is already empty; the
        // subtract-by-snapshot is a no-op. No corruption.
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        s.damage(id, rect(0, 0, 4, 4));
        let snap = s.peek_presentation_damage(id).unwrap();
        s.set_scene_participating(id, false);
        s.ack_presentation_damage(snap);
        let d = s.get(id).unwrap();
        assert!(d.presentation_damage.is_empty());
        assert!(!d.scene_participating);
    }

    #[test]
    fn poll_pending_retire_with_no_ticket_destroys() {
        let mut s = DrawableStore::new();
        let mut platform = PlatformBackend::for_tests();
        let id = s
            .allocate(0x1, DrawableKind::Pixmap, 32, false, stub_storage())
            .unwrap();
        // Force into pending_retire by simulating a never-
        // signaled ticket (here we just push id manually since
        // we can't easily construct a FenceTicket in tests).
        s.entries.get_mut(&id).unwrap().refcount = 0;
        s.pending_retire.push(id);
        s.poll_pending_retire(&mut platform, |_| {});
        // No ticket attached → treated as signaled → destroyed.
        assert!(s.lookup(0x1).is_none());
        assert_eq!(s.pending_retire_count(), 0);
    }

    /// xeyes resize bug: decref → PendingFence + re-allocate the
    /// same xid + later destroy_now of the old drawable MUST NOT
    /// remove `by_xid[xid]` (which now maps to the NEW drawable).
    /// Pre-fix: blanket `by_xid.remove(drawable.xid)` in destroy_now
    /// orphaned the new storage when the old fence eventually
    /// signaled.
    #[test]
    fn decref_then_realloc_then_retire_keeps_new_xid_mapping() {
        c0_2ci_storage_decref_then_realloc_then_retire_keeps_new_xid_mapping();
    }

    #[test]
    fn c0_2ci_storage_decref_then_realloc_then_retire_keeps_new_xid_mapping() {
        let mut s = DrawableStore::new();
        let mut platform = PlatformBackend::for_tests();
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);

        // Allocate old; force into pending_retire (simulates an
        // unsignaled ticket). We can't construct a real FenceTicket
        // in the test fixture, so we manually push.
        let old_id = s
            .allocate(0x42, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();

        // Convert old drawable storage to managed and retain a lease before reallocation
        let old_target = PaintTarget::new(old_id, (10, 20), None, 24);
        let old_storage = s.entries.get_mut(&old_id).unwrap().storage.backing_mut();
        let old_legacy = match std::mem::replace(
            old_storage,
            StorageBacking::Legacy(StorageAllocation {
                image: vk::Image::null(),
                memory: vk::DeviceMemory::null(),
                image_view: vk::ImageView::null(),
                sample_view: vk::ImageView::null(),
                extent: vk::Extent2D {
                    width: 100,
                    height: 100,
                },
                format: vk::Format::B8G8R8A8_UNORM,
                depth: 24,
                current_layout: vk::ImageLayout::UNDEFINED,
                is_test_stub: true,
                imported_drawable: None,
                imported_dmabuf: None,
                promoted_exportable: false,
                export_stride: 0,
                export_size: 0,
                export_modifier: 0,
                vk: None,
                pixmap_pool: None,
            }),
        ) {
            StorageBacking::Legacy(alloc) => alloc,
            _ => unreachable!(),
        };

        let old_lease = Storage::from_backing(StorageBacking::Legacy(old_legacy))
            .into_managed(&mut service, &platform, old_target, (10, 20))
            .map_err(|(e, _)| e)
            .unwrap();

        // Retain a managed allocation lease from the old drawable before reallocation
        let held_lease = service.retain_storage(&old_lease).unwrap();
        s.entries.get_mut(&old_id).unwrap().storage =
            Storage::from_backing(StorageBacking::Managed(old_lease));

        // Keep its original offset/depth in PixelIdentity
        let held_pixels = &held_lease.pixels;
        assert_eq!(held_pixels.content_offset, (10, 20));
        assert_eq!(held_pixels.target.x11_depth(), 24);
        assert_eq!(held_pixels.allocation, held_lease.allocation.key());

        s.entries.get_mut(&old_id).unwrap().refcount = 0;
        // Mimic decref → PendingFence: park + detach xid.
        s.pending_retire.push(old_id);
        s.by_xid.remove(&0x42);

        // Re-allocate the SAME xid with fresh storage.
        let new_id = s
            .allocate(0x42, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        assert_ne!(old_id, new_id, "store mints a fresh DrawableId");
        assert_eq!(
            s.lookup(0x42),
            Some(new_id),
            "by_xid now points to the new drawable",
        );

        // Add damage and bump content_version on the new drawable
        s.entries.get_mut(&new_id).unwrap().content_version = 77;
        s.damage(new_id, rect(2, 2, 20, 20));

        // Now retire the old drawable. No real ticket attached, so
        // poll_pending_retire treats it as signaled → destroy_now.
        s.poll_pending_retire(&mut platform, |_| {});

        // The new drawable's xid mapping MUST survive.
        assert_eq!(
            s.lookup(0x42),
            Some(new_id),
            "destroy_now of old drawable preserves new xid mapping",
        );
        assert!(
            s.get(new_id).is_some(),
            "new drawable still alive in entries",
        );
        assert!(s.get(old_id).is_none(), "old drawable destroyed");

        // Assert old destruction is delayed: old allocation in service still exists!
        let _ = service.service_ready();
        assert!(
            service.contains(&held_lease.allocation.key()),
            "held_lease keeps old allocation alive in service"
        );

        // Assert old cleanup does not reset the new drawable's content/damage state
        let new_d = s.get(new_id).unwrap();
        assert_eq!(new_d.content_version, 77);
        assert_eq!(new_d.presentation_damage.rects().len(), 1);

        // Drop held lease: now the old allocation is destroyed
        let old_key = held_lease.allocation.key();
        drop(held_lease);
        let _ = service.service_ready();
        assert!(
            !service.contains(&old_key),
            "releasing held_lease allows service to destroy old allocation"
        );
    }

    /// xeyes resize regression with Picture refs: a Picture
    /// wrapping a window increfs the drawable. Pre-fix:
    /// `configure_subwindow`'s `decref(old) → StillReferenced`
    /// kept by_xid mapped to old → `allocate(xid, new)` failed
    /// `XidInUse` → window stayed at old size (heavy visible
    /// artifact). Now: `detach_xid` runs unconditionally,
    /// re-allocate succeeds, old drawable lingers for the
    /// picture's lifetime.
    #[test]
    fn detach_xid_lets_realloc_succeed_even_with_picture_refcount() {
        let mut s = DrawableStore::new();
        let mut platform = PlatformBackend::for_tests();
        let id = s
            .allocate(0x42, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        // Simulate a Picture wrapping the window: bump refcount.
        s.incref(id);
        assert_eq!(s.get(id).unwrap().refcount, 2);
        // Simulate configure_subwindow resize sequence:
        s.detach_xid(0x42);
        let r = s.decref(&mut platform, id, |_| {});
        assert_eq!(
            r,
            RetireDecision::StillReferenced,
            "picture still references the old drawable",
        );
        // Old drawable survives in entries (picture still has it),
        // but its xid mapping is gone.
        assert!(s.get(id).is_some(), "old drawable kept alive by picture");
        assert!(s.lookup(0x42).is_none(), "xid map free for re-alloc");
        // Re-allocate the same xid — pre-fix: XidInUse.
        let new_id = s
            .allocate(0x42, DrawableKind::Window, 24, true, stub_storage())
            .expect("re-alloc must succeed after detach_xid");
        assert_ne!(new_id, id);
        assert_eq!(s.lookup(0x42), Some(new_id));
    }

    /// Stage 4a — `set_redirected_target(Some(B))` makes
    /// `redirected_target(W)` return `Some(B)`. Pure storage-side
    /// state; the full ancestor walk + paint dispatch lives on
    /// `KmsBackend::resolve_paint_target`.
    #[test]
    fn set_redirected_target_stores_backing_id() {
        let mut s = DrawableStore::new();
        let w_id = s
            .allocate(0x100, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        let b_id = s
            .allocate(0x200, DrawableKind::Pixmap, 24, false, stub_storage())
            .unwrap();
        assert_eq!(s.redirected_target(w_id), None);
        s.set_redirected_target(w_id, Some(b_id));
        assert_eq!(s.redirected_target(w_id), Some(b_id));
    }

    /// Stage 4a — `set_redirected_target(None)` clears the route.
    #[test]
    fn set_redirected_target_none_clears_route() {
        let mut s = DrawableStore::new();
        let w_id = s
            .allocate(0x100, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        let b_id = s
            .allocate(0x200, DrawableKind::Pixmap, 24, false, stub_storage())
            .unwrap();
        s.set_redirected_target(w_id, Some(b_id));
        s.set_redirected_target(w_id, None);
        assert_eq!(s.redirected_target(w_id), None);
    }

    /// Stage 4a — flipping `set_redirected_target` does NOT touch
    /// damage / refcount / `scene_participating`. Those flips are
    /// 4c's responsibility via the dedicated Backend methods.
    #[test]
    fn set_redirected_target_has_no_side_effects() {
        let mut s = DrawableStore::new();
        let w_id = s
            .allocate(0x100, DrawableKind::Window, 24, true, stub_storage())
            .unwrap();
        let b_id = s
            .allocate(0x200, DrawableKind::Pixmap, 24, false, stub_storage())
            .unwrap();
        s.damage(w_id, rect(0, 0, 4, 4));
        let epoch_before = s.get(w_id).unwrap().presentation_damage_epoch;
        let refcount_before = s.get(w_id).unwrap().refcount;
        let participating_before = s.get(w_id).unwrap().scene_participating;
        s.set_redirected_target(w_id, Some(b_id));
        let d = s.get(w_id).unwrap();
        assert_eq!(d.presentation_damage.rects().len(), 1);
        assert_eq!(d.presentation_damage_epoch, epoch_before);
        assert_eq!(d.refcount, refcount_before);
        assert_eq!(d.scene_participating, participating_before);
    }

    #[test]
    fn redirected_target_unknown_id_returns_none() {
        let s = DrawableStore::new();
        // Construct a fresh DrawableId that doesn't exist in the store.
        assert_eq!(s.redirected_target(DrawableId(999)), None);
    }

    #[test]
    fn region_set_subtract_exact_match_only() {
        let mut r = RegionSet::new();
        r.add(rect(0, 0, 4, 4));
        r.add(rect(8, 8, 2, 2));
        let mut sub = RegionSet::new();
        sub.add(rect(0, 0, 4, 4));
        r.subtract(&sub);
        assert_eq!(r.rects().len(), 1);
        assert_eq!(r.rects()[0].offset, vk::Offset2D { x: 8, y: 8 });
    }

    #[test]
    fn region_set_zero_extent_ignored() {
        let mut r = RegionSet::new();
        r.add(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: 0,
                height: 4,
            },
        });
        r.add(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D {
                width: 4,
                height: 0,
            },
        });
        assert!(r.is_empty());
    }

    /// Stage 3f.10: `Storage::from_pooled` inherits the pool
    /// entry's tracked layout so the next `record_layout_transition`
    /// issues a correct `old_layout` barrier (not the
    /// `UNDEFINED` that a fresh allocate would imply). Pool entries
    /// also keep the size + format the key was built for.
    #[test]
    fn storage_from_pooled_inherits_layout_and_dims() {
        use crate::kms::vk::pixmap_pool::PooledPixmapImage;
        // Sentinel non-null handles — Storage::from_pooled just
        // copies them; nothing dereferences a real Vk image here.
        let pooled = PooledPixmapImage {
            image: ash::vk::Handle::from_raw(0x1000_0001),
            view: ash::vk::Handle::from_raw(0x1000_0002),
            memory: ash::vk::Handle::from_raw(0x1000_0003),
            current_layout: ash::vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        };
        let extent = ash::vk::Extent2D {
            width: 32,
            height: 64,
        };
        let format = ash::vk::Format::B8G8R8A8_UNORM;
        let sample_view: ash::vk::ImageView = ash::vk::Handle::from_raw(0x1000_0004);
        let s = Storage::from_pooled(pooled, sample_view, extent, format, 32);
        assert_eq!(s.extent.width, 32);
        assert_eq!(s.extent.height, 64);
        assert_eq!(s.format, format);
        assert_eq!(
            s.current_layout,
            ash::vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        );
        assert!(!s.is_test_stub);
    }

    /// `shutdown_destroy_all` on an empty store is a clean no-op:
    /// no entries, no by_xid lookups, no pending retires.
    #[test]
    fn shutdown_destroy_all_on_empty_store_is_noop() {
        let mut s = DrawableStore::new();
        let platform = PlatformBackend::for_tests();
        s.shutdown_destroy_all(&platform);
        assert!(s.entries.is_empty());
        assert!(s.by_xid.is_empty());
        assert!(s.pending_retire.is_empty());
    }

    /// Regression: pre-fix `DrawableStore` had no `Drop`, so
    /// stub drawables still in `entries` at SIGTERM had their
    /// `Storage` dropped silently — production drawables would
    /// leak their real Vk handles. Verify `shutdown_destroy_all`
    /// drains every entry and clears the xid lookup. Stub
    /// storage's `destroy` is a no-op (early-returns on
    /// `is_test_stub`), so this only proves the iteration +
    /// drain logic; live-Vk leak verification is via the smoke
    /// recipe with `VK_LAYER_KHRONOS_validation`.
    #[test]
    fn shutdown_destroy_all_drains_every_entry() {
        let mut s = DrawableStore::new();
        let platform = PlatformBackend::for_tests();
        let id1 = s
            .allocate(0x100_0001, DrawableKind::Pixmap, 32, false, stub_storage())
            .expect("allocate 1");
        let id2 = s
            .allocate(0x100_0002, DrawableKind::Window, 24, true, stub_storage())
            .expect("allocate 2");
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.by_xid.len(), 2);
        // Park one in pending_retire to verify it's cleared too
        // (would otherwise hold a stale DrawableId past the drain).
        s.pending_retire.push(id1);
        s.pending_retire.push(id2);

        s.shutdown_destroy_all(&platform);

        assert!(s.entries.is_empty(), "shutdown must drain entries");
        assert!(s.by_xid.is_empty(), "shutdown must clear xid lookup");
        assert!(
            s.pending_retire.is_empty(),
            "shutdown must clear pending_retire so a redrop attempt finds no DrawableId",
        );
    }

    /// `decref` invokes `on_destroyed` exactly when it actually
    /// destroys the drawable's storage (refcount→0 + ticket
    /// signaled). The callback is the bridge that lets
    /// `KmsBackend` invalidate `RenderEngine::drawable_view_cache`
    /// entries keyed on the destroyed `DrawableId` before
    /// `Storage::destroy` runs. Pre-fix the callback parameter
    /// didn't exist; the engine's cached views accumulated for
    /// every `DestroyPixmap` / `DestroyWindow` across the session
    /// and only got swept at engine `Drop`.
    #[test]
    fn decref_invokes_invalidation_callback_on_destroy() {
        let mut s = DrawableStore::new();
        let mut platform = PlatformBackend::for_tests();
        let id = s
            .allocate(0x100_0001, DrawableKind::Pixmap, 32, false, stub_storage())
            .expect("allocate");
        let mut invalidated = Vec::new();
        let decision = s.decref(&mut platform, id, |dropped| invalidated.push(dropped));
        assert_eq!(decision, RetireDecision::Destroyed);
        assert_eq!(
            invalidated,
            vec![id],
            "decref must invoke `on_destroyed(id)` BEFORE Storage::destroy \
             so engine view cache invalidates while the underlying VkImage \
             is still alive (the Vulkan view-before-image idiom)",
        );
    }

    /// Companion to the test above: when `decref` parks the
    /// drawable in `pending_retire` (ticket not yet signaled),
    /// the invalidation callback must NOT fire — the drawable's
    /// storage is still alive and its cached views are still
    /// valid. The callback fires later, when `poll_pending_retire`
    /// finally destroys it. Pre-fix this distinction was moot
    /// (no callback); post-fix it's load-bearing because firing
    /// early would invalidate views the engine may still be
    /// sampling from in-flight ops.
    #[test]
    fn decref_does_not_invalidate_when_still_referenced() {
        let mut s = DrawableStore::new();
        let mut platform = PlatformBackend::for_tests();
        let id = s
            .allocate(0x100_0002, DrawableKind::Pixmap, 32, false, stub_storage())
            .expect("allocate");
        s.incref(id);
        let mut invalidated = Vec::new();
        let decision = s.decref(&mut platform, id, |dropped| invalidated.push(dropped));
        assert_eq!(decision, RetireDecision::StillReferenced);
        assert!(
            invalidated.is_empty(),
            "decref with refcount > 1 must NOT invalidate — drawable storage \
             is still alive and engine cache entries are still valid",
        );
    }

    /// `poll_pending_retire` invokes `on_destroyed` per drawable
    /// that gets destroyed during the sweep (one callback per
    /// id, BEFORE `Storage::destroy`).
    #[test]
    fn poll_pending_retire_invokes_invalidation_callback_per_destroyed_id() {
        let mut s = DrawableStore::new();
        let mut platform = PlatformBackend::for_tests();
        // Stash two drawables in pending_retire by setting
        // refcount=1 + a (stub) ticket and decref'ing. Stub
        // storage has no real ticket so decref goes synchronous;
        // park them directly via the internal list.
        let id_a = s
            .allocate(0x100_0003, DrawableKind::Pixmap, 32, false, stub_storage())
            .expect("allocate a");
        let id_b = s
            .allocate(0x100_0004, DrawableKind::Pixmap, 32, false, stub_storage())
            .expect("allocate b");
        s.pending_retire.push(id_a);
        s.pending_retire.push(id_b);
        let mut invalidated = Vec::new();
        s.poll_pending_retire(&mut platform, |dropped| invalidated.push(dropped));
        assert_eq!(invalidated.len(), 2);
        assert!(invalidated.contains(&id_a));
        assert!(invalidated.contains(&id_b));
        assert!(s.entries.is_empty(), "both entries destroyed");
        assert!(s.pending_retire.is_empty(), "pending_retire drained");
    }

    #[test]
    fn mark_contents_modified_bumps_content_version() {
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1, DrawableKind::Pixmap, 32, false, stub_storage())
            .unwrap();
        assert_eq!(s.get(id).unwrap().content_version, 0);
        s.mark_contents_modified(id);
        s.mark_contents_modified(id);
        assert_eq!(s.get(id).unwrap().content_version, 2);
        // Unknown id is a silent no-op (never panics).
        s.mark_contents_modified(DrawableId::for_tests(u64::MAX));
    }

    /// `mark_contents_modified` uses `saturating_add`, so at `u64::MAX` the
    /// counter freezes rather than wrapping to 0. Wrapping would alias a fresh
    /// version onto the sentinel "nothing cached yet" value and cause the
    /// clip-mask cache to treat a new mask as unchanged. Freezing is
    /// preferable: the cache conservatively re-reads while the counter stays
    /// saturated, which is correct (if extremely rare in practice).
    #[test]
    fn mark_contents_modified_saturates_at_max() {
        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x1, DrawableKind::Pixmap, 32, false, stub_storage())
            .unwrap();
        s.get_mut(id).unwrap().content_version = u64::MAX;
        s.mark_contents_modified(id);
        assert_eq!(s.get(id).unwrap().content_version, u64::MAX);
    }

    #[test]
    fn imported_dmabuf_metadata_preserves_layout_exactly() {
        let metadata = ImportedDmabufMetadata {
            implicit_layout: false,
            fourcc: u32::from_le_bytes(*b"XR24"),
            vk_format: vk::Format::B8G8R8A8_UNORM,
            modifier: 0x0100_0000_0000_0002,
            planes: vec![ImportedDmabufPlane {
                offset: 4096,
                pitch: 8192,
            }],
            width: 1920,
            height: 1080,
            depth: 24,
            bpp: 32,
        };
        assert_eq!(metadata.fourcc, u32::from_le_bytes(*b"XR24"));
        assert_eq!(metadata.vk_format, vk::Format::B8G8R8A8_UNORM);
        assert_eq!(metadata.modifier, 0x0100_0000_0000_0002);
        assert_eq!(metadata.planes[0].offset, 4096);
        assert_eq!(metadata.planes[0].pitch, 8192);
        assert_eq!((metadata.width, metadata.height), (1920, 1080));
        assert_eq!(metadata.depth, 24);
        assert_eq!(metadata.bpp, 32);
        assert!(stub_storage().imported_dmabuf.is_none());
    }

    #[test]
    fn c0_2ci_storage_in_place_relayout_exclusion() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        let platform = PlatformBackend::for_tests();

        let storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 64,
                height: 64,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
        let lease = storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();

        // 1. With an active reader, write reservation fails with Busy
        let reader = service
            .reserve(lease.allocation.key(), UseKind::Read)
            .unwrap();
        let write_result = service.with_storage_write(&lease, |_| ());
        assert!(matches!(write_result, Err(ResourceError::Busy)));

        // Release reader; now write succeeds
        drop(reader);
        let write_result = service.with_storage_write(&lease, |alloc| {
            alloc.current_layout = vk::ImageLayout::GENERAL;
        });
        assert!(write_result.is_ok());

        // 2. With a pending obligation, write reservation also fails with Busy
        let ob = service
            .register(lease.allocation.key(), ObligationKind::KmsRelease)
            .unwrap();
        let write_result = service.with_storage_write(&lease, |_| ());
        assert!(matches!(write_result, Err(ResourceError::Busy)));

        // Fulfill obligation
        service
            .apply_validated_proof_for_tests(lease.allocation.key(), ob)
            .unwrap();
        let write_result = service.with_storage_write(&lease, |alloc| {
            alloc.current_layout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
        });
        assert!(write_result.is_ok());
    }

    #[test]
    fn c0_2ci_storage_allocate_and_copy_retaining_both_allocations() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        let platform = PlatformBackend::for_tests();

        // Old bordered window: width 100, bw 2 -> storage 104, content_offset (2, 2)
        let old_storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 104,
                height: 104,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        let old_target = PaintTarget::new(DrawableId::for_tests(1), (2, 2), None, 24);
        let old_lease = old_storage
            .into_managed(&mut service, &platform, old_target, (2, 2))
            .map_err(|(e, _)| e)
            .unwrap();

        // Simulate busy in-place write: old storage is in use by reader/KMS
        let _reader = service
            .reserve(old_lease.allocation.key(), UseKind::Read)
            .unwrap();

        // Since in-place write is busy, border relayout allocates a new storage:
        // New border width 4 -> storage 108, content_offset (4, 4)
        let new_storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 108,
                height: 108,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        let new_target = PaintTarget::new(DrawableId::for_tests(1), (4, 4), None, 24);
        let new_lease = new_storage
            .into_managed(&mut service, &platform, new_target, (4, 4))
            .map_err(|(e, _)| e)
            .unwrap();

        // Both allocations are retained simultaneously in the service
        assert_ne!(old_lease.allocation.key(), new_lease.allocation.key());
        assert!(service.contains(&old_lease.allocation.key()));
        assert!(service.contains(&new_lease.allocation.key()));

        assert_eq!(old_lease.pixels.content_offset, (2, 2));
        assert_eq!(new_lease.pixels.content_offset, (4, 4));
        assert_eq!(old_lease.pixels.extent.width, 104);
        assert_eq!(new_lease.pixels.extent.width, 108);

        // Content offset is never bumped on old storage
        assert_eq!(old_lease.pixels.content_offset, (2, 2));
    }

    #[test]
    fn c0_2ci_storage_promotion_with_old_read_kms_lease() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        let platform = PlatformBackend::for_tests();

        let mut s = DrawableStore::new();
        let id = s
            .allocate(0x50, DrawableKind::Pixmap, 24, false, stub_storage())
            .unwrap();

        // Adopt old storage into managed
        let target = PaintTarget::new(id, (0, 0), None, 24);
        let old_storage = std::mem::replace(&mut s.get_mut(id).unwrap().storage, stub_storage());
        let lease = old_storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();
        let old_key = lease.allocation.key();
        s.get_mut(id).unwrap().storage = Storage::from_backing(StorageBacking::Managed(lease));

        // Hold a read lease and a KMS release obligation on generation 1
        let read_holder = service.reserve(old_key, UseKind::Read).unwrap();
        let kms_ob = service
            .register(old_key, ObligationKind::KmsRelease)
            .unwrap();

        // Now promote storage (adopt_exportable_managed)
        let old_lease = s
            .get_mut(id)
            .unwrap()
            .storage
            .adopt_exportable_managed(
                &mut service,
                vk::Image::null(),
                vk::DeviceMemory::null(),
                vk::ImageView::null(),
                vk::ImageView::null(),
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                1024,
                65536,
                0,
                None,
            )
            .unwrap();

        let new_key = s
            .get(id)
            .unwrap()
            .storage
            .managed_lease()
            .unwrap()
            .allocation
            .key();
        assert_ne!(old_key, new_key);
        assert!(service.contains(&old_key));
        assert!(service.contains(&new_key));

        // Dropping old logical lease from return still leaves old_key alive in service due to read_holder and kms_ob
        drop(old_lease);
        let _ = service.service_ready();
        assert!(
            service.contains(&old_key),
            "generation 1 must survive while read and KMS are active"
        );

        // Fulfill KMS obligation
        service
            .apply_validated_proof_for_tests(old_key, kms_ob)
            .unwrap();
        let _ = service.service_ready();
        assert!(
            service.contains(&old_key),
            "generation 1 must survive while read lease is active"
        );

        // Drop read holder
        drop(read_holder);
        let _ = service.service_ready();
        assert!(
            !service.contains(&old_key),
            "generation 1 is destroyed after read and KMS are released"
        );
        assert!(service.contains(&new_key), "generation 2 remains active");
    }

    #[test]
    fn c0_2ci_storage_depth_semantics() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        let platform = PlatformBackend::for_tests();

        // Target has x11_depth 24
        let target = PaintTarget::new(DrawableId::for_tests(10), (0, 0), None, 24);
        // Backing storage is depth 32
        let storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 800,
                height: 600,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        assert_eq!(storage.depth, 32);

        let lease = storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();

        // Exact assertion required by contract:
        let pixels = &lease.pixels;
        assert_eq!(pixels.target.x11_depth(), 24);
        assert_eq!(pixels.allocation, lease.allocation.key());
    }

    #[test]
    fn c0_2ci_storage_no_premature_pool_return() {
        let platform = PlatformBackend::for_tests();

        // Promoted storage must never be eligible for pool return
        let mut promoted = Storage::for_tests_null(
            vk::Extent2D {
                width: 64,
                height: 64,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        if let StorageBacking::Legacy(ref mut alloc) = promoted.backing {
            alloc.promoted_exportable = true;
        }
        assert!(promoted.is_exportable(None));
        // destroy must not crash and must not pool-return
        promoted.destroy(&platform);
    }

    /// M-21: the deterministic test above never wires a real pool, so
    /// "must not pool-return" was unobservable -- `is_test_stub` makes
    /// `cleanup_handles` return before reaching the pool-return branch at
    /// all. This drives the real, non-stub path with a live `PixmapPool`
    /// and asserts the pool's own acceptance counter, not just "no panic".
    #[test]
    #[ignore = "needs live Vulkan ICD"]
    fn c0_2ci_storage_no_premature_pool_return_vulkan() {
        let vk = match crate::kms::vk::device::VkContext::new() {
            Ok(v) => v,
            Err(_) => {
                panic!("environmental skip: no live Vulkan ICD available; not claiming pass")
            }
        };
        let pool = Arc::new(crate::kms::vk::pixmap_pool::PixmapPool::new(Arc::clone(
            &vk,
        )));
        let mut platform = PlatformBackend::for_tests();
        platform.vk = Some(Arc::clone(&vk));
        platform.pixmap_pool = Some(Arc::clone(&pool));

        let mut storage = platform
            .allocate_drawable_storage(64, 64, 32)
            .expect("allocate_drawable_storage");
        if let StorageBacking::Legacy(ref mut alloc) = storage.backing {
            alloc.promoted_exportable = true;
        }
        storage.destroy(&platform);

        let stats = pool.stats();
        assert_eq!(
            stats.total_returns_accepted, 0,
            "a promoted (exportable) storage's handles must never re-enter the pixmap pool",
        );
        pool.drain();
    }

    /// B-14: `into_managed` must refuse to adopt a real (non-stub)
    /// allocation when no `VkContext` is available to eventually clean it
    /// up, rather than silently adopting it and leaking its Vulkan
    /// handles the first time the service actually destroys the entry.
    /// Fully deterministic: the null-handle, non-stub `Storage` never
    /// needs a real context to construct, only to prove `into_managed`
    /// checks for one.
    #[test]
    fn c0_2ci_storage_into_managed_refuses_non_stub_without_vk_context() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        // No Vk, no pool -- the precondition into_managed must refuse for
        // a non-stub allocation.
        let platform = PlatformBackend::for_tests();

        let storage = Storage::new_server_owned(
            vk::Image::null(),
            vk::DeviceMemory::null(),
            vk::ImageView::null(),
            vk::ImageView::null(),
            vk::Extent2D {
                width: 64,
                height: 64,
            },
            vk::Format::B8G8R8A8_UNORM,
            24,
        );
        let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
        let err = storage.into_managed(&mut service, &platform, target, (0, 0));
        assert!(
            matches!(err, Err((ResourceError::InvalidState, _))),
            "non-stub adoption without a Vk context must be refused, not \
             silently adopted with vk left None -- the payload's eventual \
             StorageAllocation::Drop would then find vk == None and skip \
             cleanup_handles entirely",
        );
    }

    /// B-14 live half: when a real `VkContext` IS available, `into_managed`
    /// must pin it onto the adopted allocation so `cleanup_handles` can
    /// actually run against it later, instead of leaving `vk: None`
    /// forever (the pre-fix signature had no `platform` parameter at all
    /// and never touched `vk`/`pixmap_pool`).
    #[test]
    #[ignore = "needs live Vulkan ICD"]
    fn c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan() {
        let vk = match crate::kms::vk::device::VkContext::new() {
            Ok(v) => v,
            Err(_) => {
                panic!("environmental skip: no live Vulkan ICD available; not claiming pass")
            }
        };
        let mut platform = PlatformBackend::for_tests();
        platform.vk = Some(Arc::clone(&vk));

        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);

        let storage = platform
            .allocate_drawable_storage(64, 64, 32)
            .expect("allocate_drawable_storage");
        let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
        let lease = storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .expect("adopt real, non-stub storage");
        let key = lease.allocation.key();

        let has_context = service
            .with_storage_read(&lease, |alloc| alloc.vk.is_some())
            .unwrap();
        assert!(
            has_context,
            "into_managed must pin the real VkContext so cleanup_handles \
             can run instead of finding vk == None and leaking",
        );

        // Real cleanup: dropping the lease and servicing must actually
        // destroy the live Vulkan handles (StorageAllocation::Drop),
        // not merely remove the bookkeeping entry.
        drop(lease);
        let _ = service.service_ready();
        assert!(
            !service.contains(&key),
            "managed storage reclaimed once the retain lease drops",
        );
    }

    /// M-18: proves `is_exportable_managed` reserves a `Read` use through
    /// `ResourceService::with_storage_read` instead of reading
    /// `AllocationEntry.payload` directly. Pre-fix, `Storage::is_exportable`
    /// returned a bare `bool` computed from an unreserved
    /// `RefCell::borrow()`, so a live incompatible writer could not make it
    /// fail -- there was no `Result` to fail with.
    #[test]
    fn c0_2ci_storage_is_exportable_managed_reserves_read_and_refuses_when_written() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        let platform = PlatformBackend::for_tests();

        let mut storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 64,
                height: 64,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        if let StorageBacking::Legacy(ref mut alloc) = storage.backing {
            alloc.promoted_exportable = true;
        }
        let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
        let lease = storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();
        let key = lease.allocation.key();
        let managed = Storage::from_backing(StorageBacking::Managed(lease));

        assert_eq!(
            managed.is_exportable_managed(&mut service),
            Ok(true),
            "read reservation succeeds and observes the real payload flag",
        );
        // F3-M1: Storage::is_exportable must not panic on Managed storage
        assert!(managed.is_exportable(Some(&mut service)));
        assert!(!managed.is_exportable(None));

        // A live writer makes a Read reservation incompatible
        // (`EntryAvailability::is_compatible`); the pre-fix direct
        // `.borrow()` had no reservation at all and could not observe this.
        let _writer = service.reserve(key, UseKind::Write).unwrap();
        assert_eq!(
            managed.is_exportable_managed(&mut service),
            Err(ResourceError::Busy),
            "is_exportable must go through the reservation protocol, not read \
             AllocationEntry.payload directly",
        );
    }

    /// M-18/F11-B1: proves `record_layout_transition_managed` reserves a
    /// `Write` use before mutating `current_layout`, refusing (and
    /// leaving the layout untouched) while an incompatible reader is
    /// live; then proves the SAME is true of the plain
    /// `Drawable::record_layout_transition` (F11-B1 -- pre-fix, that
    /// arm mutated a `StorageLease`-local `Cell<vk::ImageLayout>`
    /// unconditionally, with no reservation at all, so a live reader
    /// did not stop it); then proves `current_layout` has exactly one
    /// copy by transitioning through the drawable and observing the
    /// new layout through an independent `retain_storage` twin lease's
    /// `with_storage_read` -- pre-fix each lease had its own Cell
    /// snapshot and the twin would still show the OLD layout.
    /// `is_test_stub` keeps this deterministic (no real barrier is
    /// recorded), but the type still requires a live `VkContext` to
    /// construct at all.
    #[test]
    #[ignore = "needs live Vulkan ICD"]
    fn c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan() {
        let vk = match crate::kms::vk::device::VkContext::new() {
            Ok(v) => v,
            Err(_) => {
                panic!("environmental skip: no live Vulkan ICD available; not claiming pass")
            }
        };
        let platform = PlatformBackend::for_tests();
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);

        let mut s = DrawableStore::new();
        let storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 64,
                height: 64,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
        let lease = storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();
        let key = lease.allocation.key();
        let id = s
            .allocate(
                0x1,
                DrawableKind::Pixmap,
                24,
                false,
                Storage::from_backing(StorageBacking::Managed(lease)),
            )
            .unwrap();

        // No competing reservation: the transition succeeds and the stub
        // path (no real barrier recorded) still updates current_layout.
        s.get_mut(id)
            .unwrap()
            .record_layout_transition_managed(
                &mut service,
                &vk,
                vk::CommandBuffer::null(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                vk::AccessFlags2::empty(),
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::TRANSFER_WRITE,
            )
            .expect("no competing reservation");
        let layout_after_success = {
            let StorageBacking::Managed(lease) = &s.get(id).unwrap().storage.backing else {
                panic!("still managed");
            };
            service
                .with_storage_read(lease, |alloc| alloc.current_layout)
                .unwrap()
        };
        assert_eq!(layout_after_success, vk::ImageLayout::TRANSFER_DST_OPTIMAL);

        // A live reader makes Write incompatible.
        let _reader = service.reserve(key, UseKind::Read).unwrap();
        let refusal = s.get_mut(id).unwrap().record_layout_transition_managed(
            &mut service,
            &vk,
            vk::CommandBuffer::null(),
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            vk::PipelineStageFlags2::TRANSFER,
            vk::AccessFlags2::TRANSFER_WRITE,
            vk::PipelineStageFlags2::FRAGMENT_SHADER,
            vk::AccessFlags2::SHADER_SAMPLED_READ,
        );
        assert_eq!(
            refusal,
            Err(ResourceError::Busy),
            "record_layout_transition_managed must reserve Write, not mutate \
             current_layout directly under an outstanding reader",
        );
        let layout_after_refusal = {
            let StorageBacking::Managed(lease) = &s.get(id).unwrap().storage.backing else {
                panic!("still managed");
            };
            service
                .with_storage_read(lease, |alloc| alloc.current_layout)
                .unwrap()
        };
        assert_eq!(
            layout_after_refusal,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            "a refused write must not mutate current_layout",
        );

        // F11-B1: with the SAME live reader still held, the PLAIN
        // `Drawable::record_layout_transition` on Managed storage must
        // be refused too -- it now delegates to
        // `record_layout_transition_managed`, which reserves the same
        // Write use. Pre-fix, this arm mutated a lease-local Cell with
        // NO reservation at all, so this exact scenario (a live Read
        // reservation) went through silently.
        let plain_refusal = s.get_mut(id).unwrap().record_layout_transition(
            &vk,
            vk::CommandBuffer::null(),
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::PipelineStageFlags2::TRANSFER,
            vk::AccessFlags2::TRANSFER_WRITE,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            Some(&mut service),
        );
        assert_eq!(
            plain_refusal,
            Err(ResourceError::Busy),
            "the plain Drawable::record_layout_transition arm must reserve Write via \
             with_storage_write, not mutate a lease-local Cell unconditionally under a \
             live reader (F11-B1)",
        );
        let layout_still_unchanged = {
            let StorageBacking::Managed(lease) = &s.get(id).unwrap().storage.backing else {
                panic!("still managed");
            };
            service
                .with_storage_read(lease, |alloc| alloc.current_layout)
                .unwrap()
        };
        assert_eq!(
            layout_still_unchanged,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            "a refused plain transition must not mutate the single-sourced payload layout",
        );

        // F11-B1: with no service at all, the plain arm must propagate
        // InvalidState -- never fall back to a silent local write.
        let no_service_result = s.get_mut(id).unwrap().record_layout_transition(
            &vk,
            vk::CommandBuffer::null(),
            vk::ImageLayout::GENERAL,
            vk::PipelineStageFlags2::TRANSFER,
            vk::AccessFlags2::TRANSFER_WRITE,
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            None,
        );
        assert_eq!(
            no_service_result,
            Err(ResourceError::InvalidState),
            "a Managed transition with no ResourceService must propagate InvalidState, \
             matching the precedent RenderEngine::promote_drawable_exportable set (F11-B1)",
        );

        // F13a-m1: `Storage::set_current_layout`'s Managed arm must reserve
        // Write via `with_storage_write` too, not silently succeed (or
        // silently no-op) under the same live reader. Mutation: replacing
        // the Managed arm's body with a bare `Ok(())` makes this fail.
        let set_current_layout_refusal = s
            .get_mut(id)
            .unwrap()
            .storage
            .set_current_layout(vk::ImageLayout::GENERAL, Some(&mut service));
        assert_eq!(
            set_current_layout_refusal,
            Err(ResourceError::Busy),
            "Storage::set_current_layout must reserve Write via with_storage_write, not \
             mutate the payload unconditionally under a live reader (F13a-m1)",
        );
        let layout_after_set_current_layout_refusal = {
            let StorageBacking::Managed(lease) = &s.get(id).unwrap().storage.backing else {
                panic!("still managed");
            };
            service
                .with_storage_read(lease, |alloc| alloc.current_layout)
                .unwrap()
        };
        assert_eq!(
            layout_after_set_current_layout_refusal,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            "a refused set_current_layout must not mutate the payload (F13a-m1)",
        );

        drop(_reader);

        // F11-B1: `current_layout` has exactly one copy. Create an
        // independent twin lease over the SAME allocation via
        // `retain_storage` BEFORE the transition, then transition
        // through the drawable's own lease, then observe the new
        // layout through the twin's `with_storage_read` -- pre-fix,
        // each `StorageLease` carried its own `Cell` snapshot and the
        // twin would still report the OLD layout here.
        let twin = {
            let d = s.get(id).unwrap();
            let lease = d.storage.managed_lease().expect("still managed");
            service.retain_storage(lease).unwrap()
        };
        s.get_mut(id)
            .unwrap()
            .record_layout_transition(
                &vk,
                vk::CommandBuffer::null(),
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::TRANSFER_WRITE,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                Some(&mut service),
            )
            .expect("no competing reservation now that the reader is gone");
        let observed_by_twin = service
            .with_storage_read(&twin, |alloc| alloc.current_layout)
            .unwrap();
        assert_eq!(
            observed_by_twin,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            "a retain_storage twin lease over the same allocation must observe the \
             transition made through the drawable's lease -- single source of truth \
             (F11-B1)",
        );

        // Also exercise the accessor-level API `Storage::current_layout`/
        // `set_current_layout` (F11-B1's other required surface):
        // Managed + no service is InvalidState, never a silent value.
        let accessor_no_service = s.get(id).unwrap().storage.current_layout(None);
        assert_eq!(accessor_no_service, Err(ResourceError::InvalidState));
        let accessor_with_service = s
            .get(id)
            .unwrap()
            .storage
            .current_layout(Some(&mut service))
            .unwrap();
        assert_eq!(
            accessor_with_service,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        );

        // F13a-m1: a `set_current_layout` made through the drawable is also
        // observed by an independent `retain_storage` twin -- single
        // source of truth applies to the accessor-level write too, not
        // just `record_layout_transition_managed`. Mutation to paste:
        // replace `set_current_layout`'s Managed arm with a bare `Ok(())`
        // (no `with_storage_write` call) -- `observed_by_twin2` then still
        // reads `COLOR_ATTACHMENT_OPTIMAL` instead of `GENERAL` and this
        // assertion fails (134/134 pass without this test, per F13a-m1).
        let twin2 = {
            let d = s.get(id).unwrap();
            let lease = d.storage.managed_lease().expect("still managed");
            service.retain_storage(lease).unwrap()
        };
        s.get_mut(id)
            .unwrap()
            .storage
            .set_current_layout(vk::ImageLayout::GENERAL, Some(&mut service))
            .expect("no competing reservation");
        let observed_by_twin2 = service
            .with_storage_read(&twin2, |alloc| alloc.current_layout)
            .unwrap();
        assert_eq!(
            observed_by_twin2,
            vk::ImageLayout::GENERAL,
            "a retain_storage twin lease must observe a set_current_layout made through \
             the drawable (F13a-m1)",
        );
    }

    /// M-20: `Storage::destroy`'s Managed arm must detach the Retain use
    /// AT THE CALL, not merely whenever the surrounding `Storage`
    /// eventually drops. Pre-fix, the arm was a bare `{}`: this only
    /// looked correct because both current callers happen to drop the
    /// whole `Drawable` immediately afterward. This test calls
    /// `destroy()` and inspects service state WHILE the `Storage` value
    /// is still alive (not yet dropped) -- pre-fix, the retain use would
    /// still be held at that point, so `service_ready()` could not have
    /// reclaimed the entry yet.
    #[test]
    fn c0_2ci_storage_managed_destroy_detaches_before_drop() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        let platform = PlatformBackend::for_tests();

        let storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 64,
                height: 64,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
        let lease = storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();
        let key = lease.allocation.key();
        let mut managed_storage = Storage::from_backing(StorageBacking::Managed(lease));

        managed_storage.destroy(&platform);
        // `managed_storage` is still alive here -- deliberately not
        // dropped yet. If `destroy()` had not released the retain use
        // itself, this `service_ready()` would find `can_destroy` false
        // (live_use_count() == 1) and leave the entry rooted.
        let _ = service.service_ready();
        assert!(
            !service.contains(&key),
            "destroy() must detach the retain use immediately, not only when \
             the Storage value later drops",
        );

        // F3-m1: StorageBacking transitions to Detached, not a fabricated Legacy stub.
        assert!(
            managed_storage.is_detached(),
            "destroy() on Managed must transition to Detached (F3-m1)",
        );
        assert!(matches!(managed_storage.backing, StorageBacking::Detached));

        // Idempotent: destroying twice (now Detached) must not panic.
        managed_storage.destroy(&platform);
        assert!(managed_storage.is_detached());
        drop(managed_storage);
    }

    /// F3-m1: `Storage::destroy` on `Managed` transitions honestly to
    /// `StorageBacking::Detached` instead of fabricating an inert `Legacy`
    /// stub. All accessors report safe zero/null/default values.
    #[test]
    fn c0_2ci_storage_managed_destroy_transitions_to_detached() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        let platform = PlatformBackend::for_tests();

        let storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 64,
                height: 64,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
        let lease = storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();
        let mut managed_storage = Storage::from_backing(StorageBacking::Managed(lease));

        assert!(managed_storage.is_managed());
        assert!(!managed_storage.is_detached());
        assert_eq!(
            managed_storage.extent(),
            vk::Extent2D {
                width: 64,
                height: 64
            }
        );
        assert_eq!(managed_storage.depth(), 24);
        assert_eq!(managed_storage.format(), vk::Format::B8G8R8A8_UNORM);

        managed_storage.destroy(&platform);

        // F3-m1 decisive assertion: backing is Detached, not Legacy
        assert!(
            managed_storage.is_detached(),
            "Storage::destroy() must transition Managed to Detached",
        );
        assert!(matches!(managed_storage.backing, StorageBacking::Detached));
        assert!(!managed_storage.is_managed());
        assert_eq!(managed_storage.extent(), vk::Extent2D::default());
        assert_eq!(managed_storage.depth(), 0);
        assert_eq!(managed_storage.content_offset(), (0, 0));
        assert_eq!(managed_storage.format(), vk::Format::UNDEFINED);
        assert_eq!(managed_storage.image_view(), vk::ImageView::null());
        assert_eq!(managed_storage.sample_view(), vk::ImageView::null());
        assert!(!managed_storage.has_image_view());
        assert!(!managed_storage.is_exportable(None));

        // Idempotent: repeat call is safe
        managed_storage.destroy(&platform);
        assert!(managed_storage.is_detached());
    }

    /// M-20 integration: the same guarantee through the actual
    /// `DrawableStore` retirement seam (`decref` → `destroy_now`), which
    /// is what `FreePixmap`/window teardown actually calls in production.
    #[test]
    fn c0_2ci_storage_managed_drawable_decref_reclaims_via_service() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        let mut platform = PlatformBackend::for_tests();

        let storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 64,
                height: 64,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
        let lease = storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();
        let key = lease.allocation.key();

        let mut s = DrawableStore::new();
        let id = s
            .allocate(
                0x77,
                DrawableKind::Pixmap,
                24,
                false,
                Storage::from_backing(StorageBacking::Managed(lease)),
            )
            .unwrap();

        // No fence ticket attached, so decref-to-zero destroys
        // immediately (FreePixmap with nothing in flight).
        let decision = s.decref(&mut platform, id, |_| {});
        assert_eq!(decision, RetireDecision::Destroyed);
        assert!(s.get(id).is_none());

        let _ = service.service_ready();
        assert!(
            !service.contains(&key),
            "destroy_now's Storage::destroy must release the retain use so \
             service_ready can reclaim the managed allocation",
        );
    }

    #[test]
    fn c0_2ci_storage_dri3_lease_regressions() {
        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);
        let platform = PlatformBackend::for_tests();

        // 1. Explicit modifier
        let explicit_metadata = ImportedDmabufMetadata {
            fourcc: u32::from_le_bytes(*b"XR24"),
            vk_format: vk::Format::B8G8R8A8_UNORM,
            modifier: 0x0010_0000_0000_0001,
            implicit_layout: false,
            planes: vec![ImportedDmabufPlane {
                offset: 128,
                pitch: 1024,
            }],
            width: 256,
            height: 256,
            depth: 24,
            bpp: 32,
        };
        let mut explicit_storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 256,
                height: 256,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        if let StorageBacking::Legacy(ref mut alloc) = explicit_storage.backing {
            alloc.imported_dmabuf = Some(explicit_metadata);
        }
        let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
        let explicit_lease = explicit_storage
            .into_managed(&mut service, &platform, target, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();

        service
            .with_storage_read(&explicit_lease, |alloc| {
                let meta = alloc.imported_dmabuf.as_ref().unwrap();
                assert_eq!(meta.modifier, 0x0010_0000_0000_0001);
                assert!(!meta.implicit_layout);
                assert_eq!(meta.planes[0].offset, 128);
                assert_eq!(meta.planes[0].pitch, 1024);
            })
            .unwrap();

        // 2. Implicit modifier reports DRM_FORMAT_MOD_INVALID and stated client size
        let implicit_metadata = ImportedDmabufMetadata {
            fourcc: u32::from_le_bytes(*b"XR24"),
            vk_format: vk::Format::B8G8R8A8_UNORM,
            modifier: 0,
            implicit_layout: true,
            planes: vec![ImportedDmabufPlane {
                offset: 0,
                pitch: 1024,
            }],
            width: 256,
            height: 256,
            depth: 24,
            bpp: 32,
        };
        let mut implicit_storage = Storage::for_tests_null(
            vk::Extent2D {
                width: 256,
                height: 256,
            },
            vk::Format::B8G8R8A8_UNORM,
        );
        if let StorageBacking::Legacy(ref mut alloc) = implicit_storage.backing {
            alloc.imported_dmabuf = Some(implicit_metadata);
        }
        let target2 = PaintTarget::new(DrawableId::for_tests(2), (0, 0), None, 24);
        let implicit_lease = implicit_storage
            .into_managed(&mut service, &platform, target2, (0, 0))
            .map_err(|(e, _)| e)
            .unwrap();

        service
            .with_storage_read(&implicit_lease, |alloc| {
                let meta = alloc.imported_dmabuf.as_ref().unwrap();
                assert!(meta.implicit_layout);
                assert_eq!(meta.planes[0].offset, 0);
            })
            .unwrap();
    }

    /// 3.5b (M-21): the metadata-only regression above never drives a real
    /// import/export round trip, a logical FreePixmap or deferred
    /// retirement -- it is unobservable whether managed adoption changes
    /// any of upstream's DRI3 export guarantees
    /// (`dri3_imported_pixmap_exports_the_clients_own_description` in
    /// `backend.rs`). This extends that same contract across managed
    /// adoption: a real dma-buf is imported, wrapped as managed
    /// `Storage`, exported back out through `dri3::export_dmabuf` while
    /// still owned by the service, and only actually destroyed once a
    /// simulated FreePixmap's retirement is no longer deferred by an
    /// outstanding KMS obligation.
    #[test]
    #[ignore = "needs a Vulkan ICD that can export dma-bufs (not lavapipe)"]
    fn c0_2ci_storage_dri3_lease_regressions_vulkan() {
        use crate::kms::vk::{
            device::VkContext,
            dri3::{self, DRM_FORMAT_MOD_INVALID, DmabufPlane},
            target::allocate_exportable,
        };

        let vk = match VkContext::new() {
            Ok(v) => v,
            Err(_) => {
                panic!("environmental skip: no live Vulkan ICD available; not claiming pass")
            }
        };
        let mut platform = PlatformBackend::for_tests();
        platform.vk = Some(Arc::clone(&vk));

        let (w, h) = (256u32, 64u32);
        let seed = match allocate_exportable(&vk, w, h, vk::Format::B8G8R8A8_UNORM) {
            Ok(img) => img,
            Err(e) if format!("{e:?}").contains("FORMAT_NOT_SUPPORTED") => {
                panic!("environmental skip: no live Vulkan ICD available; not claiming pass")
            }
            Err(e) => panic!("fixture: allocate_exportable: {e:?}"),
        };
        let seed_export = dri3::export_backing(&vk, &seed).expect("fixture: export seed");
        assert!(
            seed_export.size > 0 && seed_export.stride > 0,
            "fixture: seed export must describe a real buffer, got size={} stride={}",
            seed_export.size,
            seed_export.stride,
        );
        let stride = seed_export.stride;
        let seed_modifier = seed_export.modifier;
        // Deliberately NOT the seed's own size: a distinguishable value is
        // what gives "the client-stated size round-trips" assertion teeth.
        let stated_size = seed_export.size + 4096;

        let device_key = crate::platform::drm::DrmDeviceKey {
            major: 226,
            minor: 0,
        };
        let incarnation = crate::kms::owner::identity::IncarnationId::first();
        let mut service = ResourceService::new(device_key, incarnation);

        // `vk_import_modifier` is what Vulkan is told to import with
        // (production always forces LINEAR for an implicit layout -- it
        // has nothing else to resolve it to, per `dri3_import_pixmap`);
        // `reported_modifier` is what `DrawableImage.drm_modifier` (and
        // hence `dri3::export_dmabuf`) tells the CLIENT back. The two
        // diverge exactly for the implicit case -- conflating them is
        // what made the initial version of this test assert LINEAR
        // where the contract requires `DRM_FORMAT_MOD_INVALID`.
        for (
            case,
            vk_import_modifier,
            reported_modifier,
            implicit_layout,
            client_size,
            expected_modifier,
            expected_size,
        ) in [
            (
                "implicit",
                dri3::DRM_FORMAT_MOD_LINEAR,
                DRM_FORMAT_MOD_INVALID,
                true,
                Some(stated_size),
                DRM_FORMAT_MOD_INVALID,
                stated_size,
            ),
            (
                "explicit",
                seed_modifier,
                seed_modifier,
                false,
                None,
                seed_modifier,
                seed_export.size,
            ),
        ] {
            // Once-only FD ownership: each case dups its own handle from
            // the seed; `import_dmabuf_reporting` takes ownership of
            // exactly this dup, never the seed's own fd.
            let fd = seed_export
                .fd
                .try_clone()
                .unwrap_or_else(|e| panic!("{case}: dup seed fd: {e}"));
            let drawable = dri3::import_dmabuf_reporting(
                Arc::clone(&vk),
                fd,
                w,
                h,
                vk::Format::B8G8R8A8_UNORM,
                vk_import_modifier,
                Some(reported_modifier),
                client_size,
                &[DmabufPlane {
                    offset: 0,
                    pitch: stride,
                }],
            )
            .unwrap_or_else(|e| panic!("{case}: import failed: {e:?}"));

            let sample_view =
                PlatformBackend::build_sample_view(&vk, drawable.vk_image, drawable.format, 24)
                    .unwrap_or_else(|e| panic!("{case}: build_sample_view: {e:?}"));

            let storage = Storage::from_imported_drawable_image(
                drawable,
                sample_view,
                24,
                ImportedDmabufMetadata {
                    fourcc: u32::from_le_bytes(*b"XR24"),
                    vk_format: vk::Format::B8G8R8A8_UNORM,
                    modifier: vk_import_modifier,
                    implicit_layout,
                    planes: vec![ImportedDmabufPlane {
                        offset: 0,
                        pitch: stride,
                    }],
                    width: u16::try_from(w).unwrap(),
                    height: u16::try_from(h).unwrap(),
                    depth: 24,
                    bpp: 32,
                },
            );

            let target = PaintTarget::new(DrawableId::for_tests(1), (0, 0), None, 24);
            let lease = storage
                .into_managed(&mut service, &platform, target, (0, 0))
                .map_err(|(e, _)| e)
                .unwrap_or_else(|e| panic!("{case}: into_managed: {e:?}"));
            let key = lease.allocation.key();

            // Export path round-trips the client's own description even
            // through managed adoption -- not a re-derived Vulkan view.
            let export = service
                .with_storage_read(&lease, |alloc| {
                    dri3::export_dmabuf(&vk, alloc.imported_drawable.as_ref().unwrap())
                })
                .unwrap_or_else(|e| panic!("{case}: with_storage_read: {e:?}"))
                .unwrap_or_else(|e| panic!("{case}: export_dmabuf: {e:?}"));
            assert_eq!(
                export.modifier, expected_modifier,
                "{case}: exported modifier must be what the client's buffer is described by \
                 -- reporting LINEAR for an unnamed layout is #138",
            );
            assert_eq!(
                export.stride, stride,
                "{case}: the client's own stride must survive the round trip",
            );
            assert_eq!(export.offset, 0, "{case}: no client fd offset change");
            assert_eq!(
                export.size, expected_size,
                "{case}: the client's stated buffer size must be reported verbatim",
            );

            // Logical FreePixmap + deferred retirement (3.5b): a still-
            // outstanding KMS obligation must keep the managed allocation
            // alive across a decref-to-zero, and release it only once
            // that obligation is discharged -- not merely whenever the
            // Drawable/Storage happens to drop.
            let kms_ob = service
                .register(key, ObligationKind::KmsRelease)
                .unwrap_or_else(|e| panic!("{case}: register KMS obligation: {e:?}"));

            let mut drawables = DrawableStore::new();
            let id = drawables
                .allocate(
                    0x99,
                    DrawableKind::Pixmap,
                    24,
                    false,
                    Storage::from_backing(StorageBacking::Managed(lease)),
                )
                .unwrap();
            let mut decref_platform = PlatformBackend::for_tests();
            let decision = drawables.decref(&mut decref_platform, id, |_| {});
            assert_eq!(
                decision,
                RetireDecision::Destroyed,
                "{case}: no fence ticket attached, so FreePixmap destroys immediately",
            );
            let _ = service.service_ready();
            assert!(
                service.contains(&key),
                "{case}: retirement is deferred while the KMS obligation is outstanding",
            );

            service
                .apply_validated_proof_for_tests(key, kms_ob)
                .unwrap_or_else(|e| panic!("{case}: discharge KMS obligation: {e:?}"));
            let _ = service.service_ready();
            assert!(
                !service.contains(&key),
                "{case}: managed storage reclaimed once the obligation clears",
            );
            // Idempotent: servicing again must not attempt to close the
            // dma-buf fd or destroy the sample view a second time.
            let _ = service.service_ready();
        }
    }
}
