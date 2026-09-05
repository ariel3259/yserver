//! Test support and stub helper for executor substrate integration testing.

use std::{
    io,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd},
        unix::net::UnixStream,
    },
    time::{Duration, Instant},
};

use super::{
    CONTROL_FD, HostCallEvent, HostCallOutcome, HostCallReservation, KMS_FD, KmsIoExecutor,
    STUB_ARG_PREFIX, SendError, SubmittingProof, ValidationLease, executor_executable, protocol,
    spawn_internal, take_inherited_fd, transport,
};
use crate::kms::owner::identity::IncarnationId;

/// Scripted reply shapes for reply-validation integration tests.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum ScriptedReply {
    Accepted { mask: u32, fds: usize },
    StaleCorrelation,
}

/// Simulated helper behaviors for testing supervisor isolation and error paths.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum StubBehaviour {
    NeverReply,
    ExitBeforeReply,
    RejectWith(i32),
    IgnoreTermination,
    AcceptAfter(Duration),
    Scripted(ScriptedReply),
    AcceptAfterReturningInheritedFd {
        delay: Duration,
        ignore_termination: bool,
    },
    ReplyWithForeignCorrelation,
    RejectWithRepeatedly(i32),
    AcceptProbeWith(u64),
    ReplyWithWrongFamily,
    AcceptDeclaringMissingFence,
    ReplyTwiceWith(i32),
}

impl StubBehaviour {
    pub(crate) fn to_arg_string(self) -> String {
        match self {
            Self::NeverReply => "never-reply".to_string(),
            Self::ExitBeforeReply => "exit-before-reply".to_string(),
            Self::RejectWith(errno) => format!("reject:{errno}"),
            Self::IgnoreTermination => "ignore-termination".to_string(),
            Self::AcceptAfter(duration) => format!("accept-after:{}", duration.as_millis()),
            Self::Scripted(ScriptedReply::Accepted { mask, fds }) => {
                format!("scripted-accepted:{mask}:{fds}")
            }
            Self::Scripted(ScriptedReply::StaleCorrelation) => {
                "scripted-stale-correlation".to_string()
            }
            Self::AcceptAfterReturningInheritedFd {
                delay,
                ignore_termination,
            } => {
                format!(
                    "accept-fd-after:{}:{}",
                    delay.as_millis(),
                    if ignore_termination { 1 } else { 0 }
                )
            }
            Self::ReplyWithForeignCorrelation => "reply-foreign-correlation".to_string(),
            Self::RejectWithRepeatedly(errno) => format!("reject-repeatedly:{errno}"),
            Self::AcceptProbeWith(seq) => format!("accept-probe:{seq}"),
            Self::ReplyWithWrongFamily => "reply-wrong-family".to_string(),
            Self::AcceptDeclaringMissingFence => "accept-declaring-missing-fence".to_string(),
            Self::ReplyTwiceWith(errno) => format!("reply-twice:{errno}"),
        }
    }

