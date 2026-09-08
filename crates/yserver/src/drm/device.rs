use std::{
    fs::{File, OpenOptions},
    io,
    os::unix::io::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
};

use drm::{ClientCapability, Device as DrmDevice};

pub struct Device {
    file: File,
    path: String,
    master_ownership: MasterOwnership,
    atomic_client_cap_enabled: bool,
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
        let (reader, writer) = std::os::unix::net::UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        std::mem::forget(writer);
        Ok(Self {
            file: std::fs::File::from(std::os::fd::OwnedFd::from(reader)),
            path: "/dev/null".to_string(),
            master_ownership: MasterOwnership::None,
            atomic_client_cap_enabled: false,
        })
    }

    /// Open a render node without taking DRM master.
    ///
    /// `open` is the KMS-master constructor: it calls `acquire_master_lock`
    /// and `enable_atomic_capabilities`, both of which a render node rejects
    /// (`DRM_IOCTL_SET_MASTER` → `EACCES` from a render client). Render nodes
    /// still serve the `DRM_RENDER_ALLOW` ioctls, which is everything the
    /// syncobj paths need.
    pub fn open_render_node(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| open_error(path, &err))?;
        Ok(Self {
            file,
            path: path.to_string(),
            master_ownership: MasterOwnership::None,
            atomic_client_cap_enabled: false,
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
        Self {
            file: File::from(fd),
            path: path.into(),
            master_ownership: MasterOwnership::InheritedDuplicate,
            atomic_client_cap_enabled: false,
        }
    }

    /// Wrap an existing file descriptor for tests.
    #[doc(hidden)]
    pub fn from_file_for_tests(file: File) -> Self {
        Self {
            file,
            path: "test_device".to_string(),
            master_ownership: MasterOwnership::None,
            atomic_client_cap_enabled: false,
        }
    }

    pub fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| open_error(path, &err))?;
        set_and_verify_nonblocking(file.as_raw_fd())?;
        let mut device = Self {
            file,
            path: path.to_string(),
            master_ownership: MasterOwnership::AcquiredHere,
            atomic_client_cap_enabled: false,
        };
        device.acquire_master_lock().map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to acquire DRM master on {path}: {err}"),
            )
        })?;
        device.atomic_client_cap_enabled = device.enable_atomic_capabilities()?;
        Ok(device)
    }

    #[allow(dead_code)]
    pub(crate) fn set_and_verify_nonblocking_for_tests(fd: std::os::fd::RawFd) -> io::Result<()> {
        set_and_verify_nonblocking(fd)
    }

    fn enable_atomic_capabilities(&self) -> io::Result<bool> {
        // Both UniversalPlanes and Atomic are *opt-ins to fd visibility*
        // — drivers that have inherently-universal-only planes (e.g.
        // Asahi's apple_drm) reject the cap-set with EOPNOTSUPP even
        // though atomic state is still honoured at the ioctl level.
        // Warn but continue on either; an actually-non-atomic driver
        // will fail downstream at the first atomic_commit, which is
        // where we'd want the diagnostic anyway.
        if let Err(err) = self.set_client_capability(ClientCapability::UniversalPlanes, true) {
            log::warn!(
                "DRM_CLIENT_CAP_UNIVERSAL_PLANES rejected ({err}); driver is presumably \
                 universal-only — continuing"
            );
        }
        let atomic_ok = match self.set_client_capability(ClientCapability::Atomic, true) {
            Ok(()) => true,
            Err(err) => {
                log::warn!(
                    "DRM_CLIENT_CAP_ATOMIC rejected ({err}); the driver may still honour \
                     atomic_commit ioctls without the explicit opt-in (Asahi apple_drm) — \
                     continuing. If subsequent atomic_commit calls fail, the driver is \
                     genuinely non-atomic and yserver/KMS won't work on this kernel."
                );
                false
            }
        };
        Ok(atomic_ok)
    }

    #[allow(dead_code)]
    pub(crate) fn atomic_client_cap_enabled(&self) -> bool {
        self.atomic_client_cap_enabled
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

fn set_and_verify_nonblocking(fd: std::os::fd::RawFd) -> io::Result<()> {
    // SAFETY: fcntl F_GETFL reads descriptor status flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl F_SETFL sets flags preserving existing ones.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl F_GETFL verifies the flag was set.
    let new_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if new_flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if (new_flags & libc::O_NONBLOCK) == 0 {
        return Err(io::Error::other("O_NONBLOCK flag verification failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn nonblocking_flag_preservation_and_empty_drain_returns_wouldblock() {
        let (reader, _writer) = UnixStream::pair().expect("socketpair");
        let raw_fd = reader.as_raw_fd();
        let initial_flags = unsafe { libc::fcntl(raw_fd, libc::F_GETFL) };
        assert!(initial_flags >= 0);

        set_and_verify_nonblocking(raw_fd).expect("set nonblocking");

        let after_flags = unsafe { libc::fcntl(raw_fd, libc::F_GETFL) };
        assert_eq!(after_flags & libc::O_NONBLOCK, libc::O_NONBLOCK);
        // Preserves existing access mode / flags
        assert_eq!(
            after_flags & !libc::O_NONBLOCK,
            initial_flags & !libc::O_NONBLOCK
        );

        let mut seen = Vec::new();
        let res = crate::drm::event_stream::drain_fd_events(&reader, |rec| seen.push(rec));
        assert!(matches!(
            res,
            Ok(crate::drm::event_stream::DrainStop::WouldBlock)
        ));
        assert!(seen.is_empty());
    }
}
