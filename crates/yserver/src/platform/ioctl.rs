//! Portable raw-ioctl ABI boundary.

use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd},
};

/// Safety: request must describe T's initialized allocation, including every
/// pointed-to allocation the kernel may access for the duration of this ioctl.
pub(crate) unsafe fn ioctl_readwrite<T>(
    fd: BorrowedFd<'_>,
    request: u32,
    arg: *mut T,
) -> io::Result<()> {
    let rc = unsafe { libc::ioctl(fd.as_raw_fd(), request as _, arg) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// DRM ioctl type letter, `<drm/drm.h>` `DRM_IOCTL_BASE`.
pub(crate) const DRM_IOCTL_BASE: u8 = b'd';

const DIRECTION_READ_WRITE: u32 = 3;
const SIZE_SHIFT: u32 = 16;
const TYPE_SHIFT: u32 = 8;
const DIRECTION_SHIFT: u32 = 30;
// The size field's width itself is not portable either: Linux's
// `_IOC_SIZEBITS` is 14 bits (`<asm-generic/ioctl.h>`), but FreeBSD's
// `IOCPARM_MASK` (`<sys/ioccom.h>`) is only 13 bits. This guard exists to
// catch an oversized payload struct as a build-time programming error; on
// FreeBSD the 14-bit mask would silently accept a struct one bit too wide
// for the field it is about to be packed into.
#[cfg(not(target_os = "freebsd"))]
const SIZE_MASK: u32 = 0x3FFF;
#[cfg(target_os = "freebsd")]
const SIZE_MASK: u32 = 0x1FFF;

/// Build an `_IOWR` request code.
///
/// `size` is the payload struct size in bytes and must fit the size field
/// (14 bits on Linux, 13 on FreeBSD — see `SIZE_MASK`); a larger struct is
/// a programming error, not a runtime condition, so this panics in a const
/// context at build time.
pub(crate) const fn iowr(kind: u8, nr: u8, size: usize) -> u32 {
    assert!(
        size <= SIZE_MASK as usize,
        "ioctl payload exceeds the 14-bit size field"
    );
    (DIRECTION_READ_WRITE << DIRECTION_SHIFT)
        | ((size as u32) << SIZE_SHIFT)
        | ((kind as u32) << TYPE_SHIFT)
        | (nr as u32)
}

#[cfg(test)]
mod tests {
    use super::{DRM_IOCTL_BASE, iowr};

    #[test]
    fn iowr_reproduces_the_queue_sequence_request_code() {
        // _IOWR('d' /*0x64*/, 0x3C, drm_crtc_queue_sequence /*24 bytes*/):
        //   (3 << 30) | (24 << 16) | (0x64 << 8) | 0x3C = 0xC018643C
        assert_eq!(iowr(DRM_IOCTL_BASE, 0x3C, 24), 0xC018_643C_u32);
    }

    #[test]
    fn iowr_reproduces_the_atomic_request_code() {
        // _IOWR('d', 0xBC, drm_mode_atomic /*40 bytes*/):
        //   (3 << 30) | (40 << 16) | (0x64 << 8) | 0xBC = 0xC02864BC
        assert_eq!(iowr(DRM_IOCTL_BASE, 0xBC, 40), 0xC028_64BC_u32);
    }
}
