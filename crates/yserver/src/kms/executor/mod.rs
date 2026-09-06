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

use libc::{c_int, pollfd};

use self::{
    protocol::{
        AtomicPropertyList, AtomicRequest, HostCallCorrelation, HostCallReply, RequestSeq,
        encode_request,
    },
    transport::{REPLY_FRAME_LEN, adopt_reply, recv_frame, send_frame, seqpacket_pair},
};
use crate::kms::owner::{
    identity::{CommitId, EventToken, IncarnationId},
    lifecycle::LifecycleEpochId,
};

#[doc(hidden)]
pub mod device_lock;
pub(crate) mod helper;
#[doc(hidden)]
pub mod protocol;
#[doc(hidden)]
pub mod test_support;
pub(crate) mod transport;

#[doc(hidden)]
pub use device_lock::LOCK_HANDOFF_ARG;
#[doc(hidden)]
pub use helper::run_reexec_executor_if_requested;

pub(crate) const CONTROL_FD: RawFd = 198;
pub(crate) const KMS_FD: RawFd = 199;
pub(crate) const LOCK_FD: RawFd = 200;
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
    SeatActiveValidation,
    ColdStartOrOfflineBlocking,
    ColdStartOrOfflineValidation,
}

impl HostCallClass {
    /// The two validation classes carry identical atomic flags — `TEST_ONLY`
    /// set, `NONBLOCK` clear — and differ only in watchdog. That is why the
    /// class is an explicit field of the request rather than something
    /// derived from the flag bits: `spec:320-329` gives seat-active
    /// validation two seconds and cold-start/offline validation thirty, and
    /// no flag distinguishes them.
    pub const fn watchdog(self) -> Duration {
        match self {
            Self::SeatActiveNonblock | Self::SeatActiveValidation => Duration::from_secs(2),
            Self::ColdStartOrOfflineBlocking | Self::ColdStartOrOfflineValidation => {
                Duration::from_secs(30)
            }
        }
    }

    /// `TEST_ONLY` work: it touches no hardware, creates no out-fence, and
    /// occupies no submitted-commit slot (`spec:320-329`).
    #[allow(dead_code)] // Consumed by the wire and the API in tasks 2 and 4.
    pub const fn is_validation(self) -> bool {
        matches!(
            self,
            Self::SeatActiveValidation | Self::ColdStartOrOfflineValidation
        )
    }

    /// Wire encoding. Zero is deliberately not a class, so a zeroed byte
    /// cannot decode as a valid one.
    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn wire_tag(self) -> u8 {
        match self {
            Self::SeatActiveNonblock => 1,
            Self::SeatActiveValidation => 2,
            Self::ColdStartOrOfflineBlocking => 3,
            Self::ColdStartOrOfflineValidation => 4,
        }
    }

    #[allow(dead_code)] // Consumed by the wire in task 2.
    pub const fn from_wire_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::SeatActiveNonblock),
            2 => Some(Self::SeatActiveValidation),
            3 => Some(Self::ColdStartOrOfflineBlocking),
            4 => Some(Self::ColdStartOrOfflineValidation),
            _ => None,
        }
    }
}

pub use crate::kms::owner::slot::{SubmittingProof, ValidationLease};
pub use protocol::HostCallRequest;

/// Outcome of a supervised KMS host-call IPC exchange.
#[derive(Debug)]
#[doc(hidden)]
pub enum HostCallOutcome {
    Accepted {
        helper_duration_ns: u64,
        round_trip_ns: u64,
        out_fences: Vec<OwnedFd>,
        out_fence_mask: u32,
    },
    ProbeAccepted {
        sequence: u64,
        helper_duration_ns: u64,
        round_trip_ns: u64,
    },
    Rejected {
        errno: i32,
        helper_duration_ns: u64,
        round_trip_ns: u64,
        unexpected_fence_output: bool,
    },
    Unknown(UnknownReason),
    ValidationAbandoned(UnknownReason),
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

/// Lifecycle phase of host calls on an executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum HostCallPhase {
    ColdStart,
    SeatActive,
    FinalOffline,
}

/// Precondition violation when a blocking host call is attempted during seat-active service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("blocking host call attempted during seat-active service")]
#[doc(hidden)]
pub struct BoundaryViolation;

