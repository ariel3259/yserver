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

fn read_batch(
    source: AllocationLease,
    source_ob: ObligationId,
    staging: Option<(AllocationLease, ObligationId)>,
) -> CoreRetirementBatch {
    let mut batch = CoreRetirementBatch::new(Vec::new(), Vec::new(), true);
    let (staging_lease, staging_ob) = match staging {
        Some((lease, ob)) => (Some(lease), Some(ob)),
        None => (None, None),
    };
    batch.bind_read_obligation(super::gpu::ReadObligation::new(
        source,
        source_ob,
        staging_lease,
        staging_ob,
    ));
    batch
}

/// census: F-read-source-frozen mod.rs validate_gpu_batch `avail.frozen` #2
#[test]
fn c0_2ci_guard_gpu_batch_refuses_frozen_read_source() {
    let (mut service, held, _drops) = spy_service();
    let key = held.key();
    let ob = service.register(key, ObligationKind::Read).unwrap();
    service.freeze(key).unwrap();
    assert_eq!(
        service
            .validate_gpu_batch(read_batch(held, ob, None))
            .err()
            .map(|(e, _)| e),
        Some(ResourceError::Frozen),
        "a read obligation on a frozen source must be refused [census:F-read-source-frozen]"
    );
}

/// census: F-read-source-pending mod.rs validate_gpu_batch `!avail.pending_obligations.contains_key(&obligation_id)` #2
#[test]
fn c0_2ci_guard_gpu_batch_refuses_read_source_without_pending_obligation() {
    let (mut service, held, _drops) = spy_service();
    let key = held.key();
    let ob = service.register(key, ObligationKind::Read).unwrap();
    service.cancel(key, ob).unwrap();
    assert_eq!(
        service
            .validate_gpu_batch(read_batch(held, ob, None))
            .err()
            .map(|(e, _)| e),
        Some(ResourceError::InvalidProof),
        "a read obligation that is no longer pending must be refused [census:F-read-source-pending]"
    );
}

/// census: F-read-staging-frozen mod.rs validate_gpu_batch `s_avail.frozen`
#[test]
fn c0_2ci_guard_gpu_batch_refuses_frozen_read_staging() {
    let (mut service, held, _drops) = spy_service();
    let source_ob = service.register(held.key(), ObligationKind::Read).unwrap();
    let staging = spy(&mut service);
    let staging_key = staging.key();
    let staging_ob = service.register(staging_key, ObligationKind::Read).unwrap();
    service.freeze(staging_key).unwrap();
    assert_eq!(
        service
            .validate_gpu_batch(read_batch(held, source_ob, Some((staging, staging_ob))))
            .err()
            .map(|(e, _)| e),
        Some(ResourceError::Frozen),
        "a read obligation with a frozen staging lease must be refused [census:F-read-staging-frozen]"
    );
}

/// census: F-read-staging-pending mod.rs validate_gpu_batch `!s_avail.pending_obligations.contains_key(&staging_ob)`
#[test]
fn c0_2ci_guard_gpu_batch_refuses_read_staging_without_pending_obligation() {
    let (mut service, held, _drops) = spy_service();
    let source_ob = service.register(held.key(), ObligationKind::Read).unwrap();
    let staging = spy(&mut service);
    let staging_key = staging.key();
    let staging_ob = service.register(staging_key, ObligationKind::Read).unwrap();
    service.cancel(staging_key, staging_ob).unwrap();
    assert_eq!(
        service
            .validate_gpu_batch(read_batch(held, source_ob, Some((staging, staging_ob))))
            .err()
            .map(|(e, _)| e),
        Some(ResourceError::InvalidProof),
        "a staging obligation that is no longer pending must be refused [census:F-read-staging-pending]"
    );
}

/// census: G-adopt-exhausted mod.rs adopt_unchecked `self.exhausted`
#[test]
fn c0_2ci_guard_adopt_refuses_when_exhausted() {
    let (mut service, _held, _drops) = spy_service();
    service.force_exhausted_for_tests();
    let result = service.adopt(AllocationPayload::Spy(SpyAllocation {
        drops: Rc::new(Cell::new(0)),
    }));
    assert!(
        matches!(result, Err((ResourceError::Exhausted, _))),
        "an exhausted service must refuse adoption [census:G-adopt-exhausted]"
    );
}

/// census: G-reserve-exhausted mod.rs reserve `self.exhausted`
#[test]
fn c0_2ci_guard_reserve_refuses_when_exhausted() {
    let (mut service, held, _drops) = spy_service();
    service.force_exhausted_for_tests();
    assert_eq!(
        service.reserve(held.key(), UseKind::Read).err(),
        Some(ResourceError::Exhausted),
        "an exhausted service must refuse a reservation [census:G-reserve-exhausted]"
    );
}

/// census: G-register-exhausted mod.rs register `self.exhausted`
#[test]
fn c0_2ci_guard_register_refuses_when_exhausted() {
    let (mut service, held, _drops) = spy_service();
    service.force_exhausted_for_tests();
    assert_eq!(
        service.register(held.key(), ObligationKind::Gpu),
        Err(ResourceError::Exhausted),
        "an exhausted service must refuse a new obligation [census:G-register-exhausted]"
    );
}

/// census: H-adopt-file-owned mod.rs adopt `payload.file_owned_alias_present()`
#[test]
fn c0_2ci_guard_adopt_refuses_live_file_owned_payload() {
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let right = DrmCleanupRight::new(device_key, IncarnationId::first(), 70, 71, GemOwner::Right);
    let file_owned = FileOwnedBacking::new(right, None, device).unwrap();
    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let payload = AllocationPayload::Scanout(ScanoutAllocation::new(Some(file_owned), shared));
    let mut service = ResourceService::new(device_key, IncarnationId::first());
    assert!(
        matches!(
            service.adopt(payload),
            Err((ResourceError::InvalidState, _))
        ),
        "adopt must refuse a payload with a live file-owned alias [census:H-adopt-file-owned]"
    );
}

/// census: I-teardown-requires-frozen mod.rs apply_teardown_release `!avail.frozen`
#[test]
fn c0_2ci_guard_teardown_release_refuses_unfrozen_entry() {
    let (mut service, held, _drops) = spy_service();
    let supervisor = RetainingSupervisor::new();
    let proof = supervisor.issue_teardown_release(IncarnationId::first(), vec![held.key()]);
    assert_eq!(
        service.apply_teardown_release(proof),
        Err(ResourceError::InvalidState),
        "teardown release must refuse an entry that is not frozen [census:I-teardown-requires-frozen]"
    );
}
