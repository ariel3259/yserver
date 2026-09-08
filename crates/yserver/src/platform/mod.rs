#[doc(hidden)]
pub mod drm;
pub(crate) mod ioctl;
pub mod sync_file;

#[cfg(target_os = "linux")]
mod drm_linux;