/// Lease authorizing a clock-probe query.
#[derive(Debug)]
#[doc(hidden)]
pub struct ClockProbeLease(());

impl ClockProbeLease {
    #[doc(hidden)]
    pub const fn for_tests() -> Self {
        Self(())
    }
}

/// Reservation proof authorizing an asynchronous host call dispatch.
#[derive(Debug)]
#[doc(hidden)]
pub enum HostCallReservation {
    Submitting(SubmittingProof),
    Validation(ValidationLease),
    ClockProbe(ClockProbeLease),
}

/// Error returned when an asynchronous host call cannot be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[doc(hidden)]
pub enum SendError {
    #[error("host call already in flight")]
    AlreadyInFlight,
    #[error("executor is stalled")]
    Stalled,
    #[error("executor helper process was reaped")]
    Reaped,
    #[error("transport error sending host call")]
    Ipc,
    #[error("reservation kind does not match request class")]
    ReservationMismatch,
    #[error("boundary violation: cold start / offline request sent during seat active service")]
    BoundaryViolation,
}

#[derive(Debug)]
struct InFlight {
    correlation: HostCallCorrelation,
    class: HostCallClass,
    kind: protocol::RequestKind,
    slot_count: u32,
    started: Instant,
    deadline: Instant,
    terminalized: Option<UnknownReason>,
}

/// Asynchronous host-call event emitted by an executor.
#[derive(Debug)]
#[doc(hidden)]
pub enum HostCallEvent {
    Outcome {
        correlation: HostCallCorrelation,
        outcome: HostCallOutcome,
    },
    /// Arrived after its request was terminalized. Its fds are adopted so the
    /// owner can close them exactly once into quarantine.
    LateReply {
        correlation: HostCallCorrelation,
        outcome: HostCallOutcome,
    },
}

/// Supervisor for a single process-isolated KMS executor instance.
#[derive(Debug)]
#[doc(hidden)]
pub struct KmsIoExecutor {
    child: Child,
    control: UnixStream,
    incarnation: IncarnationId,
    lifecycle_epoch: LifecycleEpochId,
    termination_requested: bool,
    reaped: Option<ExitStatus>,
    state: ExecutorState,
    reap_proof: Option<ReapProof>,
    reap_proof_taken: bool,
    next_seq: u64,
    phase: HostCallPhase,
    in_flight: Option<InFlight>,
    queued_terminal_event: Option<HostCallEvent>,
    helper_pid: Option<libc::pid_t>,
}

impl KmsIoExecutor {
    #[allow(dead_code)]
    pub fn state(&self) -> ExecutorState {
        self.state
    }

