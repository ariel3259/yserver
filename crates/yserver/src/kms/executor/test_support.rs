//! Test support and stub helper for executor substrate integration testing.

use std::{
    io,
    os::{fd::AsFd, unix::net::UnixStream},
    time::{Duration, Instant},
};

use super::{
    CONTROL_FD, KMS_FD, KmsIoExecutor, STUB_ARG_PREFIX, executor_executable, protocol,
    spawn_internal, take_inherited_fd, transport,
};
use crate::kms::owner::identity::IncarnationId;

/// Simulated helper behaviors for testing supervisor isolation and error paths.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum StubBehaviour {
    NeverReply,
    ExitBeforeReply,
    RejectWith(i32),
    IgnoreTermination,
    AcceptAfter(Duration),
}

impl StubBehaviour {
    pub(crate) fn to_arg_string(self) -> String {
        match self {
            Self::NeverReply => "never-reply".to_string(),
            Self::ExitBeforeReply => "exit-before-reply".to_string(),
            Self::RejectWith(errno) => format!("reject:{errno}"),
            Self::IgnoreTermination => "ignore-termination".to_string(),
            Self::AcceptAfter(duration) => format!("accept-after:{}", duration.as_millis()),
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
            let mut buf = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut buf);
            std::process::exit(0);
        }
        StubBehaviour::RejectWith(errno) => {
            let mut req_buf = [0u8; protocol::REQUEST_FRAME_LEN];
            let received = transport::recv_frame(&control, &mut req_buf)?;
            if received.len > 0 {
                let req = protocol::decode_request(&req_buf[..received.len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("protocol error: {e:?}"))
                })?;
                let reply = protocol::HostCallReply::Rejected {
                    seq: req.seq(),
                    errno,
                    helper_duration_ns: 1_000_000,
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
            let mut req_buf = [0u8; protocol::REQUEST_FRAME_LEN];
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
                    seq: req.seq(),
                    helper_duration_ns,
                    out_fence_count: 0,
                };
                let rep_frame = protocol::encode_reply(&reply);
                transport::send_frame(&control, &rep_frame)?;
            }
            let mut sink = [0u8; 1];
            let _ = std::io::Read::read(&mut &control, &mut sink);
            Ok(())
        }
    }
}
