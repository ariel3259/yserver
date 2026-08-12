//! XSync / DRI3 `VkSemaphore` import / export helpers (Phase 4.2.2,
//! design §3.4).
//!
//! - **Binary semaphore from a `sync_file` fd** — [`import_sync_file`].
//!   Used by DRI3 `FenceFromFD`. The imported payload is TEMPORARY:
//!   it lasts only until the first wait, which matches XSync
//!   `Fence`'s one-shot trigger/wait semantics.
//! - [`export_sync_file`] — symmetric to `import_sync_file`, used by
//!   DRI3 `FDFromFence` to hand the client back a `sync_file` fd.
//!
//! **fd ownership rule** (design §3.2). `vkImportSemaphoreFdKHR`
//! consumes the fd only on `VK_SUCCESS`. The helpers in this module
//! take `OwnedFd` by value: on success, the fd's ownership transfers
//! into the resulting `VkSemaphore`; on failure, we re-claim the
//! raw fd and close it before returning `Err`.

use ash::vk;
use std::os::fd::{IntoRawFd, OwnedFd};

use super::device::VkContext;

/// Import a `sync_file` fd as a fresh **binary** `VkSemaphore`.
/// Phase 4.2.2 — backs DRI3 `FenceFromFD`.
pub fn import_sync_file(vk: &VkContext, fd: OwnedFd) -> Result<vk::Semaphore, vk::Result> {
    let create_info = vk::SemaphoreCreateInfo::default();
    let semaphore = unsafe { vk.device.create_semaphore(&create_info, None)? };
    let raw = fd.into_raw_fd();
    let import_info = vk::ImportSemaphoreFdInfoKHR::default()
        .semaphore(semaphore)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
        // SYNC_FD import is required to be TEMPORARY by spec.
        .flags(vk::SemaphoreImportFlags::TEMPORARY)
        .fd(raw);
    let result = unsafe { vk.external_semaphore_fd.import_semaphore_fd(&import_info) };
    match result {
        Ok(()) => Ok(semaphore),
        Err(e) => {
            // SAFETY: vkImportSemaphoreFdKHR didn't consume the fd
            // on failure — we still own it and must close to avoid
            // a leak. Then destroy the freshly-created semaphore.
            unsafe {
                libc::close(raw);
                vk.device.destroy_semaphore(semaphore, None);
            }
            Err(e)
        }
    }
}

/// Export a `VkSemaphore`'s current payload as a fresh `sync_file`
/// fd. Phase 4.2.2 — backs DRI3 `FDFromFence`. The returned fd's
/// ownership transfers to the caller (Vulkan's internal copy is
/// disjoint from this fd).
pub fn export_sync_file(vk: &VkContext, semaphore: vk::Semaphore) -> Result<OwnedFd, vk::Result> {
    let info = vk::SemaphoreGetFdInfoKHR::default()
        .semaphore(semaphore)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let raw = unsafe { vk.external_semaphore_fd.get_semaphore_fd(&info)? };
    super::owned_fd_from_vk(raw, "vkGetSemaphoreFdKHR(SYNC_FD)")
}

#[cfg(test)]
mod tests {
    // The import / export helpers all touch live Vulkan handles, so
    // they're only exercised by the integration smoke under vng or
    // bare metal (design §5.5 hardware coverage matrix). Wire-level
    // unit-test coverage of the wrapping XSync resource types lives
    // in process_request::tests::sync_create_trigger_query_fence_round_trip.
}
