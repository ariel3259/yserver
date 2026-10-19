//! Page-flip submission + completion drain.
//!
//! `submit_flip` atomic-commits a new FB_ID on the primary plane with
//! PAGE_FLIP_EVENT | NONBLOCK; the kernel produces a completion event
//! on the DRM fd when scanout latches the new buffer.
//!
//! `drain_events` reads pending events with `Device::receive_events()`
//! and dispatches PageFlip completions to a closure. The drm crate's
//! parser folds the kernel `user_data` field into `crtc` (preferring
//! `crtc_id` from the vblank event when present, else falling back to
//! `user_data`). The closure receives the per-CRTC handle so multi-output
//! callsites can route the completion to the right swapchain.

use std::io;

use drm::control::{
    AtomicCommitFlags, Device as ControlDevice, atomic::AtomicModeReq, crtc, framebuffer, from_u32,
};

use crate::drm::{
    Device,
    atomic::{AtomicCommitEvent, AtomicCommitKind, AtomicCommitRequest, AtomicCommitSink},
    modeset::{Output, PropMap},
};

#[cfg(target_os = "linux")]
pub(crate) type IoctlReq = libc::Ioctl;
#[cfg(not(target_os = "linux"))]
pub(crate) type IoctlReq = libc::c_ulong;

// ── DRM_IOCTL_CRTC_QUEUE_SEQUENCE plumbing ──────────────────────
//
// `drm` 0.15 / `drm-ffi` 0.9 do not wrap this ioctl; we issue it
// raw. Layouts mirror `<drm/drm.h>` exactly (kernel headers, verified
// against /usr/include/drm/drm.h on the build host). All multi-byte
// fields are little-endian on every supported target.
//
// Both flags are passed in the `flags` field; combined or'd.
pub(crate) const DRM_CRTC_SEQUENCE_RELATIVE: u32 = 0x0000_0001;
pub(crate) const DRM_CRTC_SEQUENCE_NEXT_ON_MISS: u32 = 0x0000_0002;

