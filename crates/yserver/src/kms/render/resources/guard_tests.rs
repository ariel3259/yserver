//! Stage 2c-i debt, session 1: one test per refusal guard the census found
//! unproven. Each guard's assertion carries `[census:<MARKER>]` and the test a
//! matching `/// census:` tag bound to the test, so `tools/guard-census.py --require-oracle`
//! can confirm the guard is killed by its own assertion. Spec:
//! docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md.

use std::{cell::Cell, num::NonZeroU32, rc::Rc};

use super::{
    tests::{SpyAllocation, spy_service},
    *,
};
// `CommitId` and `CrtcKey` are not re-exported by `resources`; `tests.rs`
// imports them explicitly too.
use crate::{
    kms::{
        owner::identity::{CommitId, IncarnationId},
        render::platform::CrtcKey,
    },
    platform::drm::DrmDeviceKey,
};

fn wrong_device(key: AllocationKey) -> AllocationKey {
    AllocationKey {
        device: DrmDeviceKey {
            major: key.device.major,
            minor: key.device.minor + 1,
        },
        ..key
    }
}

fn wrong_incarnation(key: AllocationKey) -> AllocationKey {
    AllocationKey {
        incarnation: key.incarnation.next(),
        ..key
    }
}

fn spy(service: &mut ResourceService) -> AllocationLease {
    service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::new(Cell::new(0)),
        }))
        .unwrap()
}

fn member() -> GroupMember {
    let crtc = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(NonZeroU32::new(10).unwrap()),
    );
    GroupMember::new(crtc, 1, 1)
}

/// A second service on another incarnation, holding one spy allocation with a
/// pending read obligation: the only way to obtain a lease whose key is
/// foreign to the first service, since `reserve`/`adopt` enforce identity.
fn foreign_read_lease(
    device: DrmDeviceKey,
    incarnation: IncarnationId,
) -> (ResourceService, AllocationLease, ObligationId) {
    let mut other = ResourceService::new(device, incarnation);
    let lease = spy(&mut other);
    let ob = other.register(lease.key(), ObligationKind::Read).unwrap();
    (other, lease, ob)
}

/// census: E-register mod.rs register `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_register_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.register(foreign, ObligationKind::Gpu),
            Err(ResourceError::WrongIncarnation),
            "register must refuse a foreign key [census:E-register]"
        );
    }
}

/// census: E-freeze mod.rs freeze `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_freeze_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.freeze(foreign),
            Err(ResourceError::WrongIncarnation),
            "freeze must refuse a foreign key [census:E-freeze]"
        );
    }
}

/// census: E-cancel mod.rs cancel `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_cancel_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    let ob = service.register(held.key(), ObligationKind::Gpu).unwrap();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.cancel(foreign, ob),
            Err(ResourceError::WrongIncarnation),
            "cancel must refuse a foreign key [census:E-cancel]"
        );
    }
}

/// census: E-validate-proof-target mod.rs validate_proof_target `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_validate_proof_target_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    let ob = service.register(held.key(), ObligationKind::Gpu).unwrap();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.validate_proof_target(foreign, ob),
            Err(ResourceError::WrongIncarnation),
            "validate_proof_target must refuse a foreign key [census:E-validate-proof-target]"
        );
    }
}

/// census: E-record-kms-discharged mod.rs record_kms_discharged `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_record_kms_discharged_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    let commit = CommitId::for_tests(910);
    let ob = service.register_kms(held.key(), commit, member()).unwrap();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        assert_eq!(
            service.record_kms_discharged(foreign, ob, commit, member()),
            Err(ResourceError::WrongIncarnation),
            "record_kms_discharged must refuse a foreign key [census:E-record-kms-discharged]"
        );
    }
}