    pub(crate) fn from_arg_str(s: &str) -> Option<Self> {
        if s == "never-reply" {
            Some(Self::NeverReply)
        } else if s == "exit-before-reply" {
            Some(Self::ExitBeforeReply)
        } else if let Some(errno_str) = s.strip_prefix("reject:") {
            errno_str.parse::<i32>().ok().map(Self::RejectWith)
        } else if s == "ignore-termination" {
            Some(Self::IgnoreTermination)
        } else if let Some(ms_str) = s.strip_prefix("accept-after:") {
            ms_str
                .parse::<u64>()
                .ok()
                .map(|ms| Self::AcceptAfter(Duration::from_millis(ms)))
        } else if let Some(rest) = s.strip_prefix("scripted-accepted:") {
            let mut parts = rest.split(':');
            let mask = parts.next()?.parse::<u32>().ok()?;
            let fds = parts.next()?.parse::<usize>().ok()?;
            Some(Self::Scripted(ScriptedReply::Accepted { mask, fds }))
        } else if s == "scripted-stale-correlation" {
            Some(Self::Scripted(ScriptedReply::StaleCorrelation))
        } else if let Some(rest) = s.strip_prefix("accept-fd-after:") {
            let mut parts = rest.split(':');
            let ms = parts.next()?.parse::<u64>().ok()?;
            let ign = parts.next()?.parse::<u8>().ok()? == 1;
            Some(Self::AcceptAfterReturningInheritedFd {
                delay: Duration::from_millis(ms),
                ignore_termination: ign,
            })
        } else if s == "reply-foreign-correlation" {
            Some(Self::ReplyWithForeignCorrelation)
        } else if let Some(errno_str) = s.strip_prefix("reject-repeatedly:") {
            errno_str
                .parse::<i32>()
                .ok()
                .map(Self::RejectWithRepeatedly)
        } else if let Some(seq_str) = s.strip_prefix("accept-probe:") {
            seq_str.parse::<u64>().ok().map(Self::AcceptProbeWith)
        } else if s == "reply-wrong-family" {
            Some(Self::ReplyWithWrongFamily)
        } else if s == "accept-declaring-missing-fence" {
            Some(Self::AcceptDeclaringMissingFence)
        } else if let Some(errno_str) = s.strip_prefix("reply-twice:") {
            errno_str.parse::<i32>().ok().map(Self::ReplyTwiceWith)
        } else {
            None
        }
    }
}

/// Spawn a process-isolated stub helper configured with `behaviour`.
#[doc(hidden)]
pub fn spawn_stub_helper(behaviour: StubBehaviour) -> io::Result<KmsIoExecutor> {
    let dummy_file = std::fs::File::open("/dev/null")?;
    let exe = executor_executable()?;
    spawn_internal(
        &exe,
        dummy_file.as_fd(),
        IncarnationId::first(),
        Some(behaviour),
    )
}

/// Spawn a process-isolated stub helper configured with `behaviour` and an inherited event/KMS fd.
#[doc(hidden)]
pub fn spawn_stub_helper_with_event_fd(
    behaviour: StubBehaviour,
    event_fd: impl AsFd,
) -> io::Result<KmsIoExecutor> {
    let exe = executor_executable()?;
    let raw = event_fd.as_fd().as_raw_fd();
    let readable_alias = std::fs::OpenOptions::new()
        .read(true)
        .open(format!("/proc/self/fd/{raw}"))
        .ok();
    let fd_to_pass = match &readable_alias {
        Some(f) => f.as_fd(),
        None => event_fd.as_fd(),
    };
    spawn_internal(&exe, fd_to_pass, IncarnationId::first(), Some(behaviour))
}

/// Write synthetic event bytes into the pipe/descriptor standing in for the DRM event stream.
#[doc(hidden)]
pub fn write_synthetic_event(fd: &impl std::os::fd::AsRawFd, count: usize) {
    let raw = fd.as_raw_fd();
    let data = vec![0x42u8; count];

    // Attempt direct write in case the descriptor is writable.
    // SAFETY: data points to count valid bytes.
    let written = unsafe { libc::write(raw, data.as_ptr().cast(), count) };
    if written == count as isize {
        return;
    }

    // For a read-only descriptor (such as the read end of a pipe), open the
    // write end of the same underlying file description via procfs or fdescfs.
    use std::io::Write;
    let proc_path = format!("/proc/self/fd/{raw}");
    if let Ok(mut writer) = std::fs::OpenOptions::new().write(true).open(&proc_path) {
        writer
            .write_all(&data)
            .expect("write synthetic event via /proc/self/fd");
        return;
    }
    let dev_path = format!("/dev/fd/{raw}");
    if let Ok(mut writer) = std::fs::OpenOptions::new().write(true).open(&dev_path) {
        writer
            .write_all(&data)
            .expect("write synthetic event via /dev/fd");
        return;
    }

    panic!(
        "write_synthetic_event failed for fd {raw}: {}",
        io::Error::last_os_error()
    );
}

