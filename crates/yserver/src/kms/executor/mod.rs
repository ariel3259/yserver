//! KMS executor subsystem.

use std::{
    collections::HashMap,
    io,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use self::{
    protocol::{AtomicRequest, HostCallReply, HostCallRequest, RequestSeq, encode_request},
    transport::{REPLY_FRAME_LEN, adopt_reply, recv_frame, send_frame, seqpacket_pair},
};
use crate::kms::owner::identity::{ClockEpochId, CommitId, EventToken, IncarnationId};

pub(crate) mod helper;
pub(crate) mod protocol;
#[doc(hidden)]
pub mod test_support;
pub(crate) mod transport;

#[doc(hidden)]
pub use helper::run_reexec_executor_if_requested;

pub(crate) const CONTROL_FD: RawFd = 198;
pub(crate) const KMS_FD: RawFd = 199;
const INHERIT_SOURCE_FD_MIN: RawFd = 256;
pub(crate) const REEXEC_ARG: &str = "--yserver-internal-kms-executor-v1";
const STUB_ARG_PREFIX: &str = "--yserver-internal-kms-executor-stub=";

/// Monotonic lease identifier assigned to an open descriptor in an `IncarnationFdSet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[doc(hidden)]
pub struct LeaseId(pub(crate) u64);

impl LeaseId {
    #[doc(hidden)]
    pub const fn for_tests(id: u64) -> Self {
        Self(id)
    }

    #[allow(dead_code)]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Lease management errors on an `IncarnationFdSet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[doc(hidden)]
pub enum LeaseError {
    #[error("lease not reaped")]
    NotReaped,
    #[error("invalid lease")]
    InvalidLease,
    #[error("outstanding leases")]
    OutstandingLeases,
}

/// Lifecycle state of a KMS I/O executor instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum ExecutorState {
    Live,
    Stalled,
    ShutdownStalled,
    Reaped,
}

/// Linear proof that a child KMS executor process was reaped.
///
/// This type is neither [`Copy`] nor [`Clone`]; leases may only be released once.
///
/// ```compile_fail
/// use yserver::kms::executor::ReapProof;
/// fn test_clone(p: ReapProof) {
///     let _ = p.clone();
/// }
/// ```
#[derive(Debug)]
#[doc(hidden)]
pub struct ReapProof(pub(crate) ());

impl ReapProof {
    #[doc(hidden)]
    pub const fn for_tests() -> Self {
        Self(())
    }
}

/// Tracks the complete set of open descriptors and helper leases for a KMS device incarnation.
#[derive(Debug, Default)]
#[doc(hidden)]
#[allow(dead_code)]
pub struct IncarnationFdSet {
    leases: HashMap<LeaseId, OwnedFd>,
    next_lease_id: u64,
}

#[allow(dead_code)]
impl IncarnationFdSet {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn register_alias(&mut self, fd: OwnedFd) -> LeaseId {
        self.next_lease_id = self.next_lease_id.wrapping_add(1);
        let id = LeaseId(self.next_lease_id);
        self.leases.insert(id, fd);
        id
    }

    pub(crate) fn release(&mut self, lease: LeaseId) -> Result<(), LeaseError> {
        if !self.leases.contains_key(&lease) {
            return Err(LeaseError::InvalidLease);
        }
        Err(LeaseError::NotReaped)
    }

    pub(crate) fn release_with_proof(
        &mut self,
        lease: LeaseId,
        _proof: ReapProof,
    ) -> Result<(), LeaseError> {
        if self.leases.remove(&lease).is_some() {
            Ok(())
        } else {
            Err(LeaseError::InvalidLease)
        }
    }

    pub(crate) fn outstanding(&self) -> usize {
        self.leases.len()
    }

    pub(crate) fn may_open_fresh_incarnation(&self) -> Result<(), LeaseError> {
        if self.outstanding() > 0 {
            Err(LeaseError::OutstandingLeases)
        } else {
            Ok(())
        }
    }
}

/// Classifies a KMS host call to determine its normative watchdog deadline.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum HostCallClass {
    SeatActiveNonblock,
    ColdStartOrOfflineBlocking,
}

impl HostCallClass {
    pub const fn watchdog(self) -> Duration {
        match self {
            Self::SeatActiveNonblock => Duration::from_secs(2),
            Self::ColdStartOrOfflineBlocking => Duration::from_secs(30),
        }
    }

