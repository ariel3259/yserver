//! Descriptions shared by unit tests, the integration test and the backend
//! fixtures.
//!
//! **Not `#[cfg(test)]`.** An integration-test crate links the library built
//! *without* `cfg(test)`, so a `#[cfg(test)]` fixture is invisible to it and
//! a crate-private one is inaccessible. `#[doc(hidden)] pub` is the same seam
//! stage 2a used for `executor::test_support`, and Task 7's grep bounds it.

use std::collections::{BTreeMap, BTreeSet};

use crate::kms::{
    executor::{
        HostCallClass, HostCallRequest, KmsIoExecutor, ReapState,
        protocol::{HostCallCorrelation, RequestSeq},
        test_support::StubBehaviour,
    },
    owner::{
        build::{CommitDescription, build_atomic_request},
        clock::ClockKey,
        closure::{CrtcPower, ObjectKind, PropertyIds, SerializedObject},
        completion::{CompletionClass, CompletionContext},
        identity::{CommitId, EventToken, IncarnationId},
        lifecycle::LifecycleEpochId,
    },
};

#[doc(hidden)]
pub const TEST_PROPERTY_IDS: PropertyIds = PropertyIds {
    crtc_id: 20,
    active: 21,
    out_fence_ptr: 22,
};

#[derive(Debug, PartialEq)]
#[doc(hidden)]
pub enum TestResource {
    OldFramebuffer(u32),
    NewFramebuffer(u32),
}

#[doc(hidden)]
pub fn ledger() -> super::ledger::Submitted<TestResource> {
    super::ledger::Submitted::new(
        vec![TestResource::OldFramebuffer(66)],
        vec![TestResource::NewFramebuffer(77)],
    )
}

#[doc(hidden)]
pub fn owner_for_tests() -> super::device::DeviceCommitOwner<TestResource> {
    super::device::DeviceCommitOwner::new(IncarnationId::first(), LifecycleEpochId::first(), 1)
}

#[doc(hidden)]
pub fn never_owner_for_tests() -> super::device::DeviceCommitOwner<super::NeverResource> {
    super::device::DeviceCommitOwner::new(IncarnationId::first(), LifecycleEpochId::first(), 1)
}

#[doc(hidden)]
pub fn never_ledger() -> super::ledger::Submitted<super::NeverResource> {
    super::ledger::Submitted::new(Vec::new(), Vec::new())
}

#[doc(hidden)]
pub fn off_to_off_crtc(id: u32) -> SerializedObject {
    SerializedObject {
        object: id,
        kind: ObjectKind::Crtc,
        old_crtc_id: None,
        props: vec![(TEST_PROPERTY_IDS.active, 0)],
    }
}

#[doc(hidden)]
pub fn owner_correlation(commit: CommitId) -> HostCallCorrelation {
    let mut c = atomic_correlation_for_tests(commit.get());
    if let HostCallCorrelation::Atomic { event_token, .. } = &mut c {
        *event_token = EventToken::for_tests(commit.get());
    }
    c
}

#[doc(hidden)]
pub fn accepted(commit: CommitId, mask: u32, count: usize) -> crate::kms::executor::HostCallEvent {
    crate::kms::executor::HostCallEvent::Outcome {
        correlation: owner_correlation(commit),
        outcome: crate::kms::executor::HostCallOutcome::Accepted {
            helper_duration_ns: 0,
            round_trip_ns: 0,
            out_fence_mask: mask,
            out_fences: (0..count)
                .map(|_| std::fs::File::open("/dev/null").expect("fd").into())
                .collect(),
        },
    }
}

#[doc(hidden)]
pub fn late_accepted(
    commit: CommitId,
    mask: u32,
    count: usize,
) -> crate::kms::executor::HostCallEvent {
    let crate::kms::executor::HostCallEvent::Outcome {
        correlation,
        outcome,
    } = accepted(commit, mask, count)
    else {
        unreachable!()
    };
    crate::kms::executor::HostCallEvent::LateReply {
        correlation,
        outcome,
    }
}

#[doc(hidden)]
pub fn rejected(commit: CommitId, errno: i32) -> crate::kms::executor::HostCallEvent {
    crate::kms::executor::HostCallEvent::Outcome {
        correlation: owner_correlation(commit),
        outcome: crate::kms::executor::HostCallOutcome::Rejected {
            errno,
            helper_duration_ns: 0,
            round_trip_ns: 0,
            unexpected_fence_output: false,
        },
    }
}

