use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::io::{AsFd, BorrowedFd, OwnedFd},
    sync::Arc,
};

use crate::drm::client_caps::{
    CursorDeviceCaps, DeviceControl, DrmDeviceControl, negotiate_client_caps,
};
use drm::Device as DrmDevice;

pub struct Device {
    file: File,
    path: String,
    master_ownership: MasterOwnership,
    device_control: Arc<dyn DeviceControl + Send + Sync>,
    cursor_caps: Option<CursorDeviceCaps>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MasterOwnership {
    None,
    AcquiredHere,
    InheritedDuplicate,
}

impl AsFd for Device {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

impl DrmDevice for Device {}
impl drm::control::Device for Device {}

impl Device {
    /// Construct a stub `Device` backed by `/dev/null` for tests.
    ///
    /// The returned device is unable to issue real ioctls; callers
    /// that exercise actual DRM control paths must use `open`.
    /// Hidden from rustdoc — for use by test fixtures only.
    #[doc(hidden)]
    pub fn for_tests() -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;
        Ok(Self {
            file,
            path: "/dev/null".to_string(),
            master_ownership: MasterOwnership::None,
            device_control: Arc::new(DrmDeviceControl),
            cursor_caps: None,
        })
    }

    /// Construct a `/dev/null` device and negotiate against an injected test dependency.
    #[doc(hidden)]
    #[cfg(test)]
    pub(crate) fn for_tests_with_control(
        device_control: Arc<dyn DeviceControl + Send + Sync>,
    ) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;
        let mut device = Self {
            file,
            path: "/dev/null".to_string(),
            master_ownership: MasterOwnership::None,
            device_control,
            cursor_caps: None,
        };
        device.negotiate_cursor_caps()?;
        Ok(device)
    }

    /// Open a render node without taking DRM master.
    ///
    /// `open` is the KMS-master constructor: it acquires DRM master and
    /// negotiates KMS client caps. Render nodes still serve the
    /// `DRM_RENDER_ALLOW` ioctls used by the syncobj paths.
    pub fn open_render_node(path: &str) -> io::Result<Self> {
        Self::open_render_node_with_control(path, Arc::new(DrmDeviceControl))
    }

    pub(crate) fn open_render_node_with_control(
        path: &str,
        device_control: Arc<dyn DeviceControl + Send + Sync>,
    ) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| open_error(path, &err))?;
        Ok(Self {
            file,
            path: path.to_string(),
            master_ownership: MasterOwnership::None,
            device_control,
            cursor_caps: None,
        })
    }

    /// Wrap a duplicated KMS fd inherited by a helper process.
    ///
    /// The fd must refer to the same DRM file description as a parent-owned
    /// KMS device. This `Device` owns and closes its duplicate, but deliberately
    /// never issues `DRM_IOCTL_DROP_MASTER`: master ownership remains with the
    /// parent, and closing one duplicate does not release it while the parent's
    /// reference remains open. Per-file client capabilities are inherited with
    /// the duplicated file description, so this constructor does not repeat
    /// `SET_CLIENT_CAP` either.
    #[doc(hidden)]
    pub fn from_inherited_kms_fd(fd: OwnedFd, path: impl Into<String>) -> Self {
        Self::from_inherited_kms_fd_with_control(fd, path, Arc::new(DrmDeviceControl))
    }

    pub(crate) fn from_inherited_kms_fd_with_control(
        fd: OwnedFd,
        path: impl Into<String>,
        device_control: Arc<dyn DeviceControl + Send + Sync>,
    ) -> Self {
        Self {
            file: File::from(fd),
            path: path.into(),
            master_ownership: MasterOwnership::InheritedDuplicate,
            device_control,
            cursor_caps: None,
        }
    }

    pub fn open(path: &str) -> io::Result<Self> {
        Self::open_with_control(path, Arc::new(DrmDeviceControl))
    }

    pub(crate) fn open_with_control(
        path: &str,
        device_control: Arc<dyn DeviceControl + Send + Sync>,
    ) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| open_error(path, &err))?;
        let mut device = Self {
            file,
            path: path.to_string(),
            master_ownership: MasterOwnership::AcquiredHere,
            device_control,
            cursor_caps: None,
        };
        device.acquire_master_lock().map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to acquire DRM master on {path}: {err}"),
            )
        })?;
        device.negotiate_cursor_caps()?;
        Ok(device)
    }

    fn negotiate_cursor_caps(&mut self) -> io::Result<()> {
        self.cursor_caps = Some(negotiate_client_caps(self, self.device_control.as_ref())?);
        Ok(())
    }

    /// Cached capability and driver identity from primary-node initialization.
    pub fn cursor_caps(&self) -> Option<&CursorDeviceCaps> {
        self.cursor_caps.as_ref()
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if self.master_ownership != MasterOwnership::AcquiredHere {
            return;
        }
        if let Err(err) = self.release_master_lock() {
            log::warn!(
                "failed to release DRM master on {}: {err} (file close will still drop the fd)",
                self.path
            );
        }
    }
}

fn open_error(path: &str, err: &io::Error) -> io::Error {
    use io::ErrorKind;
    let msg = match err.kind() {
        ErrorKind::NotFound => format!(
            "DRM device {path} not found. In vng: pass --graphics or \
             --qemu-opts=\"-device virtio-gpu-pci\". On bare metal: check `ls /dev/dri/` — \
             the GPU may be at card1 instead. Override with \
             `YSERVER_DRM_DEVICE=/dev/dri/cardN`."
        ),
        ErrorKind::PermissionDenied => format!(
            "opening {path} requires root — vng runs as root by default; on host use sudo \
             (but B is vng-only by design)"
        ),
        _ if err.raw_os_error() == Some(libc::EBUSY) => format!(
            "another DRM master holds {path} — B is vng-only; do not run yserver on a host \
             with an active graphical session"
        ),
        _ => format!("failed to open {path}: {err}"),
    };
    io::Error::new(err.kind(), msg)
}