    pub(crate) fn from_request(request: &HostCallRequest) -> Self {
        match request {
            HostCallRequest::Atomic(atomic) => {
                const DRM_MODE_ATOMIC_NONBLOCK: u32 = 0x0200;
                if (atomic.flags & DRM_MODE_ATOMIC_NONBLOCK) != 0 {
                    Self::SeatActiveNonblock
                } else {
                    Self::ColdStartOrOfflineBlocking
                }
            }
            HostCallRequest::ClockProbe(_) => Self::SeatActiveNonblock,
        }
    }
}

/// Linear proof that a `Submitting` or `CoordinateSubmitting` lease was installed
/// before IPC dispatch.
#[derive(Debug)]
#[doc(hidden)]
pub struct SubmittingProof(());

impl SubmittingProof {
    #[doc(hidden)]
    pub const fn for_tests() -> Self {
        Self(())
    }
}

/// Outcome of a supervised KMS host-call IPC exchange.
#[derive(Debug)]
#[doc(hidden)]
pub enum HostCallOutcome {
    Accepted {
        helper_duration_ns: u64,
        round_trip_ns: u64,
        out_fences: Vec<OwnedFd>,
    },
    Rejected {
        errno: i32,
        helper_duration_ns: u64,
        round_trip_ns: u64,
    },
    Unknown(UnknownReason),
}

/// Underlying cause when a host-call outcome cannot be verified.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[doc(hidden)]
pub enum UnknownReason {
    WatchdogExpired,
    HelperExited,
    IpcFailure,
    MalformedReply,
}

/// Observable state of a child executor process during reaping.
#[derive(Debug)]
#[doc(hidden)]
pub enum ReapState {
    Running,
    Reaped(ExitStatus),
    Stalled,
}

/// Supervisor for a single process-isolated KMS executor instance.
#[derive(Debug)]
#[doc(hidden)]
pub struct KmsIoExecutor {
    child: Child,
    control: UnixStream,
    incarnation: IncarnationId,
    termination_requested: bool,
    reaped: Option<ExitStatus>,
    state: ExecutorState,
    reap_proof: Option<ReapProof>,
    reap_proof_taken: bool,
    next_seq: u64,
}

impl KmsIoExecutor {
    #[allow(dead_code)]
    pub(crate) fn state(&self) -> ExecutorState {
        self.state
    }

    #[doc(hidden)]
    pub fn take_reap_proof(&mut self) -> Option<ReapProof> {
        let _ = self.try_reap();
        self.reap_proof_taken = true;
        self.reap_proof.take()
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn enter_shutdown_stalled(&mut self) {
        self.state = ExecutorState::ShutdownStalled;
    }

    #[allow(dead_code)] // Will be consumed in Task 10, 11
    pub(crate) fn spawn(kms_fd: BorrowedFd<'_>, incarnation: IncarnationId) -> io::Result<Self> {
        let exe = executor_executable()?;
        spawn_internal(&exe, kms_fd, incarnation, None)
    }

    fn check_child_exited(&mut self) -> bool {
        if self.reaped.is_some() {
            return true;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.reaped = Some(status);
                self.state = ExecutorState::Reaped;
                if self.reap_proof.is_none() && !self.reap_proof_taken {
                    self.reap_proof = Some(ReapProof(()));
                }
                true
            }
            _ => false,
        }
    }