#[doc(hidden)]
pub fn unknown(
    commit: CommitId,
    reason: crate::kms::executor::UnknownReason,
) -> crate::kms::executor::HostCallEvent {
    crate::kms::executor::HostCallEvent::Outcome {
        correlation: owner_correlation(commit),
        outcome: crate::kms::executor::HostCallOutcome::Unknown(reason),
    }
}

#[doc(hidden)]
pub fn validation_abandoned(
    commit: CommitId,
    reason: crate::kms::executor::UnknownReason,
) -> crate::kms::executor::HostCallEvent {
    crate::kms::executor::HostCallEvent::Outcome {
        correlation: owner_correlation(commit),
        outcome: crate::kms::executor::HostCallOutcome::ValidationAbandoned(reason),
    }
}

#[doc(hidden)]
pub fn probe_accepted_event(sequence: u64) -> crate::kms::executor::HostCallEvent {
    crate::kms::executor::HostCallEvent::Outcome {
        correlation: HostCallCorrelation::ClockProbe {
            seq: RequestSeq::from_raw(1),
            incarnation: IncarnationId::first(),
            lifecycle_epoch: LifecycleEpochId::first(),
            topology_generation: 1,
            hardware_crtc: 1,
            clock_epoch: super::identity::ClockEpochId::first(),
            probe: super::lifecycle::ClockProbeId::first(),
        },
        outcome: crate::kms::executor::HostCallOutcome::ProbeAccepted {
            sequence,
            helper_duration_ns: 0,
            round_trip_ns: 0,
        },
    }
}

/// CRTC 1 active before and after, with a plane bound to it.
#[doc(hidden)]
pub fn single_active_crtc() -> CommitDescription {
    CommitDescription {
        objects: vec![
            SerializedObject {
                object: 1,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(TEST_PROPERTY_IDS.active, 1)],
            },
            SerializedObject {
                object: 10,
                kind: ObjectKind::Plane,
                old_crtc_id: Some(1),
                props: vec![(TEST_PROPERTY_IDS.crtc_id, 1)],
            },
        ],
        crtc_state: vec![CrtcPower {
            crtc_id: 1,
            old_active: true,
            new_active: true,
        }],
        present_consumers: Vec::new(),
        page_flip_event: false,
        property_ids: TEST_PROPERTY_IDS,
    }
}

/// CRTC 1 active, CRTC 2 inactive before and after.
#[doc(hidden)]
pub fn two_crtcs_one_off() -> CommitDescription {
    CommitDescription {
        objects: vec![
            SerializedObject {
                object: 1,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(TEST_PROPERTY_IDS.active, 1)],
            },
            SerializedObject {
                object: 2,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(TEST_PROPERTY_IDS.active, 0)],
            },
            SerializedObject {
                object: 10,
                kind: ObjectKind::Plane,
                old_crtc_id: Some(1),
                props: vec![(TEST_PROPERTY_IDS.crtc_id, 1)],
            },
        ],
        crtc_state: vec![
            CrtcPower {
                crtc_id: 1,
                old_active: true,
                new_active: true,
            },
            CrtcPower {
                crtc_id: 2,
                old_active: false,
                new_active: false,
            },
        ],
        present_consumers: Vec::new(),
        page_flip_event: false,
        property_ids: TEST_PROPERTY_IDS,
    }
}

/// CRTCs 1 and 2 both active before and after, so
/// `expected_completion == [1, 2]` and a request over it needs two
/// out-fences. Used by the short-mask test, which needs a request that
/// expects more fences than the reply returns.
#[doc(hidden)]
pub fn two_active_crtcs() -> CommitDescription {
    CommitDescription {
        objects: vec![
            SerializedObject {
                object: 1,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(TEST_PROPERTY_IDS.active, 1)],
            },
            SerializedObject {
                object: 2,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(TEST_PROPERTY_IDS.active, 1)],
            },
            SerializedObject {
                object: 10,
                kind: ObjectKind::Plane,
                old_crtc_id: Some(1),
                props: vec![(TEST_PROPERTY_IDS.crtc_id, 1)],
            },
            SerializedObject {
                object: 11,
                kind: ObjectKind::Plane,
                old_crtc_id: Some(2),
                props: vec![(TEST_PROPERTY_IDS.crtc_id, 2)],
            },
        ],
        crtc_state: vec![
            CrtcPower {
                crtc_id: 1,
                old_active: true,
                new_active: true,
            },
            CrtcPower {
                crtc_id: 2,
                old_active: true,
                new_active: true,
            },
        ],
        present_consumers: Vec::new(),
        page_flip_event: false,
        property_ids: TEST_PROPERTY_IDS,
    }
}

