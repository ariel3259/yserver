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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CleanupCall {
    RemoveFb(u32),
    CloseGem(u32),
}

#[derive(Debug, Clone)]
struct MockCleanupIo {
    calls: Rc<RefCell<Vec<CleanupCall>>>,
    fail_fb: Rc<Cell<bool>>,
    fail_gem: Rc<Cell<bool>>,
}

impl MockCleanupIo {
    fn new(calls: Rc<RefCell<Vec<CleanupCall>>>) -> Self {
        Self {
            calls,
            fail_fb: Rc::new(Cell::new(false)),
            fail_gem: Rc::new(Cell::new(false)),
        }
    }
}

impl CleanupIo for MockCleanupIo {
    fn remove_fb(&mut self, fb: u32) -> std::io::Result<()> {
        self.calls.borrow_mut().push(CleanupCall::RemoveFb(fb));
        if self.fail_fb.get() {
            return Err(std::io::Error::other("simulated remove_fb failure"));
        }
        Ok(())
    }

    fn close_gem(&mut self, gem: u32) -> std::io::Result<()> {
        self.calls.borrow_mut().push(CleanupCall::CloseGem(gem));
        if self.fail_gem.get() {
            return Err(std::io::Error::other("simulated close_gem failure"));
        }
        Ok(())
    }
}

#[test]
fn c0_2ci_drm_cleanup_counting_transport() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut registry = DrmCleanupRegistry::new_with_io(device_key, incarnation, Box::new(io));
    let right = registry.register_right(11, 12, GemOwner::Right);
    registry.consume(right).unwrap();

    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(11), CleanupCall::CloseGem(12)]
    );
}

#[test]
fn c0_2ci_drm_cleanup_partial_failure_and_retry() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    io.fail_gem.set(true);

    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut registry =
        DrmCleanupRegistry::new_with_io(device_key, incarnation, Box::new(io.clone()));
    let right = registry.register_right(11, 12, GemOwner::Right);

    // Initial consume fails at close_gem
    let (err, returned_right) = registry.consume(right).unwrap_err();
    assert_eq!(err.to_string(), "simulated close_gem failure");
    assert_eq!(returned_right.state(), RightState::FramebufferRemoved);
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(11), CleanupCall::CloseGem(12)]
    );

    // Clear simulated failure and retry
    io.fail_gem.set(false);
    registry.consume(returned_right).unwrap();

    // RMFB was NOT re-issued on retry; only GEM close was retried!
    assert_eq!(
        calls.borrow().as_slice(),
        &[
            CleanupCall::RemoveFb(11),
            CleanupCall::CloseGem(12),
            CleanupCall::CloseGem(12)
        ]
    );
}

#[test]
fn c0_2ci_drm_cleanup_gem_owner_gbm_never_closes_gem() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut registry = DrmCleanupRegistry::new_with_io(device_key, incarnation, Box::new(io));

    // GemOwner::Gbm: RMFB is issued, but GEM_CLOSE is NEVER issued by the right
    let right = registry.register_right(20, 21, GemOwner::Gbm);
    registry.consume(right).unwrap();

    assert_eq!(calls.borrow().as_slice(), &[CleanupCall::RemoveFb(20)]);
}

#[test]
fn c0_2ci_drm_cleanup_frozen_rights_and_family_closed_reject_ioctls() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut registry = DrmCleanupRegistry::new_with_io(device_key, incarnation, Box::new(io));

    registry.freeze_incarnation();
    assert!(registry.is_frozen());

    let right = registry.register_right(30, 31, GemOwner::Right);
    let (_err, returned) = registry.consume(right).unwrap_err();
    assert_eq!(returned.state(), RightState::Frozen);
    // No transport call was issued
    assert!(calls.borrow().is_empty());
}