/// Query the number of readable bytes queued on a descriptor using FIONREAD.
#[doc(hidden)]
pub fn readable_bytes(fd: &impl std::os::fd::AsRawFd) -> usize {
    let raw = fd.as_raw_fd();
    let mut available: libc::c_int = 0;
    // SAFETY: FIONREAD takes a pointer to c_int to write available byte count.
    let rc = unsafe { libc::ioctl(raw, libc::FIONREAD as _, &mut available) };
    if rc < 0 {
        panic!(
            "readable_bytes ioctl(FIONREAD) failed: {}",
            io::Error::last_os_error()
        );
    }
    available.max(0) as usize
}

/// Called by the `yserver` binary before normal argument parsing.
///
/// Returns `Some(...)` if a stub helper invocation was requested.
#[doc(hidden)]
pub fn run_stub_helper_if_requested() -> Option<io::Result<()>> {
    let mut args = std::env::args_os();
    let _executable = args.next();
    let first = args.next()?;
    let first_str = first.to_str()?;
    if let Some(rest) = first_str.strip_prefix(STUB_ARG_PREFIX) {
        let behaviour = match StubBehaviour::from_arg_str(rest) {
            Some(b) => b,
            None => {
                return Some(Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown stub behaviour: {rest}"),
                )));
            }
        };
        Some(run_stub_helper(behaviour))
    } else if first_str == "--yserver-internal-kms-executor-stub" {
        let second = match args.next() {
            Some(s) => s,
            None => {
                return Some(Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "missing stub behaviour argument",
                )));
            }
        };
        let second_str = match second.to_str() {
            Some(s) => s,
            None => {
                return Some(Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid stub behaviour argument encoding",
                )));
            }
        };
        let behaviour = match StubBehaviour::from_arg_str(second_str) {
            Some(b) => b,
            None => {
                return Some(Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown stub behaviour: {second_str}"),
                )));
            }
        };
        Some(run_stub_helper(behaviour))
    } else {
        None
    }
}