/// A built `HostCallRequest` for record-level tests that only need something
/// to attach and take back. It is never sent, so its contents are
/// irrelevant beyond being well-formed.
#[doc(hidden)]
pub fn request_for_tests() -> HostCallRequest {
    let (request, _closure) = build_atomic_request(
        &single_active_crtc(),
        atomic_correlation_for_tests(1),
        HostCallClass::SeatActiveNonblock,
    )
    .expect("the fixture description builds");
    HostCallRequest::Atomic(request)
}

/// A `HostCallCorrelation::Atomic` over `CommitId::for_tests(n)` and
/// `EventToken::for_tests(n)`.
#[doc(hidden)]
pub fn atomic_correlation_for_tests(n: u64) -> HostCallCorrelation {
    HostCallCorrelation::Atomic {
        seq: RequestSeq::from_raw(n),
        incarnation: IncarnationId::from_raw(1),
        lifecycle_epoch: LifecycleEpochId::from_raw(1),
        transition: None,
        commit: CommitId::for_tests(n),
        event_token: EventToken::for_tests(n),
    }
}

/// An executor whose child has already exited and been reaped, so `send`
/// returns `SendError::Reaped` before installing `InFlight` — the pre-IPC
/// refusal the `NeverDispatched` test needs.
///
/// Built from 2a's stub: spawn `ExitBeforeReply`, then drive `try_reap` until
/// it reports `Reaped`, bounded, because reaping is asynchronous and asserting
/// it at an instant is the race that cost stage 2a two defects.
#[doc(hidden)]
pub fn reaped_executor_for_tests() -> KmsIoExecutor {
    let mut executor =
        crate::kms::executor::test_support::spawn_stub_helper(StubBehaviour::ExitBeforeReply)
            .expect("spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if matches!(executor.try_reap(), ReapState::Reaped(_)) {
            return executor;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("the stub helper did not become reapable within 5s");
}

#[doc(hidden)]
pub fn legacy_owner_for_tests() -> super::device::DeviceCommitOwner<TestResource> {
    super::device::DeviceCommitOwner::new_legacy(
        IncarnationId::first(),
        LifecycleEpochId::first(),
        1,
    )
}

#[doc(hidden)]
pub fn single_active_crtc_with_present(consumer: u32) -> CommitDescription {
    CommitDescription {
        objects: vec![
            SerializedObject {
                object: 1,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(TEST_PROPERTY_IDS.active, 1)],
            },
            SerializedObject {
                object: 10,
                kind: ObjectKind::Plane,
                old_crtc_id: Some(1),
                props: vec![(TEST_PROPERTY_IDS.crtc_id, 1)],
            },
        ],
        crtc_state: vec![CrtcPower {
            crtc_id: 1,
            old_active: true,
            new_active: true,
        }],
        present_consumers: vec![consumer],
        page_flip_event: true,
        property_ids: TEST_PROPERTY_IDS,
    }
}

#[doc(hidden)]
pub fn two_active_crtcs_with_present(c1: u32, c2: u32) -> CommitDescription {
    CommitDescription {
        objects: vec![
            SerializedObject {
                object: 1,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(TEST_PROPERTY_IDS.active, 1)],
            },
            SerializedObject {
                object: 2,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(TEST_PROPERTY_IDS.active, 1)],
            },
            SerializedObject {
                object: 10,
                kind: ObjectKind::Plane,
                old_crtc_id: Some(1),
                props: vec![(TEST_PROPERTY_IDS.crtc_id, 1)],
            },
            SerializedObject {
                object: 11,
                kind: ObjectKind::Plane,
                old_crtc_id: Some(2),
                props: vec![(TEST_PROPERTY_IDS.crtc_id, 2)],
            },
        ],
        crtc_state: vec![
            CrtcPower {
                crtc_id: 1,
                old_active: true,
                new_active: true,
            },
            CrtcPower {
                crtc_id: 2,
                old_active: true,
                new_active: true,
            },
        ],
        present_consumers: vec![c1, c2],
        page_flip_event: true,
        property_ids: TEST_PROPERTY_IDS,
    }
}

