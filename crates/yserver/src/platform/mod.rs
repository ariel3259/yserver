pub(crate) mod drm;
pub(crate) mod ioctl;

#[cfg(target_os = "linux")]
mod drm_linux;