fn run_stub_helper(behaviour: StubBehaviour) -> io::Result<()> {
    let control_fd = take_inherited_fd(CONTROL_FD, "executor control")?;
    let _kms_fd = take_inherited_fd(KMS_FD, "executor KMS")?;
    let control = UnixStream::from(control_fd);

    match behaviour {
        StubBehaviour::NeverReply => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        StubBehaviour::ExitBeforeReply => {
            std::process::exit(0);
        }
        StubBehaviour::RejectWith(errno) => {
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let reply = protocol::HostCallReply::Rejected {
                    correlation: req.correlation(),
                    errno,
                    helper_duration_ns: 1_000_000,
                    unexpected_fence_output: false,
                };
                let rep_frame = protocol::encode_reply(&reply);
                transport::send_frame(&control, &rep_frame)?;
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
        StubBehaviour::IgnoreTermination => {
            #[cfg(unix)]
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        StubBehaviour::AcceptAfter(delay) => {
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let start = Instant::now();
                std::thread::sleep(delay);
                let helper_duration_ns =
                    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                let reply = protocol::HostCallReply::Accepted {
                    correlation: req.correlation(),
                    helper_duration_ns,
                    out_fence_mask: 0,
                };
                let rep_frame = protocol::encode_reply(&reply);
                transport::send_frame(&control, &rep_frame)?;
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
        StubBehaviour::Scripted(reply) => {
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                match reply {
                    ScriptedReply::Accepted { mask, fds } => {
                        let rep = protocol::HostCallReply::Accepted {
                            correlation: req.correlation(),
                            helper_duration_ns: 1_000_000,
                            out_fence_mask: mask,
                        };
                        let rep_frame = protocol::encode_reply(&rep);
                        let mut dummy_files = Vec::new();
                        for _ in 0..fds {
                            dummy_files.push(std::fs::File::open("/dev/null")?);
                        }
                        let fence_refs: Vec<BorrowedFd<'_>> =
                            dummy_files.iter().map(|f| f.as_fd()).collect();
                        transport::send_reply_with_fences(&control, &rep_frame, &fence_refs)?;
                    }
                    ScriptedReply::StaleCorrelation => {
                        let stale_correlation = match req.correlation() {
                            HostCallCorrelation::Atomic {
                                seq,
                                incarnation,
                                lifecycle_epoch,
                                transition,
                                commit,
                                event_token,
                            } => HostCallCorrelation::Atomic {
                                seq: RequestSeq::for_tests(seq.get().wrapping_add(100)),
                                incarnation,
                                lifecycle_epoch,
                                transition,
                                commit,
                                event_token,
                            },
                            HostCallCorrelation::ClockProbe {
                                seq,
                                incarnation,
                                lifecycle_epoch,
                                topology_generation,
                                hardware_crtc,
                                clock_epoch,
                                probe,
                            } => HostCallCorrelation::ClockProbe {
                                seq: RequestSeq::for_tests(seq.get().wrapping_add(100)),
                                incarnation,
                                lifecycle_epoch,
                                topology_generation,
                                hardware_crtc,
                                clock_epoch,
                                probe,
                            },
                        };
                        let rep = protocol::HostCallReply::Accepted {
                            correlation: stale_correlation,
                            helper_duration_ns: 1_000_000,
                            out_fence_mask: 0,
                        };
                        let rep_frame = protocol::encode_reply(&rep);
                        transport::send_reply_with_fences(&control, &rep_frame, &[])?;
                    }
                }
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
        StubBehaviour::AcceptAfterReturningInheritedFd {
            delay,
            ignore_termination,
        } => {
            if ignore_termination {
                #[cfg(unix)]
                unsafe {
                    libc::signal(libc::SIGTERM, libc::SIG_IGN);
                }
            }
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let start = Instant::now();
                std::thread::sleep(delay);
                let helper_duration_ns =
                    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                let reply = protocol::HostCallReply::Accepted {
                    correlation: req.correlation(),
                    helper_duration_ns,
                    out_fence_mask: 1,
                };
                let rep_frame = protocol::encode_reply(&reply);
                let dup_fd = unsafe { libc::dup(_kms_fd.as_raw_fd()) };
                if dup_fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                drop(_kms_fd);
                let dup_file = unsafe { std::fs::File::from_raw_fd(dup_fd) };
                transport::send_reply_with_fences(&control, &rep_frame, &[dup_file.as_fd()])?;
                drop(dup_file);
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
        StubBehaviour::ReplyWithForeignCorrelation => {
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let foreign_correlation = match req.correlation() {
                    HostCallCorrelation::Atomic {
                        seq,
                        incarnation,
                        transition,
                        commit,
                        event_token,
                        ..
                    } => HostCallCorrelation::Atomic {
                        seq,
                        incarnation,
                        lifecycle_epoch: LifecycleEpochId::from_raw(u64::MAX),
                        transition,
                        commit,
                        event_token,
                    },
                    HostCallCorrelation::ClockProbe {
                        seq,
                        incarnation,
                        topology_generation,
                        hardware_crtc,
                        clock_epoch,
                        probe,
                        ..
                    } => HostCallCorrelation::ClockProbe {
                        seq,
                        incarnation,
                        lifecycle_epoch: LifecycleEpochId::from_raw(u64::MAX),
                        topology_generation,
                        hardware_crtc,
                        clock_epoch,
                        probe,
                    },
                };
                let reply = protocol::HostCallReply::Accepted {
                    correlation: foreign_correlation,
                    helper_duration_ns: 1_000_000,
                    out_fence_mask: 0,
                };
                let rep_frame = protocol::encode_reply(&reply);
                transport::send_frame(&control, &rep_frame)?;
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
        StubBehaviour::RejectWithRepeatedly(errno) => {
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            loop {
                let received = transport::recv_frame(&control, &mut req_buf)?;
                if received.len == 0 {
                    return Ok(());
                }
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let reply = protocol::HostCallReply::Rejected {
                    correlation: req.correlation(),
                    errno,
                    helper_duration_ns: 1_000_000,
                    unexpected_fence_output: false,
                };
                let rep_frame = protocol::encode_reply(&reply);
                transport::send_frame(&control, &rep_frame)?;
            }
        }
        StubBehaviour::AcceptProbeWith(sequence) => {
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let reply = protocol::HostCallReply::ProbeAccepted {
                    correlation: req.correlation(),
                    sequence,
                    helper_duration_ns: 1_000_000,
                };
                let rep_frame = protocol::encode_reply(&reply);
                transport::send_frame(&control, &rep_frame)?;
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
        StubBehaviour::ReplyWithWrongFamily => {
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let reply = protocol::HostCallReply::Accepted {
                    correlation: req.correlation(),
                    helper_duration_ns: 1_000_000,
                    out_fence_mask: 0,
                };
                let rep_frame = protocol::encode_reply(&reply);
                transport::send_frame(&control, &rep_frame)?;
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
        StubBehaviour::AcceptDeclaringMissingFence => {
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let reply = protocol::HostCallReply::Accepted {
                    correlation: req.correlation(),
                    helper_duration_ns: 1_000_000,
                    out_fence_mask: 1,
                };
                let rep_frame = protocol::encode_reply(&reply);
                transport::send_frame(&control, &rep_frame)?;
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
        StubBehaviour::ReplyTwiceWith(errno) => {
            let mut req_buf = vec![0u8; protocol::MAX_REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let reply = protocol::HostCallReply::Rejected {
                    correlation: req.correlation(),
                    errno,
                    helper_duration_ns: 1_000_000,
                    unexpected_fence_output: false,
                };
                let rep_frame = protocol::encode_reply(&reply);
                transport::send_frame(&control, &rep_frame)?;
                transport::send_frame(&control, &rep_frame)?;
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
    }
}

/// Test device descriptor holder and constructor set.
#[derive(Debug)]
#[doc(hidden)]
pub struct TestDevice {
    file: std::fs::File,
    is_stub: bool,
}

impl TestDevice {
    pub fn open_stub() -> Self {
        let file = std::fs::File::open("/dev/null").expect("open /dev/null");
        Self {
            file,
            is_stub: true,
        }
    }

    pub fn open_never_a_drm_device() -> Self {
        let file = std::fs::File::open("/dev/null").expect("open /dev/null");
        Self {
            file,
            is_stub: false,
        }
    }

    pub fn open_real_drm_or_ignore() -> Option<Self> {
        for minor in 0..64 {
            let path = format!("/dev/dri/card{minor}");
            if let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
            {
                return Some(Self {
                    file,
                    is_stub: false,
                });
            }
        }
        None
    }

    pub fn is_stub(&self) -> bool {
        self.is_stub
    }
}

impl AsFd for TestDevice {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

/// Spawn a real helper process connected to `device`.
#[doc(hidden)]
pub fn spawn_real_helper_for_tests(device: &TestDevice) -> KmsIoExecutor {
    let exe = executor_executable().expect("executor executable");
    spawn_internal(&exe, device.as_fd(), IncarnationId::first(), None).expect("spawn real helper")
}

/// Spawn a scripted helper answering with `reply`.
#[doc(hidden)]
pub fn spawn_scripted_helper_for_tests(reply: ScriptedReply) -> KmsIoExecutor {
    spawn_stub_helper(StubBehaviour::Scripted(reply)).expect("spawn scripted helper")
}

/// Synchronously dispatch a request and wait for the outcome in tests.
#[doc(hidden)]
pub fn dispatch_and_wait_for_tests(
    executor: &mut KmsIoExecutor,
    request: &HostCallRequest,
) -> HostCallOutcome {
    let reservation = match request.class().is_validation() {
        true => HostCallReservation::Validation(ValidationLease::for_tests()),
        false => HostCallReservation::Submitting(SubmittingProof::for_tests()),
    };
    if let Err(err) = executor.send(request, reservation) {
        if let Some(event) = executor.poll_reply() {
            return match event {
                HostCallEvent::Outcome { outcome, .. } => outcome,
                HostCallEvent::LateReply { outcome, .. } => outcome,
            };
        }
        return match err {
            SendError::Reaped => HostCallOutcome::Unknown(super::UnknownReason::HelperExited),
            SendError::Stalled | SendError::Ipc => {
                HostCallOutcome::Unknown(super::UnknownReason::IpcFailure)
            }
            _ => HostCallOutcome::Unknown(super::UnknownReason::IpcFailure),
        };
    }
    wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(30));
    let event = executor.poll_reply().expect("poll_reply yielded an event");
    match event {
        HostCallEvent::Outcome { outcome, .. } => outcome,
        HostCallEvent::LateReply { outcome, .. } => outcome,
    }
}

/// Spawn a process-isolated stub helper configured with `behaviour` and an inherited file descriptor.
#[doc(hidden)]
pub fn spawn_stub_helper_with_inherited_fd(
    behaviour: StubBehaviour,
    inherited_fd: impl AsFd,
) -> io::Result<KmsIoExecutor> {
    let exe = executor_executable()?;
    spawn_internal(
        &exe,
        inherited_fd.as_fd(),
        IncarnationId::first(),
        Some(behaviour),
    )
}

/// Create a nonblocking pipe pair for fence descriptor ownership testing.
#[doc(hidden)]
pub fn pipe_pair() -> (std::fs::File, std::fs::File) {
    let mut fds = [0 as libc::c_int; 2];
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if rc != 0 {
        panic!("pipe2 failed: {}", io::Error::last_os_error());
    }
    unsafe {
        let flags = libc::fcntl(fds[0], libc::F_GETFL);
        if flags < 0 {
            panic!("fcntl F_GETFL failed: {}", io::Error::last_os_error());
        }
        if libc::fcntl(fds[0], libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            panic!("fcntl F_SETFL failed: {}", io::Error::last_os_error());
        }
        (
            std::fs::File::from_raw_fd(fds[0]),
            std::fs::File::from_raw_fd(fds[1]),
        )
    }
}

/// Bounded wait on a descriptor readability in tests, failing with a clear panic on timeout.
#[doc(hidden)]
pub fn wait_readable(fd: BorrowedFd<'_>, _timeout: Duration) {
    let timeout = Duration::from_secs(30);
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
    let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if rc == 0 {
        panic!("wait_readable timed out after 30s: descriptor not readable");
    }
    if rc < 0 {
        panic!(
            "wait_readable libc::poll failed: {}",
            io::Error::last_os_error()
        );
    }
}

/// Bounded wait for helper process exit in tests without reaping it.
#[doc(hidden)]
pub fn wait_for_helper_exit(executor: &mut KmsIoExecutor, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let pid = executor.child_pid();
    while Instant::now() < deadline {
        #[cfg(unix)]
        {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let rc = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                )
            };
            if rc == 0 && unsafe { info.si_pid() } == pid {
                std::thread::sleep(Duration::from_millis(10));
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("wait_for_helper_exit timed out after {timeout:?}: helper process did not exit");
}

/// Send SIGKILL to helper process in tests.
#[doc(hidden)]
pub fn kill_helper(executor: &mut KmsIoExecutor) {
    let pid = executor.child_pid();
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

/// Poll tick until executor state reaches Reaped in tests.
#[doc(hidden)]
pub fn reap_within(executor: &mut KmsIoExecutor, _timeout: Duration) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        executor.tick(Instant::now());
        if executor.state() == super::ExecutorState::Reaped {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("reap_within timed out after 30s: executor was not reaped");
}

/// Kill helper process and wait for reap in tests.
#[doc(hidden)]
pub fn kill_and_reap(executor: &mut KmsIoExecutor) {
    kill_helper(executor);
    reap_within(executor, Duration::from_secs(30));
}

#[doc(hidden)]
pub fn drive_send_failure() -> (KmsIoExecutor, HostCallEvent) {
    let mut executor = spawn_stub_helper(StubBehaviour::ExitBeforeReply).expect("spawn");
    wait_for_helper_exit(&mut executor, Duration::from_secs(30));
    let request = small_atomic_request_for_tests();
    let err = executor
        .send(
            &request,
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .unwrap_err();
    assert_eq!(err, SendError::Ipc);
    let event = executor.poll_reply().expect("poll_reply after failed send");
    (executor, event)
}

#[doc(hidden)]
pub fn drive_helper_exit() -> (KmsIoExecutor, HostCallEvent) {
    let mut executor = spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    kill_helper(&mut executor);
    wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(30));
    let event = executor.poll_reply().expect("poll_reply after helper exit");
    (executor, event)
}

#[doc(hidden)]
pub fn drive_malformed_reply() -> (KmsIoExecutor, HostCallEvent) {
    let mut executor =
        spawn_stub_helper(StubBehaviour::ReplyWithForeignCorrelation).expect("spawn");
    executor
        .send(
            &small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    wait_readable(executor.control_fd().expect("fd"), Duration::from_secs(30));
    let event = executor
        .poll_reply()
        .expect("poll_reply after malformed reply");
    (executor, event)
}

#[doc(hidden)]
pub fn drive_watchdog_expiry() -> (KmsIoExecutor, HostCallEvent) {
    let mut executor = spawn_stub_helper(StubBehaviour::NeverReply).expect("spawn");
    executor
        .send(
            &small_atomic_request_for_tests(),
            HostCallReservation::Submitting(SubmittingProof::for_tests()),
        )
        .expect("send");
    let event = executor
        .tick(Instant::now() + Duration::from_secs(3))
        .expect("tick watchdog expiry");
    (executor, event)
}

// ---------------------------------------------------------------------------
// Request builders.
//
// They live here, in the crate, rather than in the test files, so an external
// integration test never constructs a wire type by hand — and so no task
// consumes a helper produced by a later one. They are produced by the task
// that defines the types they build.
// ---------------------------------------------------------------------------

use super::{
    HostCallClass,
    protocol::{
        AtomicPropertyList, AtomicRequest, ClockProbeRequest, DRM_MODE_ATOMIC_NONBLOCK,
        DRM_MODE_ATOMIC_TEST_ONLY, HostCallCorrelation, HostCallRequest, OutFenceSlot, RequestSeq,
    },
};
use crate::kms::owner::{
    identity::{ClockEpochId, CommitId, EventToken},
    lifecycle::{ClockProbeId, LifecycleEpochId},
};

fn atomic_correlation(seq: u64) -> HostCallCorrelation {
    HostCallCorrelation::Atomic {
        seq: RequestSeq::for_tests(seq),
        incarnation: IncarnationId::first(),
        lifecycle_epoch: LifecycleEpochId::first(),
        transition: None,
        commit: CommitId::for_tests(1),
        // Tagged: an untagged token is rejected by the decoder's purpose-tag
        // check, so nothing that crosses the wire may use `for_tests`.
        event_token: EventToken::tagged_for_tests(seq),
    }
}

fn empty_properties() -> AtomicPropertyList {
    AtomicPropertyList {
        objects: Vec::new(),
        count_props: Vec::new(),
        props: Vec::new(),
        values: Vec::new(),
    }
}

/// One object with one property: the smallest request that is not empty.
#[doc(hidden)]
pub fn small_atomic_request_for_tests() -> HostCallRequest {
    HostCallRequest::Atomic(AtomicRequest {
        correlation: atomic_correlation(1),
        class: HostCallClass::SeatActiveNonblock,
        flags: DRM_MODE_ATOMIC_NONBLOCK,
        properties: AtomicPropertyList {
            objects: vec![31],
            count_props: vec![1],
            props: vec![7],
            values: vec![0],
        },
        out_fence_slots: Vec::new(),
    })
}

/// Declares one out-fence slot, so an accepted reply is expected to carry
/// exactly one descriptor.
#[doc(hidden)]
pub fn fence_returning_request_for_tests() -> HostCallRequest {
    HostCallRequest::Atomic(AtomicRequest {
        correlation: atomic_correlation(2),
        class: HostCallClass::SeatActiveNonblock,
        flags: DRM_MODE_ATOMIC_NONBLOCK,
        properties: AtomicPropertyList {
            objects: vec![31],
            count_props: vec![1],
            props: vec![7],
            values: vec![0],
        },
        out_fence_slots: vec![OutFenceSlot {
            crtc_id: 31,
            value_index: 0,
        }],
    })
}

/// `class` must be one of the two validation classes; the decoder rejects any
/// other, and neither may carry out-fence slots.
#[doc(hidden)]
pub fn validation_request_for_tests(class: HostCallClass) -> HostCallRequest {
    assert!(
        class.is_validation(),
        "validation_request_for_tests needs a validation class, got {class:?}"
    );
    HostCallRequest::Atomic(AtomicRequest {
        correlation: atomic_correlation(3),
        class,
        flags: DRM_MODE_ATOMIC_TEST_ONLY,
        properties: AtomicPropertyList {
            objects: vec![31],
            count_props: vec![1],
            props: vec![7],
            values: vec![0],
        },
        out_fence_slots: Vec::new(),
    })
}

#[doc(hidden)]
pub fn blocking_atomic_request_for_tests() -> HostCallRequest {
    HostCallRequest::Atomic(AtomicRequest {
        correlation: atomic_correlation(4),
        class: HostCallClass::ColdStartOrOfflineBlocking,
        flags: 0,
        properties: empty_properties(),
        out_fence_slots: Vec::new(),
    })
}

#[doc(hidden)]
pub fn probe_request_for_tests() -> HostCallRequest {
    HostCallRequest::ClockProbe(ClockProbeRequest {
        correlation: HostCallCorrelation::ClockProbe {
            seq: RequestSeq::for_tests(5),
            incarnation: IncarnationId::first(),
            lifecycle_epoch: LifecycleEpochId::first(),
            topology_generation: 1,
            hardware_crtc: 42,
            clock_epoch: ClockEpochId::first(),
            probe: ClockProbeId::first(),
        },
    })
}

/// Two objects carrying three properties between them, so a helper that still
/// submits `count_objs = 0` is distinguishable from one that does not.
#[doc(hidden)]
pub fn three_property_request_for_tests() -> HostCallRequest {
    HostCallRequest::Atomic(AtomicRequest {
        correlation: atomic_correlation(6),
        class: HostCallClass::SeatActiveNonblock,
        flags: DRM_MODE_ATOMIC_NONBLOCK,
        properties: AtomicPropertyList {
            objects: vec![31, 42],
            count_props: vec![1, 2],
            props: vec![7, 8, 9],
            values: vec![1, 2, 3],
        },
        out_fence_slots: Vec::new(),
    })
}

/// Object id 0 is never a valid DRM object, so a real device rejects it
/// rather than accepting an empty request.
#[doc(hidden)]
pub fn invalid_object_request_for_tests() -> HostCallRequest {
    HostCallRequest::Atomic(AtomicRequest {
        correlation: atomic_correlation(7),
        class: HostCallClass::SeatActiveNonblock,
        flags: DRM_MODE_ATOMIC_NONBLOCK,
        properties: AtomicPropertyList {
            objects: vec![0],
            count_props: vec![1],
            props: vec![7],
            values: vec![0],
        },
        out_fence_slots: Vec::new(),
    })
}

/// `n` distinct CRTCs, each with its own value slot, for the reply-bitmap
/// validity tests.
#[doc(hidden)]
pub fn request_with_slots_for_tests(n: usize) -> HostCallRequest {
    let objects: Vec<u32> = (0..n as u32).map(|i| i + 1).collect();
    HostCallRequest::Atomic(AtomicRequest {
        correlation: atomic_correlation(8),
        class: HostCallClass::SeatActiveNonblock,
        flags: DRM_MODE_ATOMIC_NONBLOCK,
        properties: AtomicPropertyList {
            objects: objects.clone(),
            count_props: vec![1; n],
            props: vec![7; n],
            values: vec![0; n],
        },
        out_fence_slots: objects
            .iter()
            .enumerate()
            .map(|(i, crtc)| OutFenceSlot {
                crtc_id: *crtc,
                value_index: i as u32,
            })
            .collect(),
    })
}
