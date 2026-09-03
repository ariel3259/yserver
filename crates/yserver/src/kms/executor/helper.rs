//! Process-isolated KMS executor re-exec helper loop.

use std::{
    env,
    ffi::OsStr,
    io,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        unix::net::UnixStream,
    },
    time::Instant,
};

use super::{
    CONTROL_FD, KMS_FD, REEXEC_ARG,
    protocol::{self, HostCallReply, HostCallRequest, REQUEST_FRAME_LEN},
    take_inherited_fd,
    transport::{self, send_reply_with_fences},
};
use crate::platform::ioctl::{DRM_IOCTL_BASE, IoctlReq, iowr};

#[repr(C)]
struct DrmModeAtomic {
    flags: u32,
    count_objs: u32,
    objs_ptr: u64,
    count_props_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    reserved: u64,
    user_data: u64,
}

const DRM_IOCTL_MODE_ATOMIC: IoctlReq =
    iowr(DRM_IOCTL_BASE, 0xBC, std::mem::size_of::<DrmModeAtomic>());

#[repr(C)]
struct DrmCrtcGetSequence {
    crtc_id: u32,
    active: u32,
    sequence: u64,
    sequence_ns: i64,
}

const DRM_IOCTL_CRTC_GET_SEQUENCE: IoctlReq = iowr(
    DRM_IOCTL_BASE,
    0x3B,
    std::mem::size_of::<DrmCrtcGetSequence>(),
);

/// Called by the `yserver` binary before normal argument parsing.
///
/// `None` means this is an ordinary server invocation. `Some` means the exact
/// private re-exec marker was present and the caller must exit after returning
/// this result.
#[doc(hidden)]
pub fn run_reexec_executor_if_requested() -> Option<io::Result<()>> {
    let mut args = env::args_os();
    let _executable = args.next();
    if args.next().as_deref() != Some(OsStr::new(REEXEC_ARG)) {
        return None;
    }
    if args.next().is_some() {
        return Some(Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "executor helper accepts no additional arguments",
        )));
    }
    Some(run_executor_helper())
}

fn run_executor_helper() -> io::Result<()> {
    let control_fd = take_inherited_fd(CONTROL_FD, "executor control socket")?;
    let kms_fd = take_inherited_fd(KMS_FD, "executor KMS device")?;
    let control = UnixStream::from(control_fd);
    serve_executor_loop(&control, kms_fd.as_fd())
}

fn serve_executor_loop(control: &UnixStream, kms_fd: BorrowedFd<'_>) -> io::Result<()> {
    let mut req_buf = [0u8; REQUEST_FRAME_LEN];
    loop {
        let received = match transport::recv_frame(control, &mut req_buf) {
            Ok(rf) => {
                if rf.len == 0 {
                    // Control socket closed by supervisor (EOF) -> exit cleanly.
                    return Ok(());
                }
                rf
            }
            Err(err) => {
                if matches!(
                    err.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::BrokenPipe
                ) {
                    return Ok(());
                }
                return Err(err);
            }
        };

        let request = match protocol::decode_request(&req_buf[..received.len]) {
            Ok(req) => req,
            Err(err) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("executor helper protocol decode error: {err:?}"),
                ));
            }
        };

        let (reply, fences) = execute_host_call(kms_fd, &request);
        let rep_frame = protocol::encode_reply(&reply);
        let fence_refs: Vec<BorrowedFd<'_>> = fences.iter().map(|f| f.as_fd()).collect();
        send_reply_with_fences(control, &rep_frame, &fence_refs)?;
    }
}

fn execute_host_call(
    kms_fd: BorrowedFd<'_>,
    request: &HostCallRequest,
) -> (HostCallReply, Vec<OwnedFd>) {
    match request {
        HostCallRequest::Atomic(atomic) => {
            let mut atomic_req = DrmModeAtomic {
                flags: atomic.flags,
                count_objs: 0,
                objs_ptr: 0,
                count_props_ptr: 0,
                props_ptr: 0,
                prop_values_ptr: 0,
                reserved: 0,
                user_data: atomic.event_token.as_user_data(),
            };
            let started = Instant::now();
            // SAFETY: atomic_req is properly initialized for DRM_IOCTL_MODE_ATOMIC.
            let rc = unsafe {
                libc::ioctl(
                    kms_fd.as_raw_fd(),
                    DRM_IOCTL_MODE_ATOMIC,
                    std::ptr::addr_of_mut!(atomic_req),
                )
            };
            let helper_duration_ns = elapsed_ns(started);
            if rc == 0 {
                (
                    HostCallReply::Accepted {
                        seq: atomic.seq,
                        helper_duration_ns,
                        out_fence_count: 0,
                    },
                    Vec::new(),
                )
            } else {
                let errno = io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO);
                (
                    HostCallReply::Rejected {
                        seq: atomic.seq,
                        errno,
                        helper_duration_ns,
                    },
                    Vec::new(),
                )
            }
        }
        HostCallRequest::ClockProbe(probe) => {
            let mut get_seq = DrmCrtcGetSequence {
                crtc_id: probe.crtc_id,
                active: 0,
                sequence: 0,
                sequence_ns: 0,
            };
            let started = Instant::now();
            // SAFETY: get_seq is properly initialized for DRM_IOCTL_CRTC_GET_SEQUENCE.
            let rc = unsafe {
                libc::ioctl(
                    kms_fd.as_raw_fd(),
                    DRM_IOCTL_CRTC_GET_SEQUENCE,
                    std::ptr::addr_of_mut!(get_seq),
                )
            };
            let helper_duration_ns = elapsed_ns(started);
            if rc == 0 {
                (
                    HostCallReply::ClockProbe {
                        seq: probe.seq,
                        sequence: get_seq.sequence,
                        helper_duration_ns,
                    },
                    Vec::new(),
                )
            } else {
                let errno = io::Error::last_os_error()
                    .raw_os_error()
                    .unwrap_or(libc::EIO);
                (
                    HostCallReply::Rejected {
                        seq: probe.seq,
                        errno,
                        helper_duration_ns,
                    },
                    Vec::new(),
                )
            }
        }
    }
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
