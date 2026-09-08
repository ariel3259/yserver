//! Canonical fence query and ownership interfaces.

use std::{io, os::fd::BorrowedFd};

pub use crate::platform::sync_file::FenceStatus;

/// Trait for querying explicit fence descriptor status.
pub trait FenceQuery {
    fn status(&mut self, fd: BorrowedFd<'_>) -> io::Result<FenceStatus>;
}

/// Canonical fence query delegating to the platform sync_file wrapper.
#[derive(Debug, Default, Clone, Copy)]
pub struct CanonicalFenceQuery;

impl FenceQuery for CanonicalFenceQuery {
    fn status(&mut self, fd: BorrowedFd<'_>) -> io::Result<FenceStatus> {
        crate::platform::sync_file::query_status(fd)
    }
}

/// Cross-platform readiness set interface for fence polling.
pub trait FencePollSet {
    fn register(&mut self, fd: BorrowedFd<'_>, token: u64) -> io::Result<()>;
    fn unregister(&mut self, fd: BorrowedFd<'_>) -> io::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::{
        executor::{HostCallEvent, HostCallOutcome},
        owner::{
            device::{MechanismFailure, OwnerEvent},
            test_fixtures::*,
        },
    };
    use nix::sys::socket::{AddressFamily, SockFlag, SockType, socketpair};
    use std::{
        cell::Cell,
        os::fd::{AsRawFd, RawFd},
        time::Instant,
    };

    struct MockQuery<F> {
        f: F,
    }

    impl<F: FnMut(BorrowedFd<'_>) -> io::Result<FenceStatus>> FenceQuery for MockQuery<F> {
        fn status(&mut self, fd: BorrowedFd<'_>) -> io::Result<FenceStatus> {
            (self.f)(fd)
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum PollAction {
        Register(RawFd, u64),
        Unregister(RawFd),
    }

    struct TrackingPollSet {
        actions: Vec<PollAction>,
        fail_register: bool,
        fail_unregister: bool,
    }

    impl TrackingPollSet {
        fn new() -> Self {
            Self {
                actions: Vec::new(),
                fail_register: false,
                fail_unregister: false,
            }
        }
    }

    impl FencePollSet for TrackingPollSet {
        fn register(&mut self, fd: BorrowedFd<'_>, token: u64) -> io::Result<()> {
            self.actions
                .push(PollAction::Register(fd.as_raw_fd(), token));
            if self.fail_register {
                Err(io::Error::from_raw_os_error(libc::ENOMEM))
            } else {
                Ok(())
            }
        }

        fn unregister(&mut self, fd: BorrowedFd<'_>) -> io::Result<()> {
            self.actions.push(PollAction::Unregister(fd.as_raw_fd()));
            if self.fail_unregister {
                Err(io::Error::from_raw_os_error(libc::EIO))
            } else {
                Ok(())
            }
        }
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Immediate fake Success closes the sole writer
    /// with ZERO register/unregister calls and reader observes EOF.
    /// Two successful slots emit HardwareComplete exactly once, never Presented.
    #[test]
    fn immediate_success_emits_hardware_complete_without_poll_set_calls() {
        let mut owner = owner_for_tests();
        let desc = two_active_crtcs();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (r1, w1) = nix::unistd::pipe().expect("pipe1");
        let (r2, w2) = nix::unistd::pipe().expect("pipe2");

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b11,
                out_fences: vec![w1, w2],
            },
        };
        owner.apply_host_call_event(event);

        let mut query = MockQuery {
            f: |_fd: BorrowedFd<'_>| Ok(FenceStatus::Success),
        };
        let mut poll_set = TrackingPollSet::new();
        let now = Instant::now();

        let events = owner.observe_fences(&mut query, &mut poll_set, now);

        assert!(
            poll_set.actions.is_empty(),
            "immediate success must invoke ZERO register/unregister calls"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::HardwareComplete { commit: c } if *c == commit)),
            "must emit HardwareComplete"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, OwnerEvent::Presented { .. })),
            "must NEVER emit Presented"
        );

