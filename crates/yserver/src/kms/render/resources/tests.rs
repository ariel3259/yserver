use std::{cell::Cell, rc::Rc};

use super::*;
use crate::{kms::owner::identity::IncarnationId, platform::drm::DrmDeviceKey};

#[derive(Debug)]
pub(crate) struct SpyAllocation {
    pub(crate) drops: Rc<Cell<usize>>,
}

impl Drop for SpyAllocation {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

fn spy_service() -> (ResourceService, AllocationLease, Rc<Cell<usize>>) {
    let drops = Rc::new(Cell::new(0));
    let mut service = ResourceService::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        IncarnationId::first(),
    );
    let held = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops),
        }))
        .unwrap();
    (service, held, drops)
}

#[test]
fn c0_2ci_kms_release_does_not_complete_gpu_work() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let kms = service.register(key, ObligationKind::KmsRelease).unwrap();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();
    drop(held);
    service.apply_validated_proof(key, kms).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 0);
    assert!(matches!(
        service.reserve(key, UseKind::Write),
        Err(ResourceError::Busy)
    ));
    service.apply_validated_proof(key, gpu).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 1);
}

#[test]
fn c0_2ci_reverse_order_gpu_before_kms() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let kms = service.register(key, ObligationKind::KmsRelease).unwrap();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();
    drop(held);
    service.apply_validated_proof(key, gpu).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 0);
    assert!(matches!(
        service.reserve(key, UseKind::Write),
        Err(ResourceError::Busy)
    ));
    service.apply_validated_proof(key, kms).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 1);
}

#[test]
fn c0_2ci_two_retain_leases_require_both_drops_for_destruction() {
    let (mut service, held1, drops) = spy_service();
    let key = held1.key();
    let held2 = service.reserve(key, UseKind::Retain).unwrap();
    assert_eq!(held2.kind(), UseKind::Retain);
    assert_eq!(held2.key(), key);

    drop(held1);
    service.service_ready();
    assert_eq!(drops.get(), 0);

    drop(held2);
    service.service_ready();
    assert_eq!(drops.get(), 1);
}

#[test]
fn c0_2ci_duplicate_proof_rejected_and_no_double_cleanup() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();
    drop(held);

    assert_eq!(service.apply_validated_proof(key, gpu), Ok(()));
    assert_eq!(
        service.apply_validated_proof(key, gpu),
        Err(ResourceError::InvalidProof)
    );

    let ready = service.service_ready();
    assert_eq!(ready, vec![key]);
    assert_eq!(drops.get(), 1);

    // Repeated service_ready does not double-clean
    let ready2 = service.service_ready();
    assert!(ready2.is_empty());
    assert_eq!(drops.get(), 1);
}

#[test]
fn c0_2ci_stale_key_and_old_evidence_rejected() {
    let (mut service, held1, _drops1) = spy_service();
    let key1 = held1.key();
    let ob1 = service.register(key1, ObligationKind::Gpu).unwrap();

    let drops2 = Rc::new(Cell::new(0));
    let held2 = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops2),
        }))
        .unwrap();
    let key2 = held2.key();
    assert_ne!(key1.generation, key2.generation);

    // Old evidence delivered to new generation fails
    assert_eq!(
        service.apply_validated_proof(key2, ob1),
        Err(ResourceError::InvalidProof)
    );

    // Wrong device or incarnation is rejected
    let wrong_device_key = AllocationKey {
        device: DrmDeviceKey {
            major: 226,
            minor: 1,
        },
        incarnation: key1.incarnation,
        generation: key1.generation,
    };
    assert_eq!(
        service
            .reserve(wrong_device_key, UseKind::Read)
            .unwrap_err(),
        ResourceError::WrongIncarnation
    );
    assert_eq!(
        service.apply_validated_proof(wrong_device_key, ob1),
        Err(ResourceError::WrongIncarnation)
    );

    // Detached / unknown generation is rejected
    let unknown_gen_key = AllocationKey {
        device: key1.device,
        incarnation: key1.incarnation,
        generation: 9999,
    };
    assert_eq!(
        service.reserve(unknown_gen_key, UseKind::Read).unwrap_err(),
        ResourceError::Detached
    );

    drop(held1);
    drop(held2);
}

#[test]
fn c0_2ci_drop_write_lease_with_pending_gpu_blocks_reuse() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    drop(held);

    let write_lease = service.reserve(key, UseKind::Write).unwrap();
    assert_eq!(write_lease.kind(), UseKind::Write);
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    drop(write_lease);
    // Even though live write use dropped, pending GPU obligation blocks Write
    assert_eq!(
        service.reserve(key, UseKind::Write).unwrap_err(),
        ResourceError::Busy
    );

    // Resolving GPU unblocks Write
    service.apply_validated_proof(key, gpu).unwrap();
    let write_again = service.reserve(key, UseKind::Write);
    assert!(write_again.is_ok());
    drop(write_again);

    service.service_ready();
    assert_eq!(drops.get(), 1);
}

#[test]
fn c0_2ci_frozen_entries_remain_retained_after_normal_proofs() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let kms = service.register(key, ObligationKind::KmsRelease).unwrap();

    service.freeze(key).unwrap();
    assert_eq!(
        service.reserve(key, UseKind::Retain).unwrap_err(),
        ResourceError::Frozen
    );
    assert_eq!(
        service.reserve(key, UseKind::Write).unwrap_err(),
        ResourceError::Frozen
    );
    assert_eq!(
        service.register(key, ObligationKind::Gpu).unwrap_err(),
        ResourceError::Frozen
    );

    drop(held);
    service.apply_validated_proof(key, kms).unwrap();
    service.service_ready();
    // Frozen entry is NOT destroyed even after all uses and obligations are 0
    assert_eq!(drops.get(), 0);
}

#[test]
fn c0_2ci_cancellation_clears_obligation_without_discharge() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let kms = service.register(key, ObligationKind::KmsRelease).unwrap();
    drop(held);

    // Cancel without hardware proof
    service.cancel(key, kms).unwrap();
    // Repeating cancellation fails with InvalidProof
    assert_eq!(service.cancel(key, kms), Err(ResourceError::InvalidProof));

    service.service_ready();
    assert_eq!(drops.get(), 1);
}

#[test]
fn c0_2ci_alias_multiple_readers_and_kms_concurrency() {
    let (mut service, held, _drops) = spy_service();
    let key = held.key();

    let read1 = service.reserve(key, UseKind::Read).unwrap();
    let read2 = service.reserve(key, UseKind::Read).unwrap();
    let kms = service.reserve(key, UseKind::Kms).unwrap();

    assert_eq!(read1.kind(), UseKind::Read);
    assert_eq!(read2.kind(), UseKind::Read);
    assert_eq!(kms.kind(), UseKind::Kms);

    // Write is excluded by active readers and KMS
    assert_eq!(
        service.reserve(key, UseKind::Write).unwrap_err(),
        ResourceError::Busy
    );

    drop(read1);
    drop(read2);
    // KMS still held, Write still blocked
    assert_eq!(
        service.reserve(key, UseKind::Write).unwrap_err(),
        ResourceError::Busy
    );

    drop(kms);
    drop(held);
    // Now Write succeeds
    let write = service.reserve(key, UseKind::Write).unwrap();
    // And while Write is held, Read and KMS are blocked
    assert_eq!(
        service.reserve(key, UseKind::Read).unwrap_err(),
        ResourceError::Busy
    );
    assert_eq!(
        service.reserve(key, UseKind::Kms).unwrap_err(),
        ResourceError::Busy
    );
    drop(write);
}