    #[doc(hidden)]
    pub fn take_reap_proof(&mut self) -> Option<ReapProof> {
        let _ = self.try_reap();
        if let Some(proof) = self.reap_proof.take() {
            self.reap_proof_taken = true;
            Some(proof)
        } else {
            None
        }
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn enter_shutdown_stalled(&mut self) {
        self.state = ExecutorState::ShutdownStalled;
    }

    #[allow(dead_code)] // Will be consumed in Task 10, 11
    pub(crate) fn spawn(kms_fd: BorrowedFd<'_>, incarnation: IncarnationId) -> io::Result<Self> {
        let exe = executor_executable()?;
        spawn_internal_full(
            &exe,
            kms_fd,
            incarnation,
            LifecycleEpochId::first(),
            None,
            None,
            false,
            false,
        )
    }

    #[doc(hidden)]
    pub fn spawn_with_device_lock(
        kms_fd: BorrowedFd<'_>,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        lock: &device_lock::InheritableDeviceLock,
    ) -> io::Result<Self> {
        let exe = executor_executable()?;
        Self::spawn_with_device_lock_at(&exe, kms_fd, incarnation, lifecycle_epoch, lock)
    }

    #[doc(hidden)]
    pub fn spawn_with_device_lock_at(
        executable: &std::path::Path,
        kms_fd: BorrowedFd<'_>,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        lock: &device_lock::InheritableDeviceLock,
    ) -> io::Result<Self> {
        spawn_internal_full(
            executable,
            kms_fd,
            incarnation,
            lifecycle_epoch,
            Some(lock.as_fd()),
            None,
            false,
            false,
        )
    }

    #[doc(hidden)]
    pub fn spawn_wedged_lock_holder_for_tests(
        kms_fd: BorrowedFd<'_>,
        incarnation: IncarnationId,
        lifecycle_epoch: LifecycleEpochId,
        lock: &device_lock::InheritableDeviceLock,
    ) -> io::Result<Self> {
        let exe = executor_executable()?;
        spawn_internal_full(
            &exe,
            kms_fd,
            incarnation,
            lifecycle_epoch,
            Some(lock.as_fd()),
            Some(test_support::StubBehaviour::WedgedHoldingLock),
            true, // disarm_pdeathsig
            true, // null_stderr
        )
    }

    #[doc(hidden)]
    pub fn await_helper_ready(&mut self, timeout: Duration) -> io::Result<()> {
        let request = protocol::HandshakeRequest {
            incarnation: self.incarnation,
            lifecycle_epoch: self.lifecycle_epoch,
        };
        let frame = protocol::encode_handshake_request(&request);
        send_frame(&self.control, &frame)?;

        let deadline = Instant::now() + timeout;
        loop {
            let readable = wait_readable_bounded(self.control.as_raw_fd(), deadline)?;
            if !readable {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for executor helper readiness handshake",
                ));
            }
            let mut buf = [0u8; 64];
            let received = match recv_frame(&self.control, &mut buf) {
                Ok(rf) => rf,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            };
            if received.len == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "executor helper control socket closed during handshake",
                ));
            }
            let reply = protocol::decode_handshake_reply(&buf[..received.len]).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("executor helper malformed handshake reply: {err:?}"),
                )
            })?;
            if reply.incarnation != self.incarnation
                || reply.lifecycle_epoch != self.lifecycle_epoch
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "executor helper handshake mismatch: expected ({:?}, {:?}), got ({:?}, {:?})",
                        self.incarnation,
                        self.lifecycle_epoch,
                        reply.incarnation,
                        reply.lifecycle_epoch
                    ),
                ));
            }
            self.helper_pid = Some(reply.helper_pid as libc::pid_t);
            return Ok(());
        }
    }

    #[doc(hidden)]
    pub fn helper_pid(&self) -> libc::pid_t {
        self.helper_pid
            .unwrap_or_else(|| self.child.id() as libc::pid_t)
    }

    /// Predicate: has the helper become reapable? Records the status and the
    /// reap proof, but deliberately does **not** publish
    /// [`ExecutorState::Reaped`].
    ///
    /// Publishing the state here would let any incidental caller —
    /// `request_termination`, `Drop`, channel-loss classification — advance a
    /// just-terminalized executor past `Stalled` depending only on whether the
    /// kernel had made the zombie reapable at that microsecond. `try_reap` and
    /// `tick` own that transition, so a terminalized executor is observably
    /// `Stalled` until one of them runs.
    fn check_child_exited(&mut self) -> bool {
        if self.reaped.is_some() {
            return true;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.reaped = Some(status);
                if self.reap_proof.is_none() && !self.reap_proof_taken {
                    self.reap_proof = Some(ReapProof(()));
                }
                true
            }
            _ => false,
        }
    }

    /// Classify a lost control channel. Peer closure — EOF, `EPIPE` or
    /// `ECONNRESET` — proves the helper's control endpoint is gone, so it is
    /// `HelperExited` on its own evidence.
    ///
    /// `try_wait` is consulted first, because a confirmed reap settles the
    /// question, but its `Ok(None)` decides nothing: the socket reports the
    /// reset as soon as the helper's descriptors are torn down, which happens
    /// before the process becomes reapable. Deciding the reason from that
    /// instant is a race — stage 1 hid it behind a 100 ms `try_wait` sleep
    /// loop, which `COMMIT-5` does not permit on the asynchronous path.
    ///
    /// The reason is telemetry either way: `COMMIT-6` makes helper exit, IPC
    /// failure, missing reply and watchdog expiry all acceptance-unknown, and
    /// reap proof stays with `try_reap`/`ReapProof`.
    fn classify_channel_loss(&mut self, err: Option<&io::Error>) -> UnknownReason {
        if self.check_child_exited() {
            return UnknownReason::HelperExited;
        }
        match err {
            None => UnknownReason::HelperExited,
            Some(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) =>
            {
                UnknownReason::HelperExited
            }
            Some(_) => UnknownReason::IpcFailure,
        }
    }

    #[doc(hidden)]
    pub fn child_pid(&self) -> libc::pid_t {
        self.child.id() as libc::pid_t
    }

    #[doc(hidden)]
    pub fn phase(&self) -> HostCallPhase {
        self.phase
    }

    #[doc(hidden)]
    pub fn enter_seat_active(&mut self) {
        self.phase = HostCallPhase::SeatActive;
    }

    #[doc(hidden)]
    pub fn enter_final_offline(&mut self) {
        self.phase = HostCallPhase::FinalOffline;
    }

    #[doc(hidden)]
    pub fn control_fd(&self) -> Option<BorrowedFd<'_>> {
        if self.state == ExecutorState::Reaped {
            None
        } else {
            Some(self.control.as_fd())
        }
    }

    #[doc(hidden)]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.in_flight
            .as_ref()
            .filter(|f| f.terminalized.is_none())
            .map(|f| f.deadline)
    }

    fn terminalize_unknown(&mut self, reason: UnknownReason) -> Option<HostCallEvent> {
        let in_flight = self.in_flight.as_mut()?;
        if in_flight.terminalized.is_some() {
            return None;
        }
        in_flight.terminalized = Some(reason);
        let correlation = in_flight.correlation;
        let outcome = if in_flight.class.is_validation() {
            HostCallOutcome::ValidationAbandoned(reason)
        } else {
            HostCallOutcome::Unknown(reason)
        };
        self.state = ExecutorState::Stalled;
        self.request_termination();
        Some(HostCallEvent::Outcome {
            correlation,
            outcome,
        })
    }

    #[doc(hidden)]
    pub fn send(
        &mut self,
        request: &HostCallRequest,
        reservation: HostCallReservation,
    ) -> Result<(), SendError> {
        if self.state == ExecutorState::Reaped {
            return Err(SendError::Reaped);
        }
        if self.state == ExecutorState::Stalled || self.state == ExecutorState::ShutdownStalled {
            return Err(SendError::Stalled);
        }
        if self.in_flight.is_some() {
            return Err(SendError::AlreadyInFlight);
        }
        if self.phase == HostCallPhase::SeatActive
            && (request.class() == HostCallClass::ColdStartOrOfflineBlocking
                || request.class() == HostCallClass::ColdStartOrOfflineValidation)
        {
            return Err(SendError::BoundaryViolation);
        }
        let valid_reservation = match (request, &reservation) {
            (HostCallRequest::Atomic(a), HostCallReservation::Submitting(_)) => {
                !a.class.is_validation()
            }
            (HostCallRequest::Atomic(a), HostCallReservation::Validation(_)) => {
                a.class.is_validation()
            }
            (HostCallRequest::ClockProbe(_), HostCallReservation::ClockProbe(_)) => true,
            _ => false,
        };
        if !valid_reservation {
            return Err(SendError::ReservationMismatch);
        }

        let class = request.class();
        let watchdog_duration = class.watchdog();
        let started = Instant::now();
        let deadline = match started.checked_add(watchdog_duration) {
            Some(d) => d,
            None => {
                self.state = ExecutorState::Stalled;
                self.request_termination();
                return Err(SendError::Ipc);
            }
        };
        let slot_count = match request {
            HostCallRequest::Atomic(atomic) => atomic.out_fence_slots.len() as u32,
            HostCallRequest::ClockProbe(_) => 0,
        };
        self.in_flight = Some(InFlight {
            correlation: request.correlation(),
            class,
            kind: request.kind(),
            slot_count,
            started,
            deadline,
            terminalized: None,
        });

        let req_frame = encode_request(request);
        if let Err(err) = send_frame(&self.control, &req_frame) {
            let reason = self.classify_channel_loss(Some(&err));
            self.queued_terminal_event = self.terminalize_unknown(reason);
            return Err(SendError::Ipc);
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn poll_reply(&mut self) -> Option<HostCallEvent> {
        if let Some(event) = self.queued_terminal_event.take() {
            return Some(event);
        }
        let in_flight = self.in_flight.as_ref()?;

        let mut reply_buf = [0u8; REPLY_FRAME_LEN];
        let received_frame = match recv_frame(&self.control, &mut reply_buf) {
            Ok(rf) => {
                if rf.len == 0 {
                    let reason = self.classify_channel_loss(None);
                    return self.terminalize_unknown(reason);
                }
                rf
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return None,
            Err(e) => {
                let reason = self.classify_channel_loss(Some(&e));
                return self.terminalize_unknown(reason);
            }
        };

        let (reply, fds) = match adopt_reply(&reply_buf[..received_frame.len], received_frame.fds) {
            Ok(res) => res,
            Err(_) => return self.terminalize_unknown(UnknownReason::MalformedReply),
        };

        if reply.family() != in_flight.kind || reply.correlation() != in_flight.correlation {
            drop(fds);
            return self.terminalize_unknown(UnknownReason::MalformedReply);
        }

        let slot_count = in_flight.slot_count;
        let valid_mask = if slot_count == 0 {
            0
        } else if slot_count >= 32 {
            u32::MAX
        } else {
            u32::MAX >> (32 - slot_count)
        };

        let round_trip_ns =
            u64::try_from(in_flight.started.elapsed().as_nanos()).unwrap_or(u64::MAX);

        let outcome = match reply {
            HostCallReply::Accepted {
                helper_duration_ns,
                out_fence_mask,
                ..
            } => {
                if out_fence_mask & !valid_mask != 0 {
                    drop(fds);
                    return self.terminalize_unknown(UnknownReason::MalformedReply);
                }
                if fds.len() as u32 != out_fence_mask.count_ones() {
                    drop(fds);
                    return self.terminalize_unknown(UnknownReason::MalformedReply);
                }
                HostCallOutcome::Accepted {
                    helper_duration_ns,
                    round_trip_ns,
                    out_fences: fds,
                    out_fence_mask,
                }
            }
            HostCallReply::ProbeAccepted {
                sequence,
                helper_duration_ns,
                ..
            } => {
                if !fds.is_empty() {
                    drop(fds);
                    return self.terminalize_unknown(UnknownReason::MalformedReply);
                }
                HostCallOutcome::ProbeAccepted {
                    sequence,
                    helper_duration_ns,
                    round_trip_ns,
                }
            }
            HostCallReply::Rejected {
                errno,
                helper_duration_ns,
                unexpected_fence_output,
                ..
            } => {
                if !fds.is_empty() {
                    drop(fds);
                    return self.terminalize_unknown(UnknownReason::MalformedReply);
                }
                HostCallOutcome::Rejected {
                    errno,
                    helper_duration_ns,
                    round_trip_ns,
                    unexpected_fence_output,
                }
            }
            HostCallReply::ProbeRejected {
                errno,
                helper_duration_ns,
                ..
            } => {
                if !fds.is_empty() {
                    drop(fds);
                    return self.terminalize_unknown(UnknownReason::MalformedReply);
                }
                HostCallOutcome::Rejected {
                    errno,
                    helper_duration_ns,
                    round_trip_ns,
                    unexpected_fence_output: false,
                }
            }
        };

        if in_flight.terminalized.is_some() {
            Some(HostCallEvent::LateReply {
                correlation: in_flight.correlation,
                outcome,
            })
        } else {
            let correlation = in_flight.correlation;
            self.in_flight = None;
            Some(HostCallEvent::Outcome {
                correlation,
                outcome,
            })
        }
    }

    #[doc(hidden)]
    pub fn tick(&mut self, now: Instant) -> Option<HostCallEvent> {
        if let Some(in_flight) = self.in_flight.as_ref()
            && in_flight.terminalized.is_none()
            && now >= in_flight.deadline
        {
            return self.terminalize_unknown(UnknownReason::WatchdogExpired);
        }

        if let Some(in_flight) = self.in_flight.as_ref()
            && in_flight.terminalized.is_some()
            && let ReapState::Reaped(_) = self.try_reap()
        {
            self.in_flight = None;
            self.state = ExecutorState::Reaped;
        } else if self.state == ExecutorState::Stalled
            && let ReapState::Reaped(_) = self.try_reap()
        {
            self.state = ExecutorState::Reaped;
        }
        None
    }

    #[doc(hidden)]
    pub fn dispatch_blocking_at_boundary(
        &mut self,
        request: &HostCallRequest,
        reservation: HostCallReservation,
    ) -> Result<HostCallOutcome, BoundaryViolation> {
        if self.phase == HostCallPhase::SeatActive {
            return Err(BoundaryViolation);
        }
        if let Err(send_err) = self.send(request, reservation) {
            if let Some(event) = self.poll_reply() {
                match event {
                    HostCallEvent::Outcome { outcome, .. } => return Ok(outcome),
                    HostCallEvent::LateReply { outcome, .. } => return Ok(outcome),
                }
            }
            return Ok(match send_err {
                SendError::Reaped => HostCallOutcome::Unknown(UnknownReason::HelperExited),
                SendError::Stalled | SendError::Ipc => {
                    HostCallOutcome::Unknown(UnknownReason::IpcFailure)
                }
                _ => HostCallOutcome::Unknown(UnknownReason::IpcFailure),
            });
        }

        let deadline = match self.next_deadline() {
            Some(d) => d,
            None => {
                self.state = ExecutorState::Stalled;
                self.request_termination();
                return Ok(HostCallOutcome::Unknown(UnknownReason::WatchdogExpired));
            }
        };

        loop {
            let now = Instant::now();
            if now >= deadline {
                if let Some(event) = self.tick(now) {
                    match event {
                        HostCallEvent::Outcome { outcome, .. } => return Ok(outcome),
                        HostCallEvent::LateReply { outcome, .. } => return Ok(outcome),
                    }
                }
                if let Some(event) = self.terminalize_unknown(UnknownReason::WatchdogExpired) {
                    match event {
                        HostCallEvent::Outcome { outcome, .. } => return Ok(outcome),
                        HostCallEvent::LateReply { outcome, .. } => return Ok(outcome),
                    }
                }
                return Ok(HostCallOutcome::Unknown(UnknownReason::WatchdogExpired));
            }

            match wait_readable_bounded(self.control.as_raw_fd(), deadline) {
                Ok(true) => {
                    if let Some(event) = self.poll_reply() {
                        match event {
                            HostCallEvent::Outcome { outcome, .. } => return Ok(outcome),
                            HostCallEvent::LateReply { outcome, .. } => return Ok(outcome),
                        }
                    }
                }
                Ok(false) => {
                    let now = Instant::now();
                    if let Some(event) = self.tick(now) {
                        match event {
                            HostCallEvent::Outcome { outcome, .. } => return Ok(outcome),
                            HostCallEvent::LateReply { outcome, .. } => return Ok(outcome),
                        }
                    }
                    if let Some(event) = self.terminalize_unknown(UnknownReason::WatchdogExpired) {
                        match event {
                            HostCallEvent::Outcome { outcome, .. } => return Ok(outcome),
                            HostCallEvent::LateReply { outcome, .. } => return Ok(outcome),
                        }
                    }
                    return Ok(HostCallOutcome::Unknown(UnknownReason::WatchdogExpired));
                }
                Err(e) => {
                    let reason = self.classify_channel_loss(Some(&e));
                    if let Some(event) = self.terminalize_unknown(reason) {
                        match event {
                            HostCallEvent::Outcome { outcome, .. } => return Ok(outcome),
                            HostCallEvent::LateReply { outcome, .. } => return Ok(outcome),
                        }
                    }
                    return Ok(HostCallOutcome::Unknown(reason));
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn dispatch(
        &mut self,
        request: &HostCallRequest,
        proof: SubmittingProof,
    ) -> HostCallOutcome {
        self.dispatch_blocking_at_boundary(request, HostCallReservation::Submitting(proof))
            .expect("dispatch called in permitted phase")
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
        // Flags per the class table: NONBLOCK for a live seat-active commit,
        // TEST_ONLY for either validation class, neither for a permitted
        // blocking one. The two validation classes are indistinguishable
        // here by design — only the class field separates their watchdogs.
        // The decoder rejects any frame whose flags contradict its class, so
        // getting this wrong is a protocol error, not a silent mismatch.
        let flags = match class {
            HostCallClass::SeatActiveNonblock => protocol::DRM_MODE_ATOMIC_NONBLOCK,
            HostCallClass::SeatActiveValidation | HostCallClass::ColdStartOrOfflineValidation => {
                protocol::DRM_MODE_ATOMIC_TEST_ONLY
            }
            HostCallClass::ColdStartOrOfflineBlocking => 0,
        };
        self.next_seq += 1;
        let request = HostCallRequest::Atomic(AtomicRequest {
            correlation: HostCallCorrelation::Atomic {
                seq: RequestSeq::for_tests(self.next_seq),
                incarnation: self.incarnation,
                lifecycle_epoch: LifecycleEpochId::first(),
                transition: None,
                commit: CommitId::for_tests(1),
                // Tagged, not `for_tests`: the decoder checks the purpose
                // tag, so an untagged token is rejected on arrival and the
                // helper exits with a protocol error instead of answering.
                event_token: EventToken::tagged_for_tests(1),
            },
            class,
            flags,
            properties: AtomicPropertyList {
                objects: Vec::new(),
                count_props: Vec::new(),
                props: Vec::new(),
                values: Vec::new(),
            },
            out_fence_slots: Vec::new(),
        });
        let reservation = match class.is_validation() {
            true => HostCallReservation::Validation(ValidationLease::for_tests()),
            false => HostCallReservation::Submitting(SubmittingProof::for_tests()),
        };
        self.dispatch_blocking_at_boundary(&request, reservation)
            .expect("dispatch_for_tests called in cold start")
    }
}

pub(crate) fn wait_readable_bounded(fd: RawFd, deadline: Instant) -> io::Result<bool> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        let remaining = deadline - now;
        let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as c_int;

        let mut pfd = pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };

        // SAFETY: pfd points to 1 valid pollfd on the stack.
        let poll_rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if poll_rc == 0 {
            return Ok(false);
        }
        if poll_rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        return Ok(true);
    }
}

impl Drop for KmsIoExecutor {
    fn drop(&mut self) {
        if self.check_child_exited() {
            return;
        }
        let _ = self.child.kill();
        if self.check_child_exited() {
            return;
        }
        log::warn!(
            "kms executor: helper pid {} unreaped at drop; incarnation {:?} lease not proven \
             released, leaving it orphaned rather than blocking the core",
            self.child.id(),
            self.incarnation
        );
    }
}

#[doc(hidden)]
pub fn executor_executable() -> io::Result<PathBuf> {
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
    spawn_internal_full(
        executable,
        kms_fd,
        incarnation,
        LifecycleEpochId::first(),
        None,
        stub,
        false,
        false,
    )
}

pub(crate) fn spawn_internal_full(
    executable: &std::path::Path,
    kms_fd: BorrowedFd<'_>,
    incarnation: IncarnationId,
    lifecycle_epoch: LifecycleEpochId,
    lock_fd: Option<BorrowedFd<'_>>,
    stub: Option<test_support::StubBehaviour>,
    disarm_pdeathsig: bool,
    null_stderr: bool,
) -> io::Result<KmsIoExecutor> {
    let (parent_control, child_control) = seqpacket_pair()?;
    parent_control.set_nonblocking(true)?;
    let inherited_control = duplicate_fd_at_least(child_control.as_fd())?;
    let inherited_kms = duplicate_fd_at_least(kms_fd)?;
    let control_source = inherited_control.as_raw_fd();
    let kms_source = inherited_kms.as_raw_fd();
    let inherited_lock = match lock_fd {
        Some(fd) => Some(duplicate_fd_at_least(fd)?),
        None => None,
    };
    let lock_source = inherited_lock.as_ref().map(|fd| fd.as_raw_fd());

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
    if null_stderr {
        command.stderr(Stdio::null());
    }

    let ignore_termination = match stub {
        Some(test_support::StubBehaviour::IgnoreTermination) => true,
        Some(test_support::StubBehaviour::AcceptAfterReturningInheritedFd {
            ignore_termination,
            ..
        }) => ignore_termination,
        Some(test_support::StubBehaviour::WedgedHoldingLock) => true,
        Some(test_support::StubBehaviour::ReplyTwiceWith(_)) => true,
        _ => false,
    };

    // SAFETY: pre_exec runs only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            if !disarm_pdeathsig {
                arm_helper_parent_death_signal(supervisor_pid)?;
            }
            if ignore_termination {
                #[cfg(unix)]
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            duplicate_to_inherited_slot(control_source, CONTROL_FD)?;
            duplicate_to_inherited_slot(kms_source, KMS_FD)?;
            if let Some(lock_src) = lock_source {
                duplicate_to_inherited_slot(lock_src, LOCK_FD)?;
            }
            Ok(())
        });
    }

    let child = command.spawn()?;
    drop(child_control);
    drop(inherited_control);
    drop(inherited_kms);
    drop(inherited_lock);

    Ok(KmsIoExecutor {
        child,
        control: parent_control,
        incarnation,
        lifecycle_epoch,
        termination_requested: false,
        reaped: None,
        state: ExecutorState::Live,
        reap_proof: None,
        reap_proof_taken: false,
        next_seq: 0,
        phase: HostCallPhase::ColdStart,
        in_flight: None,
        queued_terminal_event: None,
        helper_pid: None,
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
    fn each_host_call_class_carries_the_watchdog_the_spec_assigns_it() {
        // spec:320-329 — seat-active validation is two seconds, cold-start or
        // offline validation is thirty. Deriving the class from the NONBLOCK
        // bit gave every TEST_ONLY request the thirty-second watchdog.
        assert_eq!(
            HostCallClass::SeatActiveNonblock.watchdog(),
            Duration::from_secs(2)
        );
        assert_eq!(
            HostCallClass::SeatActiveValidation.watchdog(),
            Duration::from_secs(2)
        );
        assert_eq!(
            HostCallClass::ColdStartOrOfflineBlocking.watchdog(),
            Duration::from_secs(30)
        );
        assert_eq!(
            HostCallClass::ColdStartOrOfflineValidation.watchdog(),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn only_the_validation_classes_report_is_validation() {
        assert!(HostCallClass::SeatActiveValidation.is_validation());
        assert!(HostCallClass::ColdStartOrOfflineValidation.is_validation());
        assert!(!HostCallClass::SeatActiveNonblock.is_validation());
        assert!(!HostCallClass::ColdStartOrOfflineBlocking.is_validation());
    }

    #[test]
    fn the_host_call_class_round_trips_through_its_wire_tag() {
        for class in [
            HostCallClass::SeatActiveNonblock,
            HostCallClass::SeatActiveValidation,
            HostCallClass::ColdStartOrOfflineBlocking,
            HostCallClass::ColdStartOrOfflineValidation,
        ] {
            assert_eq!(HostCallClass::from_wire_tag(class.wire_tag()), Some(class));
        }
        // Zero must not decode, so a zeroed byte is never a valid class, and
        // neither must a tag past the last variant.
        assert_eq!(HostCallClass::from_wire_tag(0), None);
        assert_eq!(HostCallClass::from_wire_tag(5), None);
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
        test_support::reap_within(&mut executor, Duration::from_secs(5));
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
        test_support::reap_within(&mut executor, Duration::from_secs(5));
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
    fn early_take_reap_proof_returns_none_and_does_not_invalidate_future_proof() {
        let mut fds = IncarnationFdSet::default();
        let lease = fds.register_alias(std::fs::File::open("/dev/null").expect("open").into());
        let mut executor = stub_executor(StubBehaviour::ExitBeforeReply, lease);
        // Process has not replied or exited yet before dispatch
        assert!(executor.take_reap_proof().is_none());
        assert_eq!(executor.state(), ExecutorState::Live);

        let _ = executor.dispatch_for_tests(HostCallClass::SeatActiveNonblock);
        let reap = executor.try_reap();
        assert!(matches!(reap, ReapState::Reaped(_)));
        let proof = executor
            .take_reap_proof()
            .expect("reap proof must still be available");
        assert_eq!(fds.release_with_proof(lease, proof), Ok(()));
        assert_eq!(fds.outstanding(), 0);
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