        // Pipe readers must observe EOF immediately because writers were closed
        let mut buf = [0u8; 1];
        let n1 = nix::unistd::read(&r1, &mut buf).expect("read r1");
        assert_eq!(n1, 0, "reader 1 observes EOF on immediate close");
        let n2 = nix::unistd::read(&r2, &mut buf).expect("read r2");
        assert_eq!(n2, 0, "reader 2 observes EOF on immediate close");
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Partial mask quarantines at ingress and retains descriptors.
    #[test]
    fn partial_mask_quarantines_and_retains_descriptors() {
        let mut owner = owner_for_tests();
        let desc = two_active_crtcs();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (_r, w) = nix::unistd::pipe().expect("pipe");

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b01,
                out_fences: vec![w],
            },
        };
        let ingress_events = owner.apply_host_call_event(event);
        assert!(
            ingress_events
                .iter()
                .any(|e| matches!(e, OwnerEvent::Quarantined { commit: c } if *c == commit)),
            "partial mask must quarantine at ingress"
        );

        let mut query = MockQuery {
            f: |_fd: BorrowedFd<'_>| Ok(FenceStatus::Success),
        };
        let mut poll_set = TrackingPollSet::new();
        let events = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
        assert!(
            events.is_empty(),
            "quarantined record produces no observation events"
        );
        assert!(
            poll_set.actions.is_empty(),
            "quarantined record produces no poll registrations"
        );

        let record = owner.live_record().unwrap();
        assert_eq!(record.fence_evidence().unwrap().by_crtc().len(), 1);
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Full mask with one error quarantines, unregisters,
    /// emits MechanismFailed, and retains owned fds in quarantine.
    #[test]
    fn full_mask_with_one_error_quarantines_and_emits_mechanism_failed() {
        let mut owner = owner_for_tests();
        let desc = two_active_crtcs();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (r1, w1) = nix::unistd::pipe().expect("pipe1");
        let (r2, w2) = nix::unistd::pipe().expect("pipe2");

        let w1_raw = w1.as_raw_fd();

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b11,
                out_fences: vec![w1, w2],
            },
        };
        owner.apply_host_call_event(event);

        let mut query = MockQuery {
            f: move |fd: BorrowedFd<'_>| {
                if fd.as_raw_fd() == w1_raw {
                    Ok(FenceStatus::Success)
                } else {
                    Ok(FenceStatus::Error(-libc::EIO))
                }
            },
        };
        let mut poll_set = TrackingPollSet::new();
        let events = owner.observe_fences(&mut query, &mut poll_set, Instant::now());

        assert!(
            events.iter().any(|e| matches!(
                e,
                OwnerEvent::MechanismFailed {
                    reason: MechanismFailure::FenceError
                }
            )),
            "must emit MechanismFailed(FenceError)"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::Quarantined { commit: c } if *c == commit)),
            "must emit Quarantined"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, OwnerEvent::HardwareComplete { .. })),
            "must NOT emit HardwareComplete on error"
        );

        // Slot 1 closed because it succeeded
        let mut buf = [0u8; 1];
        let n1 = nix::unistd::read(&r1, &mut buf).expect("read r1");
        assert_eq!(n1, 0, "reader 1 observes EOF because writer 1 succeeded");

        // Slot 2 retained in quarantine, so r2 does NOT see EOF
        unsafe {
            let flags = libc::fcntl(r2.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(r2.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        let res2 = nix::unistd::read(&r2, &mut buf);
        assert!(
            res2.is_err(),
            "r2 must not see EOF because writer 2 is retained in quarantine"
        );
    }

    /// [COMMIT-2, COMMIT-6, MULTI] An already-signalled descriptor must progress without
    /// a new readiness edge.
    #[test]
    fn already_signalled_descriptor_progresses_without_new_readiness_edge() {
        let mut owner = owner_for_tests();
        let desc = single_active_crtc();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (_r, w) = nix::unistd::pipe().expect("pipe");

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b1,
                out_fences: vec![w],
            },
        };
        owner.apply_host_call_event(event);

        let mut query = MockQuery {
            f: |_fd: BorrowedFd<'_>| Ok(FenceStatus::Success),
        };
        let mut poll_set = TrackingPollSet::new();
        let events = owner.observe_fences(&mut query, &mut poll_set, Instant::now());

        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::HardwareComplete { commit: c } if *c == commit)),
            "already-signalled descriptor must emit HardwareComplete"
        );
        assert_eq!(poll_set.actions.len(), 0);
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Readable-but-Pending stays pending.
    #[test]
    fn readable_but_pending_stays_pending() {
        let mut owner = owner_for_tests();
        let desc = single_active_crtc();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (sock_owner, sock_peer) = socketpair(
            AddressFamily::Unix,
            SockType::Stream,
            None,
            SockFlag::empty(),
        )
        .expect("socketpair");

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b1,
                out_fences: vec![sock_owner],
            },
        };
        owner.apply_host_call_event(event);

        // Turn 1: query returns Pending -> enters poll_set
        let mut query = MockQuery {
            f: |_fd: BorrowedFd<'_>| Ok(FenceStatus::Pending),
        };
        let mut poll_set = TrackingPollSet::new();
        let events1 = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
        assert!(events1.is_empty());
        assert_eq!(poll_set.actions.len(), 1);
        assert!(matches!(poll_set.actions[0], PollAction::Register(_, _)));

        // Make sock_owner readable by peer write shutdown
        nix::sys::socket::shutdown(sock_peer.as_raw_fd(), nix::sys::socket::Shutdown::Write)
            .expect("shutdown");

        // Turn 2: sock_owner is POLLIN, but query STILL returns Pending!
        let events2 = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
        assert!(events2.is_empty(), "readable but pending stays pending");
        assert_eq!(poll_set.actions.len(), 1, "no unregister call made");
        let record = owner.live_record().unwrap();
        assert!(!record.milestones().hardware_complete);
        let evidence = record.fence_evidence().unwrap();
        assert!(evidence.slots[0].registered, "slot remains registered");
        assert!(!evidence.slots[0].succeeded, "slot is not succeeded");
        assert!(evidence.slots[0].fd.is_some(), "fd is not closed");
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Pending -> Success first registers, then unregisters before close.
    #[test]
    fn pending_then_success_registers_then_unregisters_before_close() {
        let mut owner = owner_for_tests();
        let desc = single_active_crtc();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (sock_owner, sock_peer) = socketpair(
            AddressFamily::Unix,
            SockType::Stream,
            None,
            SockFlag::empty(),
        )
        .expect("socketpair");

        let sock_owner_raw = sock_owner.as_raw_fd();

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b1,
                out_fences: vec![sock_owner],
            },
        };
        owner.apply_host_call_event(event);

        let turn = Cell::new(0);
        let mut query = MockQuery {
            f: |_fd: BorrowedFd<'_>| {
                if turn.get() == 0 {
                    Ok(FenceStatus::Pending)
                } else {
                    Ok(FenceStatus::Success)
                }
            },
        };
        let mut poll_set = TrackingPollSet::new();

        // Turn 1: Pending -> registers
        let events1 = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
        assert!(events1.is_empty());
        assert_eq!(
            poll_set.actions,
            vec![PollAction::Register(sock_owner_raw, 1)]
        );

        // Make readable
        nix::sys::socket::shutdown(sock_peer.as_raw_fd(), nix::sys::socket::Shutdown::Write)
            .expect("shutdown");
        turn.set(1);

        // Turn 2: POLLIN + Success -> unregisters then closes
        let events2 = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
        assert!(
            events2
                .iter()
                .any(|e| matches!(e, OwnerEvent::HardwareComplete { commit: c } if *c == commit)),
            "must emit HardwareComplete"
        );
        assert_eq!(
            poll_set.actions,
            vec![
                PollAction::Register(sock_owner_raw, 1),
                PollAction::Unregister(sock_owner_raw),
            ],
            "first registered, then unregistered before close"
        );

        // Peer observes EOF
        let mut buf = [0u8; 1];
        let n = nix::unistd::read(&sock_peer, &mut buf).expect("read peer");
        assert_eq!(n, 0, "peer observes EOF on close");
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Force register error fails closed without marking registered.
    #[test]
    fn force_register_error_fails_closed() {
        let mut owner = owner_for_tests();
        let desc = single_active_crtc();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (r, w) = nix::unistd::pipe().expect("pipe");

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b1,
                out_fences: vec![w],
            },
        };
        owner.apply_host_call_event(event);

        let mut query = MockQuery {
            f: |_fd: BorrowedFd<'_>| Ok(FenceStatus::Pending),
        };
        let mut poll_set = TrackingPollSet::new();
        poll_set.fail_register = true;

        let events = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
        assert!(
            events.iter().any(|e| matches!(
                e,
                OwnerEvent::MechanismFailed {
                    reason: MechanismFailure::FencePollError
                }
            )),
            "registration error must emit FencePollError"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::Quarantined { commit: c } if *c == commit)),
            "must quarantine"
        );

        let record = owner.live_record().unwrap();
        let evidence = record.fence_evidence().unwrap();
        assert!(
            !evidence.slots[0].registered,
            "fails closed without marking registered"
        );
        assert!(
            evidence.slots[0].fd.is_some(),
            "owned fd retained in quarantine"
        );

        // Reader does not observe EOF because writer was retained
        unsafe {
            let flags = libc::fcntl(r.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(r.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
        let mut buf = [0u8; 1];
        let res = nix::unistd::read(&r, &mut buf);
        assert!(res.is_err(), "reader must not see EOF while fd retained");
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Force unregister error fails closed retaining descriptor and registration state.
    #[test]
    fn force_unregister_error_fails_closed() {
        let mut owner = owner_for_tests();
        let desc = single_active_crtc();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (sock_owner, sock_peer) = socketpair(
            AddressFamily::Unix,
            SockType::Stream,
            None,
            SockFlag::empty(),
        )
        .expect("socketpair");

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b1,
                out_fences: vec![sock_owner],
            },
        };
        owner.apply_host_call_event(event);

        let turn = Cell::new(0);
        let mut query = MockQuery {
            f: |_fd: BorrowedFd<'_>| {
                if turn.get() == 0 {
                    Ok(FenceStatus::Pending)
                } else {
                    Ok(FenceStatus::Success)
                }
            },
        };
        let mut poll_set = TrackingPollSet::new();

        // Turn 1: registers successfully
        let events1 = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
        assert!(events1.is_empty());

        // Make readable and force unregister error
        nix::sys::socket::shutdown(sock_peer.as_raw_fd(), nix::sys::socket::Shutdown::Write)
            .expect("shutdown");
        turn.set(1);
        poll_set.fail_unregister = true;

        let events2 = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
        assert!(
            events2.iter().any(|e| matches!(
                e,
                OwnerEvent::MechanismFailed {
                    reason: MechanismFailure::FencePollError
                }
            )),
            "unregister failure must emit FencePollError"
        );

        let record = owner.live_record().unwrap();
        let evidence = record.fence_evidence().unwrap();
        assert!(evidence.slots[0].registered, "retains registration state");
        assert!(evidence.slots[0].fd.is_some(), "retains descriptor");
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Pending/Unknown retains writer until owner teardown.
    #[test]
    fn pending_unknown_retains_writer_until_owner_teardown() {
        let mut owner = owner_for_tests();
        let desc = single_active_crtc();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (r, w) = nix::unistd::pipe().expect("pipe");

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b1,
                out_fences: vec![w],
            },
        };
        owner.apply_host_call_event(event);

        let mut query = MockQuery {
            f: |_fd: BorrowedFd<'_>| Ok(FenceStatus::Pending),
        };
        let mut poll_set = TrackingPollSet::new();
        owner.observe_fences(&mut query, &mut poll_set, Instant::now());

        // Set nonblocking on r
        unsafe {
            let flags = libc::fcntl(r.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(r.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        let mut buf = [0u8; 1];
        let res1 = nix::unistd::read(&r, &mut buf);
        assert!(
            res1.is_err(),
            "while owner lives, writer is open, so read errors EWOULDBLOCK"
        );

        // Teardown owner
        drop(owner);

        let n = nix::unistd::read(&r, &mut buf).expect("read after owner teardown");
        assert_eq!(n, 0, "reader observes EOF after owner teardown");
    }

    /// [COMMIT-2, COMMIT-6, MULTI] Real CompletionPoller works via fence_poll_set_for_tests.
    #[test]
    fn real_completion_poller_works_with_immediate_success() {
        let mut owner = owner_for_tests();
        let desc = single_active_crtc();
        let (commit, _) = owner.begin(&desc, ledger()).unwrap();
        owner.mark_dispatched_for_tests();

        let (r, w) = nix::unistd::pipe().expect("pipe");

        let event = HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b1,
                out_fences: vec![w],
            },
        };
        owner.apply_host_call_event(event);

        let mut query = MockQuery {
            f: |_fd: BorrowedFd<'_>| Ok(FenceStatus::Success),
        };
        let mut poller = fence_poll_set_for_tests().expect("create poller");
        let events = owner.observe_fences(&mut query, &mut poller, Instant::now());

        assert!(
            events
                .iter()
                .any(|e| matches!(e, OwnerEvent::HardwareComplete { commit: c } if *c == commit)),
            "real CompletionPoller emits HardwareComplete on immediate success"
        );

        let mut buf = [0u8; 1];
        let n = nix::unistd::read(&r, &mut buf).expect("read");
        assert_eq!(n, 0, "writer closed, reader sees EOF");
    }
}