/// kernel `DRM_EVENT_CRTC_SEQUENCE` event type id.
pub(crate) const DRM_EVENT_CRTC_SEQUENCE: u32 = 0x03;

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct drm_crtc_queue_sequence {
    pub crtc_id: u32,
    pub flags: u32,
    /// In: target sequence. Out: actual scheduled sequence.
    pub sequence: u64,
    /// Echoed back verbatim in the resulting `drm_event_crtc_sequence`.
    pub user_data: u64,
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct drm_event_header {
    pub r#type: u32,
    pub length: u32,
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct drm_event_crtc_sequence {
    pub base: drm_event_header,
    pub user_data: u64,
    /// CLOCK_MONOTONIC nanoseconds. Signed per kernel header — we
    /// must `u64::try_from` rather than `as u64`.
    pub time_ns: i64,
    pub sequence: u64,
}

/// kernel `DRM_EVENT_FLIP_COMPLETE` event type id.
pub(crate) const DRM_EVENT_FLIP_COMPLETE: u32 = 0x02;

/// Bytes requested per read of the DRM fd (the drm crate's own buffer size).
const EVENT_READ_LEN: usize = 1024;

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct drm_event_vblank {
    pub base: drm_event_header,
    /// Echo of the atomic request's `user_data` (the commit identity).
    pub user_data: u64,
    pub tv_sec: u32,
    pub tv_usec: u32,
    pub sequence: u32,
    pub crtc_id: u32,
}

// `_IOWR('d', 0x3C, drm_crtc_queue_sequence)` expanded inline so
// the request code is a `const` we can also assert in a unit test.
//   dir = 3 (RW), type = 'd' (0x64), nr = 0x3C, size = 24
//
// Linux libc exposes the request type as `libc::Ioctl` (`c_ulong` on
// glibc, `c_int` on musl/Android; issues #15, #74). FreeBSD's libc does
// not export that alias even though `ioctl` still takes an unsigned-long
// request there, so use a tiny local alias to keep the callsite typed
// correctly on both platforms. The high bit is set (dir = RW), so the
// value overflows a signed 32-bit `Ioctl`; compute the bit pattern in
// `u32` and reinterpret-cast to preserve it.
pub(crate) const DRM_IOCTL_CRTC_QUEUE_SEQUENCE: IoctlReq = ((3u32 << 30)
    | ((std::mem::size_of::<drm_crtc_queue_sequence>() as u32) << 16)
    | (0x64u32 << 8)
    | 0x3C) as IoctlReq;

/// Queue a one-shot CRTC vblank sequence event. `crtc_id` is the
/// **raw KMS object id** (NOT a pipe index — that distinction is
/// the whole reason this helper exists; the legacy `drmWaitVBlank`
/// path used pipe indices and lost the dual-monitor case).
///
/// - `relative = true`  → kernel arms `current_msc + sequence`
///   vblanks from now; pass `sequence = 1` for "next vblank".
/// - `relative = false` → absolute target. **Always pair with
///   `NEXT_ON_MISS`** (set internally) so an already-passed target
///   fires at the next vblank instead of waiting a full 32-bit
///   counter wrap.
///
/// `user_data` is echoed verbatim in the resulting
/// `DRM_EVENT_CRTC_SEQUENCE` — we encode the stable `crtc_id` there.
///
/// Returns the kernel-assigned scheduled sequence on success.
///
/// # Errors
///
/// - `EOPNOTSUPP` on pre-4.14 kernels — caller should fall back
///   to flip-driven MSC only (idle arming disabled).
/// - `EACCES` if we no longer hold DRM master — caller must have
///   pre-gated on `scanout_allowed()`.
pub(crate) fn queue_crtc_sequence(
    device: &Device,
    crtc_id: u32,
    relative: bool,
    sequence: u64,
    user_data: u64,
) -> io::Result<u64> {
    use std::os::{fd::AsFd, unix::io::AsRawFd};

    let mut flags = DRM_CRTC_SEQUENCE_NEXT_ON_MISS;
    if relative {
        flags |= DRM_CRTC_SEQUENCE_RELATIVE;
    }
    let mut req = drm_crtc_queue_sequence {
        crtc_id,
        flags,
        sequence,
        user_data,
    };
    // SAFETY: `req` is a fully-initialised POD of the exact size the
    // kernel expects (24 bytes — pinned by the unit tests below). The
    // device fd is held alive by `device` for the duration of the
    // call; the kernel reads and writes `req` in place.
    let raw_fd = device.as_fd().as_raw_fd();
    let rc = unsafe { libc::ioctl(raw_fd, DRM_IOCTL_CRTC_QUEUE_SEQUENCE, &mut req as *mut _) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(req.sequence)
}

pub fn submit_flip(device: &Device, output: &Output, fb_id: framebuffer::Handle) -> io::Result<()> {
    submit_flip_inner(device, output, fb_id, None, None)
}

/// Atomic commit + explicit-fence flip (Phase 4.1.2.5). Used by the
/// Vulkan-fed scanout path: pass the SYNC_FD payload exported from
/// the bo's signalSemaphore as `in_fence_fd` so KMS waits for GPU
/// before scanning out, and pass `out_fence_holder` so the kernel
/// allocates a release fence we can wait on for retire.
///
/// The kernel takes ownership of `in_fence_fd` on a successful
/// commit (rc=0). On `-EBUSY` (or any other error) the caller still
/// owns the fd and must close it. `out_fence_holder` is written with
/// the new fence fd that the caller owns.
pub fn submit_flip_with_fences(
    device: &Device,
    output: &Output,
    fb_id: framebuffer::Handle,
    in_fence_fd: i32,
    out_fence_holder: &mut i32,
) -> io::Result<()> {
    submit_flip_inner(
        device,
        output,
        fb_id,
        Some(in_fence_fd),
        Some(out_fence_holder),
    )
}

/// Submit one scene flip through its owning device's recorder/timer and append
/// any prepared atomic cursor-plane deltas for this output. The legacy
/// non-rendering page-flip callers keep using `submit_flip_with_fences`.
pub(crate) fn submit_scene_flip_with_fences(
    device: &Device,
    atomic_commit_sink: &AtomicCommitSink,
    output: &Output,
    fb_id: framebuffer::Handle,
    in_fence_fd: i32,
    out_fence_holder: &mut i32,
    cursor_updates: &[crate::kms::render::platform::cursor_state::CursorPlaneUpdate],
    timing: crate::drm::async_flip::FlipTiming,
) -> io::Result<u64> {
    submit_scene_flip_with_fences_tearing(
        device,
        atomic_commit_sink,
        output,
        fb_id,
        in_fence_fd,
        out_fence_holder,
        cursor_updates,
        timing,
        crate::drm::atomic::TearingCommitContext::NONE,
    )
}

pub(crate) fn submit_scene_flip_with_fences_tearing(
    device: &Device,
    atomic_commit_sink: &AtomicCommitSink,
    output: &Output,
    fb_id: framebuffer::Handle,
    in_fence_fd: i32,
    out_fence_holder: &mut i32,
    cursor_updates: &[crate::kms::render::platform::cursor_state::CursorPlaneUpdate],
    timing: crate::drm::async_flip::FlipTiming,
    tearing_context: crate::drm::atomic::TearingCommitContext,
) -> io::Result<u64> {
    let mut request = AtomicCommitRequest::new();
    request.add_raw_property(
        output.plane.into(),
        output.plane_fb_id_prop,
        u64::from(u32::from(fb_id)),
    );
    request.add_raw_property(
        output.plane.into(),
        output.plane_crtc_id_prop,
        u64::from(u32::from(output.crtc)),
    );
    let in_fence_prop = match output.plane_in_fence_fd_prop {
        Some(prop) => prop,
        None => PropMap::for_object(device, output.plane)?.id("IN_FENCE_FD")?,
    };
    request.add_raw_property(
        output.plane.into(),
        in_fence_prop,
        in_fence_fd as i64 as u64,
    );
    if timing == crate::drm::async_flip::FlipTiming::Async {
        *out_fence_holder = -1;
    } else {
        let out_fence_prop = match output.crtc_out_fence_ptr_prop {
            Some(prop) => prop,
            None => PropMap::for_object(device, output.crtc)?.id("OUT_FENCE_PTR")?,
        };
        let out_fence_ptr = (out_fence_holder as *mut i32) as usize as u64;
        request.add_raw_property(output.crtc.into(), out_fence_prop, out_fence_ptr);
    }
    let has_cursor_updates =
        crate::kms::render::platform::cursor_commit::add_vsync_scene_cursor_updates(
            &mut request,
            cursor_updates,
            timing,
        );
    let event = AtomicCommitEvent {
        crtc: output.crtc,
        motion_timestamp: has_cursor_updates
            .then(|| {
                cursor_updates
                    .iter()
                    .find(|update| update.crtc == output.crtc)
                    .and_then(|update| update.motion_timestamp)
            })
            .flatten(),
    };
    let kind = match timing {
        crate::drm::async_flip::FlipTiming::Vsync if !has_cursor_updates => AtomicCommitKind::Scene,
        crate::drm::async_flip::FlipTiming::Vsync => AtomicCommitKind::SceneWithCursor,
        crate::drm::async_flip::FlipTiming::Async => AtomicCommitKind::Async,
    };
    let flags = AtomicCommitFlags::PAGE_FLIP_EVENT
        | AtomicCommitFlags::NONBLOCK
        | match timing {
            crate::drm::async_flip::FlipTiming::Vsync => AtomicCommitFlags::empty(),
            crate::drm::async_flip::FlipTiming::Async => AtomicCommitFlags::PAGE_FLIP_ASYNC,
        };
    atomic_commit_sink.submit_tearing_with_commit_id(
        device,
        flags,
        kind,
        request,
        &[event],
        tearing_context,
    )
}

fn submit_flip_inner(
    device: &Device,
    output: &Output,
    fb_id: framebuffer::Handle,
    in_fence_fd: Option<i32>,
    out_fence_holder: Option<&mut i32>,
) -> io::Result<()> {
    let mut req = AtomicModeReq::new();
    req.add_raw_property(
        output.plane.into(),
        output.plane_fb_id_prop,
        u64::from(u32::from(fb_id)),
    );
    req.add_raw_property(
        output.plane.into(),
        output.plane_crtc_id_prop,
        u64::from(u32::from(output.crtc)),
    );

    if let Some(fd) = in_fence_fd {
        // IN_FENCE_FD is a plane property. Its value is the fence fd
        // (sign-extended to u64; -1 means "no fence", which differs
        // from "absent").
        let prop = match output.plane_in_fence_fd_prop {
            Some(prop) => prop,
            None => PropMap::for_object(device, output.plane)?.id("IN_FENCE_FD")?,
        };
        req.add_raw_property(output.plane.into(), prop, fd as i64 as u64);
    }
    if let Some(holder) = out_fence_holder {
        // OUT_FENCE_PTR is a CRTC property. Its value is a userspace
        // pointer (cast to u64) where the kernel writes the freshly
        // allocated fence fd on a successful commit.
        let prop = match output.crtc_out_fence_ptr_prop {
            Some(prop) => prop,
            None => PropMap::for_object(device, output.crtc)?.id("OUT_FENCE_PTR")?,
        };
        let ptr_value = (holder as *mut i32) as usize as u64;
        req.add_raw_property(output.crtc.into(), prop, ptr_value);
    }

    device.atomic_commit(
        AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK,
        req,
    )
}

/// Returns without reading when no event is queued: the DRM fd is blocking,
/// and readiness the core loop polled can already have been consumed by
/// `discard_old_drm_events_after_all_off` earlier in the same iteration.
///
/// `on_advance` receives the page-flip `user_data`, the kernel echo of the
/// commit identity that requested the event (0 when the commit had none).
pub fn drain_events<A, S>(device: &Device, mut on_advance: A, mut on_sequence: S) -> io::Result<()>
where
    A: FnMut(crtc::Handle, u32, std::time::Duration, u64),
    S: FnMut(u64, i64, u64),
{
    if !event_pending(device)? {
        return Ok(());
    }
    let mut buf = [0u8; EVENT_READ_LEN];
    let amount = read_events(device, &mut buf)?;
    dispatch_events(&buf[..amount], &mut on_advance, &mut on_sequence);
    Ok(())
}

/// One read of the DRM fd, as the drm crate's `receive_events` performs it.
/// The raw stream is parsed here because that crate keeps only the CRTC of a
/// page-flip completion and drops the `user_data` that names the commit.
fn read_events(device: &Device, buf: &mut [u8]) -> io::Result<usize> {
    use std::os::{fd::AsFd, unix::io::AsRawFd};

    // SAFETY: `buf` is a live writable region of `buf.len()` bytes for the
    // duration of the call; the device fd is held open by `device`.
    let read = unsafe {
        libc::read(
            device.as_fd().as_raw_fd(),
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    usize::try_from(read).map_err(|_| io::Error::last_os_error())
}

/// Zero-timeout poll of the DRM fd. Sound as a guard for the blocking read
/// because this process is the fd's only event reader.
fn event_pending(device: &Device) -> io::Result<bool> {
    use std::os::{fd::AsFd, unix::io::AsRawFd};

    let mut poll_fd = libc::pollfd {
        fd: device.as_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one initialized pollfd, live for the call; the fd is held
        // open by `device`.
        let ready = unsafe { libc::poll(&raw mut poll_fd, 1, 0) };
        if ready >= 0 {
            return Ok(poll_fd.revents & libc::POLLIN != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Dispatch every complete event in one read's bytes.
///
/// - `DRM_EVENT_FLIP_COMPLETE` → `on_advance(crtc, frame /*msc*/, duration /*ust*/,
///   user_data)`. The MSC/UST are forwarded for Present vblank pacing. The
///   `user_data` is the commit identity the kernel echoes from the atomic
///   request, so the caller can match the completion to its submission.
/// - `DRM_EVENT_CRTC_SEQUENCE` (type==3, length==32) → `on_sequence(user_data_u64,
///   time_ns_i64, sequence_u64)`. **Raw**: `time_ns` is signed and not yet
///   validated; `user_data` is echoed verbatim from the arm call — the
///   backend encodes the crtc_id in the low 32 bits and (for absolute
///   per-target arms) a discriminating tag in the high bit, so the full
///   width must reach the caller undivided. The caller does clear-arm
///   BEFORE any drop on validity check.
/// - Everything else (`Vblank`, other types): dropped.
///
/// Factored out of [`drain_events`] so the per-event routing is unit-testable
/// without a real DRM fd (synthetic raw byte buffers can be passed directly).
fn dispatch_events<A, S>(bytes: &[u8], on_advance: &mut A, on_sequence: &mut S)
where
    A: FnMut(crtc::Handle, u32, std::time::Duration, u64),
    S: FnMut(u64, i64, u64),
{
    let header_len = std::mem::size_of::<drm_event_header>();
    let mut offset = 0;
    while bytes.len() - offset >= header_len {
        // SAFETY: the loop guard proves a whole header is readable at
        // `offset`; `read_unaligned` tolerates the Vec's alignment.
        let header: drm_event_header = unsafe {
            std::ptr::read_unaligned(bytes[offset..].as_ptr().cast::<drm_event_header>())
        };
        let length = header.length as usize;
        // A length that is too short to hold its own header, or that runs
        // past the read, cannot be walked past: stop at this event.
        if length < header_len || length > bytes.len() - offset {
            return;
        }
        let event = &bytes[offset..offset + length];
        match header.r#type {
            DRM_EVENT_FLIP_COMPLETE => dispatch_flip_complete(event, on_advance),
            DRM_EVENT_CRTC_SEQUENCE => dispatch_crtc_sequence(event, on_sequence),
            _ => {}
        }
        offset += length;
    }
}

fn dispatch_flip_complete<A>(event: &[u8], on_advance: &mut A)
where
    A: FnMut(crtc::Handle, u32, std::time::Duration, u64),
{
    if event.len() < std::mem::size_of::<drm_event_vblank>() {
        return;
    }
    // SAFETY: the length check proves the whole struct is readable within
    // `event`; `read_unaligned` tolerates the slice's alignment.
    let vblank: drm_event_vblank =
        unsafe { std::ptr::read_unaligned(event.as_ptr().cast::<drm_event_vblank>()) };
    // Same CRTC resolution as the drm crate: the kernel fills `crtc_id`, and
    // `user_data` names the CRTC only for events that leave it zero.
    let crtc_raw = if vblank.crtc_id != 0 {
        vblank.crtc_id
    } else {
        vblank.user_data as u32
    };
    let Some(crtc) = from_u32::<crtc::Handle>(crtc_raw) else {
        return;
    };
    let duration = std::time::Duration::from_secs(u64::from(vblank.tv_sec))
        + std::time::Duration::from_micros(u64::from(vblank.tv_usec));
    on_advance(crtc, vblank.sequence, duration, vblank.user_data);
}

fn dispatch_crtc_sequence<S>(event: &[u8], on_sequence: &mut S)
where
    S: FnMut(u64, i64, u64),
{
    if event.len() != std::mem::size_of::<drm_event_crtc_sequence>() {
        return;
    }
    // SAFETY: the length check proves the whole struct is readable within
    // `event`; `read_unaligned` tolerates the slice's alignment.
    let ev: drm_event_crtc_sequence =
        unsafe { std::ptr::read_unaligned(event.as_ptr().cast::<drm_event_crtc_sequence>()) };
    // Pass user_data through undivided — the caller decodes the
    // low-32-bits crtc_id and (for absolute arms) the high tag bit.
    on_sequence(ev.user_data, ev.time_ns, ev.sequence);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use drm::control::{crtc, from_u32};

    use super::{DRM_EVENT_CRTC_SEQUENCE, DRM_EVENT_FLIP_COMPLETE, dispatch_events};

    /// `struct drm_event_vblank` bytes: type, length, user_data, tv_sec,
    /// tv_usec, sequence, crtc_id.
    fn flip_complete_bytes(user_data: u64, sec: u32, usec: u32, seq: u32, crtc_id: u32) -> Vec<u8> {
        let mut raw = Vec::with_capacity(32);
        raw.extend_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        raw.extend_from_slice(&32u32.to_ne_bytes());
        raw.extend_from_slice(&user_data.to_ne_bytes());
        raw.extend_from_slice(&sec.to_ne_bytes());
        raw.extend_from_slice(&usec.to_ne_bytes());
        raw.extend_from_slice(&seq.to_ne_bytes());
        raw.extend_from_slice(&crtc_id.to_ne_bytes());
        raw
    }

    fn sequence_bytes(user_data: u64, time_ns: i64, sequence: u64) -> Vec<u8> {
        let mut raw = Vec::with_capacity(32);
        raw.extend_from_slice(&DRM_EVENT_CRTC_SEQUENCE.to_ne_bytes());
        raw.extend_from_slice(&32u32.to_ne_bytes());
        raw.extend_from_slice(&user_data.to_ne_bytes());
        raw.extend_from_slice(&time_ns.to_ne_bytes());
        raw.extend_from_slice(&sequence.to_ne_bytes());
        raw
    }

    type FlipCall = (crtc::Handle, u32, Duration, u64);

    fn dispatch(bytes: &[u8]) -> (Vec<FlipCall>, Vec<(u64, i64, u64)>) {
        let mut flips = Vec::new();
        let mut sequences = Vec::new();
        dispatch_events(
            bytes,
            &mut |c, frame, dur, user_data| flips.push((c, frame, dur, user_data)),
            &mut |user_data, t, s| sequences.push((user_data, t, s)),
        );
        (flips, sequences)
    }

    #[test]
    fn dispatch_passes_crtc_handle_and_commit_identity_for_page_flip() {
        let handle: crtc::Handle = from_u32(42).expect("non-zero raw handle");
        let bytes = flip_complete_bytes(0xC0FFEE, 3, 500, 77, 42);

        let (flips, sequences) = dispatch(&bytes);

        assert_eq!(
            flips,
            vec![(handle, 77, Duration::new(3, 500_000), 0xC0FFEE)]
        );
        assert!(sequences.is_empty());
    }

    #[test]
    fn dispatch_falls_back_to_user_data_crtc_when_crtc_id_is_zero() {
        let handle: crtc::Handle = from_u32(9).expect("non-zero raw handle");
        let bytes = flip_complete_bytes(9, 0, 0, 1, 0);

        let (flips, _) = dispatch(&bytes);

        assert_eq!(flips, vec![(handle, 1, Duration::ZERO, 9)]);
    }

    #[test]
    fn dispatch_ignores_empty_read() {
        let (flips, sequences) = dispatch(&[]);
        assert!(flips.is_empty());
        assert!(sequences.is_empty());
    }

    #[test]
    fn dispatch_walks_every_event_in_one_read() {
        let handle: crtc::Handle = from_u32(42).expect("non-zero raw handle");
        let mut bytes = flip_complete_bytes(5, 0, 0, 1, 42);
        bytes.extend(sequence_bytes(0x42, 7, 8));
        bytes.extend(flip_complete_bytes(6, 0, 0, 2, 42));

        let (flips, sequences) = dispatch(&bytes);

        assert_eq!(
            flips,
            vec![
                (handle, 1, Duration::ZERO, 5),
                (handle, 2, Duration::ZERO, 6)
            ]
        );
        assert_eq!(sequences, vec![(0x42, 7, 8)]);
    }

    #[test]
    fn dispatch_stops_at_an_event_that_overruns_the_read() {
        let mut bytes = flip_complete_bytes(5, 0, 0, 1, 42);
        // Header claims 64 bytes but only 8 are present.
        bytes.extend_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
        bytes.extend_from_slice(&64u32.to_ne_bytes());

        let (flips, _) = dispatch(&bytes);

        assert_eq!(flips.len(), 1);
    }

    #[test]
    fn drm_event_vblank_struct_is_32_bytes() {
        // drm.h: struct drm_event base (8B) + __u64 user_data + __u32 tv_sec
        // + __u32 tv_usec + __u32 sequence + __u32 crtc_id = 8 + 8 + 16 = 32.
        assert_eq!(std::mem::size_of::<super::drm_event_vblank>(), 32);
        assert_eq!(std::mem::align_of::<super::drm_event_vblank>(), 8);
    }

    #[test]
    fn drm_crtc_queue_sequence_struct_is_24_bytes() {
        // drm.h: __u32 crtc_id; __u32 flags; __u64 sequence; __u64 user_data;
        // → 4+4+8+8 = 24 bytes.
        assert_eq!(std::mem::size_of::<super::drm_crtc_queue_sequence>(), 24);
        assert_eq!(std::mem::align_of::<super::drm_crtc_queue_sequence>(), 8);
    }

    #[test]
    fn drm_event_crtc_sequence_struct_is_32_bytes() {
        // drm.h: struct drm_event base (8B) + __u64 user_data + __s64 time_ns
        // + __u64 sequence = 8 + 8 + 8 + 8 = 32.
        assert_eq!(std::mem::size_of::<super::drm_event_crtc_sequence>(), 32);
        assert_eq!(std::mem::align_of::<super::drm_event_crtc_sequence>(), 8);
    }

    #[test]
    fn drm_crtc_queue_sequence_ioctl_request_code() {
        // _IOWR('d' /*0x64*/, 0x3C, drm_crtc_queue_sequence):
        //   (3 << 30) | (24 << 16) | (0x64 << 8) | 0x3C = 0xC018643C
        assert_eq!(
            super::DRM_IOCTL_CRTC_QUEUE_SEQUENCE,
            0xC018_643C_u32 as super::IoctlReq
        );
    }

    #[test]
    fn queue_sequence_flags_absolute_with_next_on_miss() {
        use super::{DRM_CRTC_SEQUENCE_NEXT_ON_MISS, drm_crtc_queue_sequence};
        let req = drm_crtc_queue_sequence {
            crtc_id: 0x42,
            flags: DRM_CRTC_SEQUENCE_NEXT_ON_MISS,
            sequence: 0x1234_5678_9ABC_DEF0,
            user_data: 0x42,
        };
        assert_eq!(req.flags & 1, 0, "RELATIVE bit clear for absolute target");
        assert_eq!(req.flags & 2, 2, "NEXT_ON_MISS set");
    }

    #[test]
    fn dispatch_decodes_crtc_sequence() {
        let bytes = sequence_bytes(0xCAFE_BABE_0000_0042, 1_234_567_890, 9_999);

        let (flips, sequences) = dispatch(&bytes);

        assert!(
            flips.is_empty(),
            "sequence event must NOT route through advance callback"
        );
        // Full 64-bit user_data must reach the caller undivided — the
        // consumer (KmsBackend::on_crtc_sequence_event) decodes both the
        // low-32-bits crtc_id and the high tag bit itself.
        assert_eq!(
            sequences,
            vec![(0xCAFE_BABE_0000_0042u64, 1_234_567_890_i64, 9_999u64)]
        );
    }

    #[test]
    fn dispatch_ignores_wrong_length_sequence_event() {
        let mut bytes = sequence_bytes(1, 2, 3);
        bytes.truncate(16);
        bytes[4..8].copy_from_slice(&16u32.to_ne_bytes());

        let (flips, sequences) = dispatch(&bytes);

        assert!(flips.is_empty());
        assert!(sequences.is_empty());
    }

    #[test]
    fn dispatch_ignores_unknown_event_type() {
        let mut bytes = vec![0u8; 32];
        bytes[..4].copy_from_slice(&99u32.to_ne_bytes());
        bytes[4..8].copy_from_slice(&32u32.to_ne_bytes());

        let (flips, sequences) = dispatch(&bytes);

        assert!(flips.is_empty());
        assert!(sequences.is_empty());
    }

    /// A `Device` over one end of a socket pair: `read` on it behaves like the
    /// DRM fd's (blocks while empty), and the peer injects raw event bytes.
    fn device_with_event_feed() -> (crate::drm::Device, std::os::unix::net::UnixStream) {
        let (ours, feed) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let device =
            crate::drm::Device::from_inherited_kms_fd(std::os::fd::OwnedFd::from(ours), "<test>");
        (device, feed)
    }

    /// Readiness reported by the core loop's poll can be consumed by
    /// `discard_old_drm_events_after_all_off` earlier in the same iteration;
    /// the later drain must see an empty fd and return, not block in `read`.
    #[test]
    fn drain_events_returns_when_no_event_is_pending() {
        let (device, feed) = device_with_event_feed();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut flips, mut sequences) = (0usize, 0usize);
            let result =
                super::drain_events(&device, |_, _, _, _| flips += 1, |_, _, _| sequences += 1);
            let _ = tx.send((result.is_ok(), flips + sequences));
        });
        let outcome = rx.recv_timeout(Duration::from_secs(2));
        drop(feed);
        assert_eq!(
            outcome.ok(),
            Some((true, 0)),
            "drain blocked on an empty DRM fd"
        );
    }

    #[test]
    fn drain_events_dispatches_a_pending_flip_complete() {
        use std::io::Write;
        let (device, mut feed) = device_with_event_feed();
        feed.write_all(&flip_complete_bytes(0, 3, 500, 77, 42))
            .expect("inject event");

        let mut seen: Vec<(crtc::Handle, u32, Duration, u64)> = Vec::new();
        super::drain_events(
            &device,
            |c, f, d, user_data| seen.push((c, f, d, user_data)),
            |_, _, _| {},
        )
        .expect("drain");
        let handle: crtc::Handle = from_u32(42).expect("non-zero raw handle");
        assert_eq!(seen, vec![(handle, 77, Duration::new(3, 500_000), 0)]);
    }

    #[test]
    fn drain_events_delivers_the_commit_identity_of_a_flip_complete() {
        use std::io::Write;
        let (device, mut feed) = device_with_event_feed();
        feed.write_all(&flip_complete_bytes(0x2A, 0, 0, 5, 42))
            .expect("inject event");

        let mut user_data_seen = Vec::new();
        super::drain_events(
            &device,
            |_, _, _, user_data| user_data_seen.push(user_data),
            |_, _, _| {},
        )
        .expect("drain");
        assert_eq!(user_data_seen, vec![0x2A]);
    }
}
