//! Portable raw-ioctl ABI boundary.

#![allow(dead_code)] // Boundary lands in Stage 1 Task 1; callers migrate onto it in later Phase C.0 tasks.

#[cfg(target_os = "linux")]
pub(crate) type IoctlReq = libc::Ioctl;
#[cfg(not(target_os = "linux"))]
pub(crate) type IoctlReq = libc::c_ulong;

/// DRM ioctl type letter, `<drm/drm.h>` `DRM_IOCTL_BASE`.
pub(crate) const DRM_IOCTL_BASE: u8 = b'd';

const DIRECTION_READ_WRITE: u32 = 3;
const SIZE_SHIFT: u32 = 16;
const TYPE_SHIFT: u32 = 8;
const DIRECTION_SHIFT: u32 = 30;
const SIZE_MASK: u32 = 0x3FFF;

/// Build an `_IOWR` request code.
///
/// `size` is the payload struct size in bytes and must fit the 14-bit
/// size field; a larger struct is a programming error, not a runtime
/// condition, so this panics in a const context at build time.
pub(crate) const fn iowr(kind: u8, nr: u8, size: usize) -> IoctlReq {
    assert!(
        size <= SIZE_MASK as usize,
        "ioctl payload exceeds the 14-bit size field"
    );
    let code = (DIRECTION_READ_WRITE << DIRECTION_SHIFT)
        | ((size as u32) << SIZE_SHIFT)
        | ((kind as u32) << TYPE_SHIFT)
        | (nr as u32);
    code as IoctlReq
}

#[cfg(test)]
mod tests {
    use super::{DRM_IOCTL_BASE, IoctlReq, iowr};

    #[test]
    fn iowr_reproduces_the_queue_sequence_request_code() {
        // _IOWR('d' /*0x64*/, 0x3C, drm_crtc_queue_sequence /*24 bytes*/):
        //   (3 << 30) | (24 << 16) | (0x64 << 8) | 0x3C = 0xC018643C
        assert_eq!(iowr(DRM_IOCTL_BASE, 0x3C, 24), 0xC018_643C_u32 as IoctlReq);
    }

    #[test]
    fn iowr_reproduces_the_atomic_request_code() {
        // _IOWR('d', 0xBC, drm_mode_atomic /*40 bytes*/):
        //   (3 << 30) | (40 << 16) | (0x64 << 8) | 0xBC = 0xC02864BC
        assert_eq!(iowr(DRM_IOCTL_BASE, 0xBC, 40), 0xC028_64BC_u32 as IoctlReq);
    }
}