/// census: E-teardown-proof mod.rs apply_teardown_release `proof.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_teardown_release_refuses_foreign_proof() {
    let (mut service, held, _drops) = spy_service();
    let supervisor = RetainingSupervisor::new();
    // Deleting the guard lets the valid key reach the frozen check, which
    // returns InvalidState, not WrongIncarnation.
    let proof = supervisor.issue_teardown_release(IncarnationId::first().next(), vec![held.key()]);
    assert_eq!(
        service.apply_teardown_release(proof),
        Err(ResourceError::WrongIncarnation),
        "teardown release must refuse a proof for another incarnation [census:E-teardown-proof]"
    );
}

/// census: E-teardown-key mod.rs apply_teardown_release `key.device != self.device || key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_teardown_release_refuses_foreign_key() {
    let (mut service, held, _drops) = spy_service();
    let supervisor = RetainingSupervisor::new();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        let proof = supervisor.issue_teardown_release(IncarnationId::first(), vec![foreign]);
        assert_eq!(
            service.apply_teardown_release(proof),
            Err(ResourceError::WrongIncarnation),
            "teardown release must refuse a foreign key [census:E-teardown-key]"
        );
    }
}

/// census: E-batch-entry mod.rs validate_gpu_batch `key.device != self.device || key.incarnation != self.incarnation` #1
#[test]
fn c0_2ci_guard_gpu_batch_refuses_foreign_entry() {
    let (mut service, held, _drops) = spy_service();
    let ob = service.register(held.key(), ObligationKind::Gpu).unwrap();
    for foreign in [wrong_device(held.key()), wrong_incarnation(held.key())] {
        let mut batch = CoreRetirementBatch::new(Vec::new(), Vec::new(), true);
        batch.bind_ticket(GpuObligation::for_tests_stub(
            vec![(foreign, ob)],
            crate::kms::render::platform::FenceTicket::for_tests_stub(),
        ));
        assert_eq!(
            service.validate_gpu_batch(batch).err().map(|(e, _)| e),
            Some(ResourceError::WrongIncarnation),
            "a GPU batch entry with a foreign key must be refused [census:E-batch-entry]"
        );
    }
}

/// census: E-batch-read-source mod.rs validate_gpu_batch `key.device != self.device || key.incarnation != self.incarnation` #2
#[test]
fn c0_2ci_guard_gpu_batch_refuses_foreign_read_source() {
    let (service, held, _drops) = spy_service();
    let key = held.key();
    let foreign_ids = [
        (wrong_device(key).device, key.incarnation),
        (key.device, key.incarnation.next()),
    ];
    for (device, incarnation) in foreign_ids {
        let (_other, lease, ob) = foreign_read_lease(device, incarnation);
        let mut batch = CoreRetirementBatch::new(Vec::new(), Vec::new(), true);
        batch.bind_read_obligation(super::gpu::ReadObligation::new(lease, ob, None, None));
        assert_eq!(
            service.validate_gpu_batch(batch).err().map(|(e, _)| e),
            Some(ResourceError::WrongIncarnation),
            "a read obligation whose source is foreign must be refused [census:E-batch-read-source]"
        );
    }
}

/// census: E-batch-read-staging mod.rs validate_gpu_batch `s_key.device != self.device || s_key.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_gpu_batch_refuses_foreign_read_staging() {
    let (mut service, held, _drops) = spy_service();
    let key = held.key();
    let source_ob = service.register(key, ObligationKind::Read).unwrap();
    let (_other, staging, staging_ob) = foreign_read_lease(key.device, key.incarnation.next());
    let mut batch = CoreRetirementBatch::new(Vec::new(), Vec::new(), true);
    // The spy's own lease is the source: a fresh Read reservation beside its
    // Retain use is not something this test should depend on.
    batch.bind_read_obligation(super::gpu::ReadObligation::new(
        held,
        source_ob,
        Some(staging),
        Some(staging_ob),
    ));
    assert_eq!(
        service.validate_gpu_batch(batch).err().map(|(e, _)| e),
        Some(ResourceError::WrongIncarnation),
        "a read obligation whose staging lease is foreign must be refused [census:E-batch-read-staging]"
    );
}