#[test]
fn c0_2ci_drm_cleanup_fake_family_barrier_requires_all_closed() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut registry = DrmCleanupRegistry::new_with_io(device_key, incarnation, Box::new(io));

    registry.init_fake_family();
    registry.add_fake_alias();
    registry.register_payload_alias();

    // Fails because control is still open
    assert!(registry.try_mint_file_family_closed().is_err());
    registry.close_fake_control();

    // Fails because helper is not reaped
    assert!(registry.try_mint_file_family_closed().is_err());
    registry.reap_fake_helper();

    // Fails because fake alias is still open
    assert!(registry.try_mint_file_family_closed().is_err());
    registry.remove_fake_alias();

    // Fails because payload alias is still open
    assert!(registry.try_mint_file_family_closed().is_err());
    registry.unregister_payload_alias();

    // Now succeeds
    let proof = registry.try_mint_file_family_closed().unwrap();
    assert!(registry.is_family_closed());

    // Cannot mint twice
    assert!(registry.try_mint_file_family_closed().is_err());

    // Consuming right after closure fails closed
    let right = registry.register_right(40, 41, GemOwner::Right);
    assert!(registry.consume(right).is_err());
    assert!(calls.borrow().is_empty());

    // Test retirement with valid proof
    registry.retire_closed_family(proof);
    assert!(registry.is_family_closed());
}

#[test]
fn c0_2ci_drm_cleanup_shared_gpu_dependency_persists_after_file_rights_discharge() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();

    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    let mut registry = DrmCleanupRegistry::new_with_io(key.device, key.incarnation, Box::new(io));

    let right = registry.register_right(50, 51, GemOwner::Right);
    let gpu_obligation = service.register(key, ObligationKind::Gpu).unwrap();
    drop(held);

    // Consume file-owned right
    registry.consume(right).unwrap();
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(50), CleanupCall::CloseGem(51)]
    );

    // File-owned right discharged, but shared GPU work still holds the allocation!
    service.service_ready();
    assert_eq!(drops.get(), 0);

    // Discharging GPU proof releases shared half and allows destruction
    service.apply_validated_proof(key, gpu_obligation).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 1);
}

#[test]
fn c0_2ci_drm_cleanup_round3_b1_counted_alias_real_device_barrier() {
    use crate::drm::Device;

    let device = Rc::new(Device::for_tests().unwrap());
    let weak = Rc::downgrade(&device);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();

    let mut registry = DrmCleanupRegistry::new_with_device_and_io(
        Rc::clone(&device),
        device_key,
        incarnation,
        Box::new(io),
    );

    // Payload owns a counted alias
    let payload_device_alias = Rc::clone(&device);
    registry.register_payload_alias();

    // External control alias
    let control_alias = Rc::clone(&device);

    // Drop fixture handle so only control, payload, and registry hold references
    drop(device);

    // While external control alias exists, barrier cannot be minted
    assert!(registry.try_mint_file_family_closed().is_err());

    // Detach control alias
    drop(control_alias);

    // While payload alias exists, barrier cannot be minted
    assert!(registry.try_mint_file_family_closed().is_err());

    // Discharging payload drops the alias and unregisters
    drop(payload_device_alias);
    registry.unregister_payload_alias();

    // Now barrier is mintable! Registry performs the description's last close
    let proof = registry.try_mint_file_family_closed().unwrap();
    assert!(registry.is_family_closed());
    // Weak reference confirms description's last close was performed by the registry
    assert!(weak.upgrade().is_none());
    assert!(calls.borrow().is_empty());

    registry.retire_closed_family(proof);
}

#[test]
fn c0_2ci_direct_probe_framebuffer_into_managed() {
    use crate::drm::{
        Device,
        modeset::{DirectScanoutProbeFramebuffer, ProbeFbOwnership},
    };
    use std::num::NonZeroU32;

    let device = Rc::new(Device::for_tests().unwrap());
    let (_service, held, _drops) = spy_service();
    let key = held.key();

    let mut registry = DrmCleanupRegistry::new_with_io(
        key.device,
        key.incarnation,
        Box::new(MockCleanupIo::new(Rc::new(RefCell::new(Vec::new())))),
    );

    let probe_fb = DirectScanoutProbeFramebuffer {
        inner: ProbeFbOwnership::Legacy {
            device,
            fb: drm::control::framebuffer::Handle::from(NonZeroU32::new(100).unwrap()),
            gem: drm::buffer::Handle::from(NonZeroU32::new(101).unwrap()),
        },
    };

    let direct_alloc = probe_fb.into_managed(&mut registry, held);
    assert_eq!(u32::from(direct_alloc.fb_handle()), 100);
    assert_eq!(u32::from(direct_alloc.gem_handle()), 101);
    assert_eq!(direct_alloc.gem_owner, GemOwner::Right);
    assert!(direct_alloc.right().is_some());
    assert!(direct_alloc.source_lease().is_some());
}
