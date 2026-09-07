//! Process-isolated KMS executor re-exec helper loop.

use std::{
    env,
    ffi::OsStr,
    io,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::net::UnixStream,
    },
    time::Instant,
};

#[cfg(test)]
use super::protocol::{AtomicPropertyList, OutFenceSlot};
use super::{
    CONTROL_FD, KMS_FD, LOCK_FD, REEXEC_ARG,
    protocol::{
        self, AtomicRequest, HostCallCorrelation, HostCallReply, HostCallRequest,
        MAX_REQUEST_FRAME_LEN,
    },
    take_inherited_fd,
    transport::{self, send_reply_with_fences},
};
use crate::platform::ioctl::{DRM_IOCTL_BASE, IoctlReq, iowr};

#[repr(C)]
pub(crate) struct DrmModeAtomic {
    pub(crate) flags: u32,
    pub(crate) count_objs: u32,
    pub(crate) objs_ptr: u64,
    pub(crate) count_props_ptr: u64,
    pub(crate) props_ptr: u64,
    pub(crate) prop_values_ptr: u64,
    pub(crate) reserved: u64,
    pub(crate) user_data: u64,
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

const DRM_CRTC_SEQUENCE_RELATIVE: u32 = 0x0000_0001;
const DRM_CRTC_SEQUENCE_NEXT_ON_MISS: u32 = 0x0000_0002;

#[repr(C)]
struct DrmCrtcQueueSequence {
    crtc_id: u32,
    flags: u32,
    sequence: u64,
    user_data: u64,
}

const DRM_IOCTL_CRTC_QUEUE_SEQUENCE: IoctlReq = iowr(
    DRM_IOCTL_BASE,
    0x3C,
    std::mem::size_of::<DrmCrtcQueueSequence>(),
);

pub(crate) fn queue_crtc_sequence(
    fd: BorrowedFd<'_>,
    crtc_id: u32,
    relative: bool,
    sequence: u64,
    user_data: u64,
) -> io::Result<u64> {
    let mut flags = DRM_CRTC_SEQUENCE_NEXT_ON_MISS;
    if relative {
        flags |= DRM_CRTC_SEQUENCE_RELATIVE;
    }
    let mut req = DrmCrtcQueueSequence {
        crtc_id,
        flags,
        sequence,
        user_data,
    };
    // SAFETY: req is properly initialized POD for DRM_IOCTL_CRTC_QUEUE_SEQUENCE.
    let rc = unsafe {
        libc::ioctl(
            fd.as_raw_fd(),
            DRM_IOCTL_CRTC_QUEUE_SEQUENCE,
            std::ptr::addr_of_mut!(req),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(req.sequence)
}

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
    let _lock = if unsafe { libc::fcntl(LOCK_FD, libc::F_GETFD) } >= 0 {
        let lock = take_inherited_fd(LOCK_FD, "executor device lock")?;
        let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Some(lock)
    } else {
        None
    };
    let control = UnixStream::from(control_fd);
    serve_executor_loop(&control, kms_fd.as_fd())
}

fn serve_executor_loop(control: &UnixStream, kms_fd: BorrowedFd<'_>) -> io::Result<()> {
    // Heap, not stack: requests are variable-length now and the cap is
    // 32 KiB, which is not worth putting on the stack of every receive.
    let mut req_buf = vec![0u8; MAX_REQUEST_FRAME_LEN];
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

        if let Ok(handshake) = protocol::decode_handshake_request(&req_buf[..received.len]) {
            let reply = protocol::HandshakeReply {
                incarnation: handshake.incarnation,
                lifecycle_epoch: handshake.lifecycle_epoch,
                helper_pid: unsafe { libc::getpid() as u32 },
            };
            let rep_frame = protocol::encode_handshake_reply(&reply);
            transport::send_frame(control, &rep_frame)?;
            continue;
        }

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

pub(crate) struct PreparedAtomic {
    pub(crate) flags: u32,
    pub(crate) user_data: u64,
    pub(crate) objects: Vec<u32>,
    pub(crate) count_props: Vec<u32>,
    pub(crate) props: Vec<u32>,
    pub(crate) values: Vec<u64>,
    pub(crate) holders: Vec<i32>,
}

impl PreparedAtomic {
    #[cfg(test)]
    pub(crate) fn holder_address(&self, slot_idx: usize) -> u64 {
        std::ptr::from_ref(&self.holders[slot_idx]) as usize as u64
    }

    pub(crate) fn build_drm_request(&self) -> DrmModeAtomic {
        DrmModeAtomic {
            flags: self.flags,
            count_objs: self.objects.len() as u32,
            objs_ptr: if self.objects.is_empty() {
                0
            } else {
                self.objects.as_ptr() as usize as u64
            },
            count_props_ptr: if self.count_props.is_empty() {
                0
            } else {
                self.count_props.as_ptr() as usize as u64
            },
            props_ptr: if self.props.is_empty() {
                0
            } else {
                self.props.as_ptr() as usize as u64
            },
            prop_values_ptr: if self.values.is_empty() {
                0
            } else {
                self.values.as_ptr() as usize as u64
            },
            reserved: 0,
            user_data: self.user_data,
        }
    }
}

pub(crate) fn prepare_atomic(atomic: &AtomicRequest) -> PreparedAtomic {
    let HostCallCorrelation::Atomic { event_token, .. } = atomic.correlation else {
        return PreparedAtomic {
            flags: atomic.flags,
            user_data: 0,
            objects: atomic.properties.objects.clone(),
            count_props: atomic.properties.count_props.clone(),
            props: atomic.properties.props.clone(),
            values: atomic.properties.values.clone(),
            holders: vec![-1; atomic.out_fence_slots.len()],
        };
    };
    let mut prepared = PreparedAtomic {
        flags: atomic.flags,
        user_data: event_token.as_user_data(),
        objects: atomic.properties.objects.clone(),
        count_props: atomic.properties.count_props.clone(),
        props: atomic.properties.props.clone(),
        values: atomic.properties.values.clone(),
        // Allocated to its final length BEFORE any address is taken. A later
        // push would reallocate and invalidate every installed pointer.
        holders: vec![-1; atomic.out_fence_slots.len()],
    };
    for (slot_idx, slot) in atomic.out_fence_slots.iter().enumerate() {
        let holder: *mut i32 = &mut prepared.holders[slot_idx];
        if let Some(val) = prepared.values.get_mut(slot.value_index as usize) {
            *val = holder as usize as u64;
        }
    }
    prepared
}

fn atomic_ioctl(fd: RawFd, req: &mut DrmModeAtomic) -> i32 {
    // SAFETY: req is properly initialized for DRM_IOCTL_MODE_ATOMIC.
    unsafe { libc::ioctl(fd, DRM_IOCTL_MODE_ATOMIC, req as *mut DrmModeAtomic) }
}

fn close_holder(fd: i32) {
    #[cfg(test)]
    if HolderLedger::is_installed() {
        HolderLedger::record_close(fd);
        return;
    }
    // SAFETY: fd is a valid non-negative file descriptor.
    unsafe {
        libc::close(fd);
    }
}

fn execute_atomic(kms_fd: BorrowedFd<'_>, atomic: &AtomicRequest) -> (HostCallReply, Vec<OwnedFd>) {
    let HostCallCorrelation::Atomic { .. } = atomic.correlation else {
        return (
            HostCallReply::Rejected {
                correlation: atomic.correlation,
                errno: libc::EINVAL,
                helper_duration_ns: 0,
                unexpected_fence_output: false,
            },
            Vec::new(),
        );
    };

    let prepared = prepare_atomic(atomic);
    let mut atomic_req = prepared.build_drm_request();

    let started = Instant::now();
    let rc = atomic_ioctl(kms_fd.as_raw_fd(), &mut atomic_req);
    let helper_duration_ns = elapsed_ns(started);

    if rc == 0 {
        let mut fences = Vec::new();
        let mut out_fence_mask: u32 = 0;
        for (i, &holder) in prepared.holders.iter().enumerate() {
            if holder >= 0 {
                out_fence_mask |= 1 << i;
                // SAFETY: Kernel verified success and populated holder with a valid new file descriptor.
                fences.push(unsafe { OwnedFd::from_raw_fd(holder) });
            }
        }
        (
            HostCallReply::Accepted {
                correlation: atomic.correlation,
                helper_duration_ns,
                out_fence_mask,
            },
            fences,
        )
    } else {
        let errno = io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO);
        let mut unexpected_fence_output = false;
        for &holder in &prepared.holders {
            if holder >= 0 {
                unexpected_fence_output = true;
                close_holder(holder);
            }
        }
        (
            HostCallReply::Rejected {
                correlation: atomic.correlation,
                errno,
                helper_duration_ns,
                unexpected_fence_output,
            },
            Vec::new(),
        )
    }
}

fn execute_host_call(
    kms_fd: BorrowedFd<'_>,
    request: &HostCallRequest,
) -> (HostCallReply, Vec<OwnedFd>) {
    match request {
        HostCallRequest::Atomic(atomic) => execute_atomic(kms_fd, atomic),
        HostCallRequest::ClockProbe(probe) => {
            let HostCallCorrelation::ClockProbe { hardware_crtc, .. } = probe.correlation else {
                return (
                    HostCallReply::ProbeRejected {
                        correlation: probe.correlation,
                        errno: libc::EINVAL,
                        helper_duration_ns: 0,
                    },
                    Vec::new(),
                );
            };
            let mut get_seq = DrmCrtcGetSequence {
                crtc_id: hardware_crtc,
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
                    HostCallReply::ProbeAccepted {
                        correlation: probe.correlation,
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
                    // ProbeRejected, not Rejected: EOPNOTSUPP here is how 2b
                    // learns a CRTC is structurally incapable, and a reply
                    // from the atomic family would be rejected as malformed
                    // by the executor's family check.
                    HostCallReply::ProbeRejected {
                        correlation: probe.correlation,
                        errno,
                        helper_duration_ns,
                    },
                    Vec::new(),
                )
            }
        }
        HostCallRequest::SequenceQueue(queue_req) => {
            let HostCallCorrelation::SequenceQueue {
                hardware_crtc,
                token,
                ..
            } = queue_req.correlation
            else {
                return (
                    HostCallReply::QueueRejected {
                        correlation: queue_req.correlation,
                        errno: libc::EINVAL,
                        helper_duration_ns: 0,
                    },
                    Vec::new(),
                );
            };
            let started = Instant::now();
            match queue_crtc_sequence(
                kms_fd,
                hardware_crtc,
                queue_req.relative,
                queue_req.sequence,
                token.as_user_data(),
            ) {
                Ok(scheduled_seq) => {
                    let helper_duration_ns = elapsed_ns(started);
                    (
                        HostCallReply::QueueAccepted {
                            correlation: queue_req.correlation,
                            sequence: scheduled_seq,
                            helper_duration_ns,
                        },
                        Vec::new(),
                    )
                }
                Err(err) => {
                    let helper_duration_ns = elapsed_ns(started);
                    let errno = err.raw_os_error().unwrap_or(libc::EIO);
                    (
                        HostCallReply::QueueRejected {
                            correlation: queue_req.correlation,
                            errno,
                            helper_duration_ns,
                        },
                        Vec::new(),
                    )
                }
            }
        }
    }
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
pub(crate) struct HolderLedger {
    _private: (),
}

#[cfg(test)]
static HOLDER_CLOSES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static HOLDER_LEDGER_INSTALLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
impl HolderLedger {
    pub(crate) fn install() -> Self {
        HOLDER_CLOSES.store(0, std::sync::atomic::Ordering::SeqCst);
        HOLDER_LEDGER_INSTALLED.store(true, std::sync::atomic::Ordering::SeqCst);
        Self { _private: () }
    }

    pub(crate) fn is_installed() -> bool {
        HOLDER_LEDGER_INSTALLED.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn record_close(_fd: i32) {
        HOLDER_CLOSES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn closes(&self) -> usize {
        HOLDER_CLOSES.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Drop for HolderLedger {
    fn drop(&mut self) {
        HOLDER_LEDGER_INSTALLED.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
pub(crate) enum ScriptedIoctl<'a> {
    Accepted {
        holder_writes: &'a [Option<i32>],
    },
    Rejected {
        errno: i32,
        holder_writes: &'a [i32],
    },
}

#[cfg(test)]
pub(crate) fn execute_atomic_with_scripted_result_for_tests(
    scripted: ScriptedIoctl<'_>,
) -> (HostCallReply, Vec<OwnedFd>) {
    let num_slots = match &scripted {
        ScriptedIoctl::Accepted { holder_writes } => holder_writes.len(),
        ScriptedIoctl::Rejected { holder_writes, .. } => holder_writes.len(),
    };
    let mut objects = Vec::new();
    let mut count_props = Vec::new();
    let mut props = Vec::new();
    let mut values = Vec::new();
    let mut out_fence_slots = Vec::new();
    for i in 0..num_slots {
        objects.push(i as u32 + 1);
        count_props.push(1);
        props.push(i as u32 + 10);
        values.push(0);
        out_fence_slots.push(OutFenceSlot {
            crtc_id: i as u32 + 1,
            value_index: i as u32,
        });
    }
    let atomic = AtomicRequest {
        correlation: HostCallCorrelation::Atomic {
            seq: crate::kms::executor::protocol::RequestSeq::for_tests(1),
            incarnation: crate::kms::owner::identity::IncarnationId::first(),
            lifecycle_epoch: crate::kms::owner::lifecycle::LifecycleEpochId::first(),
            transition: None,
            commit: crate::kms::owner::identity::CommitId::for_tests(1),
            event_token: crate::kms::owner::identity::EventToken::for_tests(1),
        },
        class: crate::kms::executor::HostCallClass::SeatActiveNonblock,
        flags: crate::kms::executor::protocol::DRM_MODE_ATOMIC_NONBLOCK,
        properties: AtomicPropertyList {
            objects,
            count_props,
            props,
            values,
        },
        out_fence_slots,
    };

    let mut prepared = prepare_atomic(&atomic);
    let mut _atomic_req = prepared.build_drm_request();

    match scripted {
        ScriptedIoctl::Accepted { holder_writes } => {
            for (i, write) in holder_writes.iter().enumerate() {
                if let Some(fd_val) = write {
                    prepared.holders[i] = *fd_val;
                }
            }
            let mut fences = Vec::new();
            let mut out_fence_mask: u32 = 0;
            for (i, &holder) in prepared.holders.iter().enumerate() {
                if holder >= 0 {
                    out_fence_mask |= 1 << i;
                    let dummy_fd = std::fs::File::open("/dev/null").expect("open").into();
                    fences.push(dummy_fd);
                }
            }
            (
                HostCallReply::Accepted {
                    correlation: atomic.correlation,
                    helper_duration_ns: 1_000,
                    out_fence_mask,
                },
                fences,
            )
        }
        ScriptedIoctl::Rejected {
            errno,
            holder_writes,
        } => {
            for (i, &write_val) in holder_writes.iter().enumerate() {
                if i < prepared.holders.len() {
                    prepared.holders[i] = write_val;
                }
            }
            let mut unexpected_fence_output = false;
            for &holder in &prepared.holders {
                if holder >= 0 {
                    unexpected_fence_output = true;
                    close_holder(holder);
                }
            }
            (
                HostCallReply::Rejected {
                    correlation: atomic.correlation,
                    errno,
                    helper_duration_ns: 1_000,
                    unexpected_fence_output,
                },
                Vec::new(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::{
        executor::{
            HostCallClass,
            protocol::{DRM_MODE_ATOMIC_NONBLOCK, RequestSeq},
        },
        owner::{
            identity::{CommitId, EventToken, IncarnationId},
            lifecycle::LifecycleEpochId,
        },
    };

    fn atomic_request_for_tests(
        properties: AtomicPropertyList,
        slots: &[OutFenceSlot],
    ) -> AtomicRequest {
        AtomicRequest {
            correlation: HostCallCorrelation::Atomic {
                seq: RequestSeq::for_tests(1),
                incarnation: IncarnationId::first(),
                lifecycle_epoch: LifecycleEpochId::first(),
                transition: None,
                commit: CommitId::for_tests(1),
                event_token: EventToken::for_tests(1),
            },
            class: HostCallClass::SeatActiveNonblock,
            flags: DRM_MODE_ATOMIC_NONBLOCK,
            properties,
            out_fence_slots: slots.to_vec(),
        }
    }

    fn large_property_list_for_tests() -> AtomicPropertyList {
        let n = 50;
        AtomicPropertyList {
            objects: (1..=n).collect(),
            count_props: vec![1; n as usize],
            props: (1..=n).collect(),
            values: (1..=n as u64).collect(),
        }
    }

    fn prepare_atomic_for_tests(atomic: &AtomicRequest) -> PreparedAtomic {
        prepare_atomic(atomic)
    }

    struct CapturedSubmittedIoctl {
        count_objs: u32,
        objs_ptr: u64,
        objects: Vec<u32>,
        count_props: Vec<u32>,
        props: Vec<u32>,
        values: Vec<u64>,
    }

    impl CapturedSubmittedIoctl {
        fn objs_as_slice(&self) -> &[u32] {
            &self.objects
        }
        fn count_props_as_slice(&self) -> &[u32] {
            &self.count_props
        }
        fn props_as_slice(&self) -> &[u32] {
            &self.props
        }
        fn values_as_slice(&self) -> &[u64] {
            &self.values
        }
    }

    fn capture_submitted_ioctl_for_tests(atomic: &AtomicRequest) -> CapturedSubmittedIoctl {
        let prepared = prepare_atomic(atomic);
        let drm_req = prepared.build_drm_request();
        CapturedSubmittedIoctl {
            count_objs: drm_req.count_objs,
            objs_ptr: drm_req.objs_ptr,
            objects: prepared.objects.clone(),
            count_props: prepared.count_props.clone(),
            props: prepared.props.clone(),
            values: prepared.values.clone(),
        }
    }

    #[test]
    fn every_out_fence_slot_is_patched_with_the_address_of_its_live_holder() {
        let atomic = atomic_request_for_tests(
            AtomicPropertyList {
                objects: vec![0],
                count_props: vec![2],
                props: vec![0, 1],
                values: vec![0xdead_beef, 0],
            },
            &[OutFenceSlot {
                crtc_id: 0,
                value_index: 1,
            }],
        );
        let prepared = prepare_atomic_for_tests(&atomic);
        assert_eq!(
            prepared.values[0], 0xdead_beef,
            "untouched entries survive verbatim"
        );
        assert_eq!(prepared.values[1], prepared.holder_address(0));
    }

    #[test]
    fn the_prepared_arrays_are_not_reallocated_after_addresses_are_installed() {
        let atomic = atomic_request_for_tests(large_property_list_for_tests(), &[]);
        let prepared = prepare_atomic_for_tests(&atomic);
        let before = (prepared.objects.as_ptr(), prepared.values.as_ptr());
        let req = prepared.build_drm_request();
        assert_eq!(
            (prepared.objects.as_ptr(), prepared.values.as_ptr()),
            before
        );
        assert_eq!(req.objs_ptr, prepared.objects.as_ptr() as usize as u64);
        assert_eq!(req.count_objs as usize, prepared.objects.len());
    }

    #[test]
    fn a_rejected_ioctl_closes_an_unexpected_holder_exactly_once_and_reports_it() {
        let ledger = HolderLedger::install();
        let (reply, fences) =
            execute_atomic_with_scripted_result_for_tests(ScriptedIoctl::Rejected {
                errno: libc::EINVAL,
                holder_writes: &[0],
            });
        assert!(fences.is_empty());
        assert_eq!(ledger.closes(), 1);
        assert!(matches!(
            reply,
            HostCallReply::Rejected {
                unexpected_fence_output: true,
                ..
            }
        ));
    }

    #[test]
    fn a_live_success_with_a_missing_holder_reports_the_gap_rather_than_repairing_it() {
        let (reply, fences) =
            execute_atomic_with_scripted_result_for_tests(ScriptedIoctl::Accepted {
                holder_writes: &[Some(7), None],
            });
        assert_eq!(fences.len(), 1);
        assert!(matches!(
            reply,
            HostCallReply::Accepted {
                out_fence_mask: 0b01,
                ..
            }
        ));
    }

    #[test]
    fn the_submitted_ioctl_argument_is_the_prepared_arrays() {
        let atomic = atomic_request_for_tests(
            AtomicPropertyList {
                objects: vec![31, 42],
                count_props: vec![1, 2],
                props: vec![7, 8, 9],
                values: vec![1, 2, 3],
            },
            &[OutFenceSlot {
                crtc_id: 31,
                value_index: 0,
            }],
        );
        let captured = capture_submitted_ioctl_for_tests(&atomic);
        assert_eq!(
            captured.count_objs, 2,
            "the object count actually submitted"
        );
        assert_eq!(captured.objs_as_slice(), &[31, 42]);
        assert_eq!(captured.count_props_as_slice(), &[1, 2]);
        assert_eq!(captured.props_as_slice(), &[7, 8, 9]);
        assert_eq!(captured.values_as_slice().len(), 3);
        assert_ne!(captured.objs_ptr, 0, "not a null pointer");
    }
}