    pub(crate) fn dispatch(
        &mut self,
        request: &HostCallRequest,
        _proof: SubmittingProof,
    ) -> HostCallOutcome {
        let class = HostCallClass::from_request(request);
        let watchdog_duration = class.watchdog();
        let expected_seq = request.seq();

        let started = Instant::now();
        let deadline = match started.checked_add(watchdog_duration) {
            Some(d) => d,
            None => {
                self.state = ExecutorState::Stalled;
                return HostCallOutcome::Unknown(UnknownReason::WatchdogExpired);
            }
        };

        let req_frame = encode_request(request);

        if let Err(_err) = send_frame(&self.control, &req_frame) {
            if self.check_child_exited() {
                return HostCallOutcome::Unknown(UnknownReason::HelperExited);
            }
            return HostCallOutcome::Unknown(UnknownReason::IpcFailure);
        }

        let mut reply_buf = [0u8; REPLY_FRAME_LEN];
        let received_frame = loop {
            let now = Instant::now();
            if now >= deadline {
                self.state = ExecutorState::Stalled;
                self.request_termination();
                return HostCallOutcome::Unknown(UnknownReason::WatchdogExpired);
            }
            let remaining = deadline - now;
            let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as libc::c_int;

            let mut pfd = libc::pollfd {
                fd: self.control.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };

            // SAFETY: pfd points to 1 valid pollfd on the stack.
            let poll_rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
            if poll_rc == 0 {
                self.state = ExecutorState::Stalled;
                self.request_termination();
                return HostCallOutcome::Unknown(UnknownReason::WatchdogExpired);
            }
            if poll_rc < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if self.check_child_exited() {
                    return HostCallOutcome::Unknown(UnknownReason::HelperExited);
                }
                return HostCallOutcome::Unknown(UnknownReason::IpcFailure);
            }

            match recv_frame(&self.control, &mut reply_buf) {
                Ok(rf) => {
                    if rf.len == 0 {
                        let wait_deadline = Instant::now() + Duration::from_millis(100);
                        while Instant::now() < wait_deadline {
                            if self.check_child_exited() {
                                return HostCallOutcome::Unknown(UnknownReason::HelperExited);
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        if self.check_child_exited() {
                            return HostCallOutcome::Unknown(UnknownReason::HelperExited);
                        }
                        return HostCallOutcome::Unknown(UnknownReason::IpcFailure);
                    }
                    break rf;
                }
                Err(_err) => {
                    if self.check_child_exited() {
                        return HostCallOutcome::Unknown(UnknownReason::HelperExited);
                    }
                    return HostCallOutcome::Unknown(UnknownReason::IpcFailure);
                }
            }
        };

        let round_trip_ns = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);

        let (reply, fds) = match adopt_reply(&reply_buf[..received_frame.len], received_frame.fds) {
            Ok(res) => res,
            Err(_) => return HostCallOutcome::Unknown(UnknownReason::MalformedReply),
        };

        match reply {
            HostCallReply::Accepted {
                seq,
                helper_duration_ns,
                out_fence_count: _,
            } => {
                if seq != expected_seq {
                    return HostCallOutcome::Unknown(UnknownReason::MalformedReply);
                }
                HostCallOutcome::Accepted {
                    helper_duration_ns,
                    round_trip_ns,
                    out_fences: fds,
                }
            }
            HostCallReply::Rejected {
                seq,
                errno,
                helper_duration_ns,
            } => {
                if seq != expected_seq {
                    return HostCallOutcome::Unknown(UnknownReason::MalformedReply);
                }
                HostCallOutcome::Rejected {
                    errno,
                    helper_duration_ns,
                    round_trip_ns,
                }
            }
            HostCallReply::ClockProbe {
                seq,
                sequence: _,
                helper_duration_ns,
            } => {
                if seq != expected_seq {
                    return HostCallOutcome::Unknown(UnknownReason::MalformedReply);
                }
                HostCallOutcome::Accepted {
                    helper_duration_ns,
                    round_trip_ns,
                    out_fences: fds,
                }
            }
        }
    }