#[doc(hidden)]
pub fn single_active_crtc_non_consumer_page_flip() -> CommitDescription {
    CommitDescription {
        objects: vec![
            SerializedObject {
                object: 1,
                kind: ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(TEST_PROPERTY_IDS.active, 1)],
            },
            SerializedObject {
                object: 10,
                kind: ObjectKind::Plane,
                old_crtc_id: Some(1),
                props: vec![(TEST_PROPERTY_IDS.crtc_id, 1)],
            },
        ],
        crtc_state: vec![CrtcPower {
            crtc_id: 1,
            old_active: true,
            new_active: true,
        }],
        present_consumers: Vec::new(),
        page_flip_event: true,
        property_ids: TEST_PROPERTY_IDS,
    }
}

#[doc(hidden)]
pub fn fast_context_for_crtcs(keys: &[(u32, ClockKey)]) -> CompletionContext {
    let mut clocks = BTreeMap::new();
    let mut mode_periods = BTreeMap::new();
    for &(crtc, key) in keys {
        clocks.insert(crtc, key);
        mode_periods.insert(crtc, None);
    }
    CompletionContext {
        class: CompletionClass::FastUpdate,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: None,
    }
}

#[doc(hidden)]
pub fn page_event_bytes(
    crtc: u32,
    raw_sequence: u32,
    sec: u32,
    usec: u32,
    user_data: u64,
) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[0..4].copy_from_slice(&2u32.to_ne_bytes()); // DRM_EVENT_FLIP_COMPLETE
    bytes[4..8].copy_from_slice(&32u32.to_ne_bytes());
    bytes[8..16].copy_from_slice(&user_data.to_ne_bytes());
    bytes[16..20].copy_from_slice(&sec.to_ne_bytes());
    bytes[20..24].copy_from_slice(&usec.to_ne_bytes());
    bytes[24..28].copy_from_slice(&raw_sequence.to_ne_bytes());
    bytes[28..32].copy_from_slice(&crtc.to_ne_bytes());
    bytes
}

#[doc(hidden)]
pub fn event_for_current_record<R>(
    owner: &super::device::DeviceCommitOwner<R>,
    crtc: u32,
    raw_sequence: u32,
    sec: u32,
    usec: u32,
) -> crate::drm::event_stream::DrmEventRecord {
    let token = owner.live_record().expect("live record").event_token();
    let bytes = page_event_bytes(crtc, raw_sequence, sec, usec, token.as_user_data());
    let (mut records, err) = crate::drm::event_stream::parse_event_buffer_partial(&bytes);
    assert!(err.is_none(), "page_event_bytes must parse cleanly");
    assert_eq!(records.len(), 1);
    records.pop().expect("record")
}

#[doc(hidden)]
pub fn fence_poll_set_for_tests()
-> std::io::Result<impl super::fences::FencePollSet + std::os::fd::AsRawFd> {
    crate::kms::render::completion_poller::CompletionPoller::new()
}

#[doc(hidden)]
pub fn completion_caps_for_tests(
    incarnation: IncarnationId,
    generation: u64,
    atomic: bool,
    crtc_cap: bool,
    monotonic_cap: bool,
    crtcs: BTreeSet<u32>,
) -> super::qualification::CompletionCaps {
    super::qualification::CompletionCaps::new_for_tests(
        incarnation,
        generation,
        atomic,
        crtc_cap,
        monotonic_cap,
        crtcs,
    )
}

#[doc(hidden)]
pub fn install_test_completion_caps<R>(
    owner: &mut super::device::DeviceCommitOwner<R>,
    caps: super::qualification::CompletionCaps,
) -> Result<(), super::device::DispatchError<R>> {
    owner.install_completion_caps(caps)
}

#[doc(hidden)]
pub fn lifecycle_context_for_crtcs(
    keys: &[(u32, ClockKey)],
    observed_max: Option<std::time::Duration>,
) -> CompletionContext {
    let mut clocks = BTreeMap::new();
    let mut mode_periods = BTreeMap::new();
    for &(crtc, key) in keys {
        clocks.insert(crtc, key);
        mode_periods.insert(crtc, None);
    }
    CompletionContext {
        class: CompletionClass::LifecycleInstallRestore,
        host_class: HostCallClass::SeatActiveNonblock,
        allow_modeset: false,
        clocks,
        mode_periods,
        lifecycle_observed_max: observed_max,
    }
}
