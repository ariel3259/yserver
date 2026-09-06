//! Descriptions shared by unit tests, the integration test and the backend
//! fixtures.
//!
//! **Not `#[cfg(test)]`.** An integration-test crate links the library built
//! *without* `cfg(test)`, so a `#[cfg(test)]` fixture is invisible to it and
//! a crate-private one is inaccessible. `#[doc(hidden)] pub` is the same seam
//! stage 2a used for `executor::test_support`, and Task 7's grep bounds it.

use crate::kms::{
    executor::{
        HostCallClass, HostCallRequest, KmsIoExecutor, ReapState,
        protocol::{HostCallCorrelation, RequestSeq},
        test_support::StubBehaviour,
    },
    owner::{
        build::{CommitDescription, build_atomic_request},
        closure::{CrtcPower, ObjectKind, PropertyIds, SerializedObject},
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
/// `EventToken::tagged_for_tests(n)`. **Tagged, never `for_tests`:** both
/// token decoders check the purpose tag, so an untagged token is rejected on
/// arrival and the helper answers with a protocol error instead of a reply.
#[doc(hidden)]
pub fn atomic_correlation_for_tests(n: u64) -> HostCallCorrelation {
    HostCallCorrelation::Atomic {
        seq: RequestSeq::from_raw(n),
        incarnation: IncarnationId::from_raw(1),
        lifecycle_epoch: LifecycleEpochId::from_raw(1),
        transition: None,
        commit: CommitId::for_tests(n),
        event_token: EventToken::tagged_for_tests(n),
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