    pub fn request_termination(&mut self) {
        self.termination_requested = true;
        if !self.check_child_exited() {
            let pid = self.child.id() as libc::pid_t;
            // SAFETY: Sends SIGTERM to the confirmed-alive helper process.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }

    pub fn try_reap(&mut self) -> ReapState {
        if let Some(status) = self.reaped {
            self.state = ExecutorState::Reaped;
            return ReapState::Reaped(status);
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.reaped = Some(status);
                self.state = ExecutorState::Reaped;
                if self.reap_proof.is_none() && !self.reap_proof_taken {
                    self.reap_proof = Some(ReapProof(()));
                }
                ReapState::Reaped(status)
            }
            Ok(None) => {
                if self.termination_requested {
                    self.state = ExecutorState::Stalled;
                    ReapState::Stalled
                } else {
                    ReapState::Running
                }
            }
            Err(_) => {
                self.state = ExecutorState::Stalled;
                ReapState::Stalled
            }
        }
    }

    #[doc(hidden)]
    pub fn dispatch_for_tests(&mut self, class: HostCallClass) -> HostCallOutcome {
        let flags = match class {
            HostCallClass::SeatActiveNonblock => 0x0200,
            HostCallClass::ColdStartOrOfflineBlocking => 0,
        };
        self.next_seq += 1;
        let request = HostCallRequest::Atomic(AtomicRequest {
            seq: RequestSeq::for_tests(self.next_seq),
            incarnation: self.incarnation,
            epoch: ClockEpochId::first(),
            transition: None,
            commit: CommitId::for_tests(1),
            event_token: EventToken::for_tests(1),
            flags,
            payload_len: 0,
        });
        self.dispatch(&request, SubmittingProof::for_tests())
    }
}

impl Drop for KmsIoExecutor {
    fn drop(&mut self) {
        if !self.check_child_exited() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub(crate) fn executor_executable() -> io::Result<PathBuf> {
    if let Ok(exe) = std::env::var("CARGO_BIN_EXE_yserver") {
        let p = PathBuf::from(exe);
        if p.exists() {
            return Ok(p);
        }
    }
    if let Some(exe) = option_env!("CARGO_BIN_EXE_yserver") {
        let p = PathBuf::from(exe);
        if p.exists() {
            return Ok(p);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let self_exe = PathBuf::from("/proc/self/exe");
        if let Ok(target) = std::fs::read_link(&self_exe)
            && target.file_name().and_then(|n| n.to_str()) == Some("yserver")
        {
            return Ok(self_exe);
        }
    }
    if let Ok(current) = std::env::current_exe() {
        if current.file_name().and_then(|n| n.to_str()) == Some("yserver") {
            return Ok(current);
        }
        if let Some(parent) = current.parent() {
            let candidate = if parent.file_name().and_then(|n| n.to_str()) == Some("deps") {
                parent.parent().map(|p| p.join("yserver"))
            } else {
                Some(parent.join("yserver"))
            };
            if let Some(cand) = candidate
                && cand.exists()
            {
                return Ok(cand);
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        Ok(PathBuf::from("/proc/self/exe"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe()
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn arm_helper_parent_death_signal(expected_parent: libc::pid_t) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    // SAFETY: PR_SET_PDEATHSIG configures parent termination signal.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } < 0 {
        return Err(io::Error::last_os_error());
    }
    #[cfg(target_os = "freebsd")]
    {
        let mut signal = libc::SIGKILL;
        // SAFETY: PROC_PDEATHSIG_CTL configures parent termination signal.
        if unsafe {
            libc::procctl(
                libc::P_PID,
                0,
                libc::PROC_PDEATHSIG_CTL,
                std::ptr::from_mut(&mut signal).cast(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    // SAFETY: getppid has no preconditions.
    if unsafe { libc::getppid() } != expected_parent {
        return Err(io::Error::from_raw_os_error(libc::ECHILD));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn arm_helper_parent_death_signal(_expected_parent: libc::pid_t) -> io::Result<()> {
    Ok(())
}

fn duplicate_fd_at_least(fd: BorrowedFd<'_>) -> io::Result<OwnedFd> {
    // SAFETY: fcntl F_DUPFD_CLOEXEC duplicates fd to a descriptor >= INHERIT_SOURCE_FD_MIN.
    let duplicated =
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, INHERIT_SOURCE_FD_MIN) };
    if duplicated < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful F_DUPFD_CLOEXEC returns a new owned file descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

fn duplicate_to_inherited_slot(source: RawFd, target: RawFd) -> io::Result<()> {
    // SAFETY: dup2 duplicates source to target slot in child.
    if unsafe { libc::dup2(source, target) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl F_SETFD clears CLOEXEC on target slot.
    if unsafe { libc::fcntl(target, libc::F_SETFD, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn take_inherited_fd(fd: RawFd, label: &str) -> io::Result<OwnedFd> {
    // SAFETY: fcntl F_GETFD validates that fd exists.
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        let source = io::Error::last_os_error();
        return Err(io::Error::new(
            source.kind(),
            format!("executor helper missing inherited {label} fd {fd}: {source}"),
        ));
    }
    // SAFETY: Transfers ownership of the inherited slot to caller.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub(crate) fn spawn_internal(
    executable: &std::path::Path,
    kms_fd: BorrowedFd<'_>,
    incarnation: IncarnationId,
    stub: Option<test_support::StubBehaviour>,
) -> io::Result<KmsIoExecutor> {
    let (parent_control, child_control) = seqpacket_pair()?;
    let inherited_control = duplicate_fd_at_least(child_control.as_fd())?;
    let inherited_kms = duplicate_fd_at_least(kms_fd)?;
    let control_source = inherited_control.as_raw_fd();
    let kms_source = inherited_kms.as_raw_fd();

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    // SAFETY: getpid has no preconditions.
    let supervisor_pid = unsafe { libc::getpid() };

    let mut command = Command::new(executable);
    if let Some(behaviour) = stub {
        command.arg(format!("{STUB_ARG_PREFIX}{}", behaviour.to_arg_string()));
    } else {
        command.arg(REEXEC_ARG);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null());

    // SAFETY: pre_exec runs only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            arm_helper_parent_death_signal(supervisor_pid)?;
            duplicate_to_inherited_slot(control_source, CONTROL_FD)?;
            duplicate_to_inherited_slot(kms_source, KMS_FD)?;
            Ok(())
        });
    }

    let child = command.spawn()?;
    drop(child_control);
    drop(inherited_control);
    drop(inherited_kms);

    Ok(KmsIoExecutor {
        child,
        control: parent_control,
        incarnation,
        termination_requested: false,
        reaped: None,
        state: ExecutorState::Live,
        reap_proof: None,
        reap_proof_taken: false,
        next_seq: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::StubBehaviour;

    fn stub_executor(behaviour: test_support::StubBehaviour, _lease: LeaseId) -> KmsIoExecutor {
        test_support::spawn_stub_helper(behaviour).expect("spawn stub helper")
    }

    #[test]
    fn a_watchdog_expiry_does_not_release_the_lease() {
        let mut fds = IncarnationFdSet::default();
        let lease = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        let mut executor = stub_executor(StubBehaviour::NeverReply, lease);
        let _ = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
        assert_eq!(fds.outstanding(), 1, "an unresolved call keeps its lease");
        assert_eq!(executor.state(), ExecutorState::Stalled);
    }

    #[test]
    fn only_a_wait_status_releases_the_lease() {
        let mut fds = IncarnationFdSet::default();
        let lease = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        assert!(matches!(fds.release(lease), Err(LeaseError::NotReaped)));
    }

    #[test]
    fn no_fresh_incarnation_is_created_while_a_lease_is_outstanding() {
        let mut fds = IncarnationFdSet::default();
        let _lease = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        assert!(fds.may_open_fresh_incarnation().is_err());
    }

    #[test]
    fn proven_reap_releases_the_lease() {
        let mut fds = IncarnationFdSet::default();
        let lease = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        let mut executor = stub_executor(StubBehaviour::ExitBeforeReply, lease);
        let _ = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
        let reap = executor.try_reap();
        assert!(matches!(reap, ReapState::Reaped(_)));
        assert_eq!(executor.state(), ExecutorState::Reaped);
        let proof = executor.take_reap_proof().expect("reap proof");
        assert_eq!(fds.release_with_proof(lease, proof), Ok(()));
        assert_eq!(fds.outstanding(), 0);
        assert_eq!(fds.may_open_fresh_incarnation(), Ok(()));
    }

    #[test]
    fn reap_proof_is_single_use() {
        let mut fds = IncarnationFdSet::default();
        let lease1 = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        let lease2 = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        let mut executor = stub_executor(StubBehaviour::ExitBeforeReply, lease1);
        let _ = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
        let _ = executor.try_reap();
        let proof = executor.take_reap_proof().expect("proof");
        assert!(executor.take_reap_proof().is_none());
        assert_eq!(fds.release_with_proof(lease1, proof), Ok(()));
        assert_eq!(fds.outstanding(), 1);
        assert_eq!(
            fds.release_with_proof(lease2, ReapProof::for_tests()),
            Ok(())
        );
        assert_eq!(fds.outstanding(), 0);
    }

    #[test]
    fn executor_state_transitions_to_shutdown_stalled() {
        let lease = LeaseId::for_tests(1);
        let mut executor = stub_executor(StubBehaviour::NeverReply, lease);
        assert_eq!(executor.state(), ExecutorState::Live);
        executor.enter_shutdown_stalled();
        assert_eq!(executor.state(), ExecutorState::ShutdownStalled);
    }

    #[test]
    fn releasing_invalid_lease_returns_error() {
        let mut fds = IncarnationFdSet::default();
        let invalid = LeaseId::for_tests(999);
        assert_eq!(fds.release(invalid), Err(LeaseError::InvalidLease));
        assert_eq!(
            fds.release_with_proof(invalid, ReapProof::for_tests()),
            Err(LeaseError::InvalidLease)
        );
    }
}
