//! Platform sync_file ABI and status query.

use std::{io, os::fd::BorrowedFd};

#[cfg(target_os = "linux")]
use crate::platform::ioctl::{ioctl_readwrite, iowr};

#[repr(C)]
#[derive(Debug, Default)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct SyncFileInfo {
    pub(crate) name: [u8; 32],
    pub(crate) status: i32,
    pub(crate) flags: u32,
    pub(crate) num_fences: u32,
    pub(crate) pad: u32,
    pub(crate) sync_fence_info: u64,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FenceStatus {
    Pending,
    Success,
    Error(i32),
}

/// Query the status of an explicit sync_file fence descriptor.
pub fn query_status(fd: BorrowedFd<'_>) -> io::Result<FenceStatus> {
    #[cfg(target_os = "linux")]
    {
        let mut info = SyncFileInfo::default();
        let request = iowr(b'>', 4, std::mem::size_of::<SyncFileInfo>());
        unsafe {
            ioctl_readwrite(fd, request, std::ptr::addr_of_mut!(info))?;
        }
        Ok(match info.status {
            0 => FenceStatus::Pending,
            n if n > 0 => FenceStatus::Success,
            n => FenceStatus::Error(n),
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = fd;
        Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn sync_file_info_layout_and_request_code() {
        assert_eq!(std::mem::size_of::<SyncFileInfo>(), 56);
        assert_eq!(std::mem::align_of::<SyncFileInfo>(), 8);
        assert_eq!(std::mem::offset_of!(SyncFileInfo, status), 32);
        assert_eq!(std::mem::offset_of!(SyncFileInfo, num_fences), 40);
        assert_eq!(std::mem::offset_of!(SyncFileInfo, sync_fence_info), 48);

        let request = crate::platform::ioctl::iowr(b'>', 4, std::mem::size_of::<SyncFileInfo>());
        assert_eq!(request as u32, 0xc038_3e04);
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Real pipe and /dev/null descriptors must fail
    /// query_status with ENOTTY or EINVAL; readiness cannot be called Success.
    #[test]
    fn real_pipe_and_dev_null_fail_query_status() {
        let dev_null = std::fs::File::open("/dev/null").expect("open /dev/null");
        let null_res = query_status(dev_null.as_fd());
        assert!(
            null_res.is_err(),
            "real /dev/null must fail query_status, got: {:?}",
            null_res
        );

        let (r, _w) = nix::unistd::pipe().expect("pipe");
        let pipe_res = query_status(r.as_fd());
        assert!(
            pipe_res.is_err(),
            "real pipe read-end must fail query_status, got: {:?}",
            pipe_res
        );
    }
}
