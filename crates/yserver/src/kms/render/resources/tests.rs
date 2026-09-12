use std::{cell::Cell, rc::Rc, sync::Arc};

use ash::vk::{self, Handle};

use super::*;
use crate::{
    kms::{
        owner::{
            device::{DeviceCommitOwner, OwnerEvent},
            identity::{CommitId, IncarnationId},
            lifecycle::LifecycleEpochId,
        },
        render::platform::CrtcKey,
        vk::device::VkContext,
    },
    platform::drm::DrmDeviceKey,
};

/// Used only by the `_vulkan` tests that bind a real `GpuObligation` (via
/// `GpuObligation::new`) to exercise the real `poll_signaled_result` path,
/// or that otherwise need a genuine live device. R12: an absent ICD is an
/// honest `panic!`, never a silent pass.
fn real_vk_context() -> Arc<VkContext> {
    match VkContext::new() {
        Ok(vk) => vk,
        Err(e) => {
            panic!("environmental skip: no live Vulkan ICD available ({e:?}); not claiming pass")
        }
    }
}

#[derive(Debug)]
pub(crate) struct SpyAllocation {
    pub(crate) drops: Rc<Cell<usize>>,
}

impl Drop for SpyAllocation {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

pub(crate) fn spy_service() -> (ResourceService, AllocationLease, Rc<Cell<usize>>) {
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
pub(crate) struct MockCleanupIo {
    pub(crate) calls: Rc<RefCell<Vec<CleanupCall>>>,
    pub(crate) fail_fb: Rc<Cell<bool>>,
    pub(crate) fail_gem: Rc<Cell<bool>>,
}

impl MockCleanupIo {
    pub(crate) fn new(calls: Rc<RefCell<Vec<CleanupCall>>>) -> Self {
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

    // A payload alias is registered up front. The panicking closure below
    // therefore proves discharge is never *reached* while any precondition
    // is unsatisfied -- not merely that there was nothing to discharge.
    let key = AllocationKey {
        device: device_key,
        incarnation,
        generation: 1,
    };
    registry.register_payload_alias(key);

    let panicking_discharge =
        |_: &mut DrmCleanupRegistry, _: AllocationKey| -> std::io::Result<()> {
            panic!("discharge must not run before every precondition is satisfied")
        };

    // Fails because submitters are not detached
    assert!(
        registry
            .try_mint_file_family_closed(panicking_discharge)
            .is_err()
    );
    registry.detach_fake_submitters();

    // Fails because control is still open
    assert!(
        registry
            .try_mint_file_family_closed(panicking_discharge)
            .is_err()
    );
    registry.close_fake_control();

    // Fails because helper is not reaped
    assert!(
        registry
            .try_mint_file_family_closed(panicking_discharge)
            .is_err()
    );
    registry.reap_fake_helper();

    // Fails because fake alias is still open
    assert!(
        registry
            .try_mint_file_family_closed(panicking_discharge)
            .is_err()
    );
    registry.remove_fake_alias();

    // F1b-m1: the staged sequence above only proves each precondition is
    // *sufficient*, in combination with the others still unsatisfied, to
    // keep the mint failing -- deleting any single one of the four checks
    // in `try_mint_file_family_closed` would still fail every assertion up
    // to here, since whichever check comes next in the staged sequence
    // still returns `Err`. It proves nothing about any one check
    // independently. From a fresh, all-but-one-satisfied state per
    // condition, confirm the mint fails on exactly that condition: deleting
    // any one of the four checks now makes exactly one of these four
    // assertions pass when it must fail.
    for unsatisfied in 0..4 {
        registry.init_fake_family();
        if unsatisfied != 0 {
            registry.detach_fake_submitters();
        }
        if unsatisfied != 1 {
            registry.close_fake_control();
        }
        if unsatisfied != 2 {
            registry.reap_fake_helper();
        }
        if unsatisfied == 3 {
            registry.add_fake_alias();
        }
        assert!(
            registry
                .try_mint_file_family_closed(panicking_discharge)
                .is_err(),
            "condition {unsatisfied} must independently block the mint"
        );
    }
    // Restore the fully-satisfied state the rest of this test assumes.
    registry.init_fake_family();
    registry.detach_fake_submitters();
    registry.close_fake_control();
    registry.reap_fake_helper();

    // Now every precondition holds: discharge runs exactly once, for the
    // one registered payload alias, then the mint succeeds.
    let discharge_count = Rc::new(Cell::new(0));
    let counting_discharge = {
        let discharge_count = Rc::clone(&discharge_count);
        move |_: &mut DrmCleanupRegistry, discharge_key: AllocationKey| {
            assert_eq!(discharge_key, key);
            discharge_count.set(discharge_count.get() + 1);
            Ok(())
        }
    };
    let proof = registry
        .try_mint_file_family_closed(counting_discharge)
        .unwrap();
    assert_eq!(discharge_count.get(), 1);
    assert!(registry.is_family_closed());

    // Cannot mint twice
    assert!(
        registry
            .try_mint_file_family_closed(panicking_discharge)
            .is_err()
    );

    // Consuming right after closure fails closed
    let right = registry.register_right(40, 41, GemOwner::Right);
    assert!(registry.consume(right).is_err());
    assert!(calls.borrow().is_empty());

    // Test retirement with valid proof
    registry.retire_closed_family(proof).unwrap();
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
fn c0_2ci_drm_cleanup_fd_family_barrier_discharges_payload_alias() {
    use crate::drm::Device;
    use std::num::NonZeroU32;

    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut service = ResourceService::new(device_key, incarnation);

    let device = Rc::new(Device::for_tests().unwrap());
    let weak = Rc::downgrade(&device);

    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    // The registry itself holds a counted alias (its own, per R5 step 3),
    // distinct from the payload's -- so this test can tell apart "the
    // payload's own drop happened to be the last close" from "the registry
    // performed the last close", which is what R5 step 3 actually requires.
    let mut registry = DrmCleanupRegistry::new_with_device_and_io(
        Rc::clone(&device),
        device_key,
        incarnation,
        Box::new(io),
    );
    registry.detach_fake_submitters();
    registry.reap_fake_helper();
    registry.close_fake_control();

    let right = registry.register_right(60, 61, GemOwner::Right);
    let payload = DirectFramebufferAllocation::new(
        right,
        None,
        drm::control::framebuffer::Handle::from(NonZeroU32::new(60).unwrap()),
        drm::buffer::Handle::from(NonZeroU32::new(61).unwrap()),
        GemOwner::Right,
        Some(Rc::clone(&device)),
    );
    // F2-M1: adopt refuses a file-owned payload directly; adopt_with_registry
    // is the only path, and it registers the alias itself.
    let held = service
        .adopt_with_registry(AllocationPayload::DirectFramebuffer(payload), &mut registry)
        .unwrap();
    let key = held.key();

    // Only the registry's own alias and the payload's alias remain.
    drop(device);

    // The barrier is mintable *while the payload still holds its alias*
    // (R5) — the registry does not wait for it to drop, it discharges it.
    let proof = registry
        .try_mint_file_family_closed(|registry, discharge_key| {
            assert_eq!(discharge_key, key);
            let entry = service.entries.get(&discharge_key).expect("entry present");
            let mut payload = entry.payload.borrow_mut();
            let result = match payload.as_mut() {
                Some(AllocationPayload::DirectFramebuffer(alloc)) => {
                    alloc.discharge_file_owned(registry)
                }
                _ => Ok(()),
            };
            // Step 2 (discharging the payload's alias) is not step 3 (the
            // registry's own last close): the registry still holds its own
            // alias right here, so the description is not yet closed.
            assert!(
                weak.upgrade().is_some(),
                "the payload's own discharge must not be the description's last close"
            );
            result
        })
        .unwrap();

    assert!(registry.is_family_closed());
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(60), CleanupCall::CloseGem(61)]
    );

    {
        let entry = service.entries.get(&key).unwrap();
        let payload = entry.payload.borrow();
        match payload.as_ref() {
            Some(AllocationPayload::DirectFramebuffer(alloc)) => {
                assert!(
                    alloc.right().is_none(),
                    "file-owned right must be discharged by the barrier"
                );
                assert!(
                    alloc.device.is_none(),
                    "device alias must be dropped by the barrier discharge"
                );
            }
            other => panic!("expected DirectFramebuffer payload, got {other:?}"),
        }
    }

    // Only now -- after the mint itself performs step 3 -- is the
    // description's last alias (the registry's own) closed.
    assert!(weak.upgrade().is_none());

    // No ioctl follows the mint.
    let calls_before_retire = calls.borrow().len();
    registry.retire_closed_family(proof).unwrap();
    assert_eq!(calls.borrow().len(), calls_before_retire);
}

#[test]
fn c0_2ci_drm_cleanup_fd_family_barrier_discharge_failure_retries() {
    use crate::drm::Device;
    use std::num::NonZeroU32;

    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut service = ResourceService::new(device_key, incarnation);

    let device = Rc::new(Device::for_tests().unwrap());
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    io.fail_gem.set(true);
    let mut registry =
        DrmCleanupRegistry::new_with_io(device_key, incarnation, Box::new(io.clone()));
    registry.detach_fake_submitters();
    registry.reap_fake_helper();
    registry.close_fake_control();

    let right = registry.register_right(70, 71, GemOwner::Right);
    let payload = DirectFramebufferAllocation::new(
        right,
        None,
        drm::control::framebuffer::Handle::from(NonZeroU32::new(70).unwrap()),
        drm::buffer::Handle::from(NonZeroU32::new(71).unwrap()),
        GemOwner::Right,
        Some(device),
    );
    // F2-M1: adopt refuses a file-owned payload directly; adopt_with_registry
    // is the only path, and it registers the alias itself.
    let held = service
        .adopt_with_registry(AllocationPayload::DirectFramebuffer(payload), &mut registry)
        .unwrap();
    let key = held.key();

    let discharge = |registry: &mut DrmCleanupRegistry, discharge_key: AllocationKey| {
        let entry = service.entries.get(&discharge_key).expect("entry present");
        let mut payload = entry.payload.borrow_mut();
        match payload.as_mut() {
            Some(AllocationPayload::DirectFramebuffer(alloc)) => {
                alloc.discharge_file_owned(registry)
            }
            _ => Ok(()),
        }
    };

    // First mint attempt fails partway through the walk, at close_gem: the
    // key stays registered, the right stays retryable, family stays open.
    let err = registry.try_mint_file_family_closed(discharge).unwrap_err();
    assert_eq!(err.to_string(), "simulated close_gem failure");
    assert_eq!(registry.payload_aliases(), 1);
    assert!(!registry.is_family_closed());
    {
        let entry = service.entries.get(&key).unwrap();
        let payload = entry.payload.borrow();
        match payload.as_ref() {
            Some(AllocationPayload::DirectFramebuffer(alloc)) => {
                assert_eq!(
                    alloc.right().expect("right retained for retry").state(),
                    RightState::FramebufferRemoved
                );
            }
            other => panic!("expected DirectFramebuffer payload, got {other:?}"),
        }
    }
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(70), CleanupCall::CloseGem(71)]
    );

    // Clear the simulated failure and retry: RMFB is not re-issued, only
    // CloseGem is retried, and the mint now succeeds.
    io.fail_gem.set(false);
    let proof = registry.try_mint_file_family_closed(discharge).unwrap();
    assert!(registry.is_family_closed());
    assert_eq!(registry.payload_aliases(), 0);
    assert_eq!(
        calls.borrow().as_slice(),
        &[
            CleanupCall::RemoveFb(70),
            CleanupCall::CloseGem(71),
            CleanupCall::CloseGem(71),
        ]
    );
    registry.retire_closed_family(proof).unwrap();
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

#[test]
fn c0_2ci_scanout_shared_pool_read_obligation_gates_reuse() {
    let mut service = ResourceService::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        IncarnationId::first(),
    );
    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let alloc = ScanoutAllocation::new(None, shared);
    let held = service.adopt(AllocationPayload::Scanout(alloc)).unwrap();
    let reader = service.reserve(held.key(), UseKind::Read).unwrap();

    let key = held.key();
    let kms = service.register(key, ObligationKind::KmsRelease).unwrap();
    let read = service.register(key, ObligationKind::Read).unwrap();
    service.apply_validated_proof(key, kms).unwrap();
    service.service_ready();
    assert!(matches!(
        service.reserve(key, UseKind::Write),
        Err(ResourceError::Busy)
    ));
    service.apply_validated_proof(key, read).unwrap();
    drop(reader);
    service.service_ready();
    let acquired = service.reserve(key, UseKind::Write).unwrap();
    assert_eq!(acquired.key(), held.key());
}

#[test]
fn c0_2ci_scanout_shared_pool_kms_obligation_gates_reuse() {
    let mut service = ResourceService::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        IncarnationId::first(),
    );
    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let alloc = ScanoutAllocation::new(None, shared);
    let held = service.adopt(AllocationPayload::Scanout(alloc)).unwrap();
    let reader = service.reserve(held.key(), UseKind::Read).unwrap();

    let key = held.key();
    let kms = service.register(key, ObligationKind::KmsRelease).unwrap();
    let read = service.register(key, ObligationKind::Read).unwrap();
    service.apply_validated_proof(key, read).unwrap();
    drop(reader);
    service.service_ready();
    assert!(matches!(
        service.reserve(key, UseKind::Write),
        Err(ResourceError::Busy)
    ));
    service.apply_validated_proof(key, kms).unwrap();
    service.service_ready();
    let acquired = service.reserve(key, UseKind::Write).unwrap();
    assert_eq!(acquired.key(), held.key());
}

#[test]
fn c0_2ci_scanout_copied_pair_sink_dependency_gates_reuse() {
    let mut service = ResourceService::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        IncarnationId::first(),
    );
    let display_alloc = ScanoutAllocation::new(
        None,
        SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        ),
    );
    let display_held = service
        .adopt(AllocationPayload::Scanout(display_alloc))
        .unwrap();
    let display_key = display_held.key();

    let renderer_alloc = CopiedSourceAllocation::mock(
        ash::vk::Semaphore::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        crate::kms::vk::scanout::CopiedSourceOwnership::ForeignAwaitingSink,
    );
    let renderer_held = service
        .adopt(AllocationPayload::CopiedSource(renderer_alloc))
        .unwrap();
    let renderer_key = renderer_held.key();

    let kms = service
        .register(display_key, ObligationKind::KmsRelease)
        .unwrap();
    let sink_foreign = service
        .register(renderer_key, ObligationKind::ForeignReturn)
        .unwrap();

    // Retire display BO from KMS
    service.apply_validated_proof(display_key, kms).unwrap();
    service.service_ready();

    // Destination write or renderer transport reuse must NOT be allowed while sink dependency remains
    assert!(matches!(
        service.reserve(renderer_key, UseKind::Write),
        Err(ResourceError::Busy)
    ));

    // Discharge sink dependency
    service
        .apply_validated_proof(renderer_key, sink_foreign)
        .unwrap();
    service.service_ready();

    let acquired_disp = service.reserve(display_key, UseKind::Write).unwrap();
    let acquired_rend = service.reserve(renderer_key, UseKind::Write).unwrap();
    assert_eq!(acquired_disp.key(), display_key);
    assert_eq!(acquired_rend.key(), renderer_key);
}

#[test]
fn c0_2ci_scanout_file_owned_pairing_gbm_and_right() {
    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let right_gbm = DrmCleanupRight::new(device_key, IncarnationId::first(), 10, 20, GemOwner::Gbm);
    let right_val =
        DrmCleanupRight::new(device_key, IncarnationId::first(), 11, 21, GemOwner::Right);

    // GemOwner::Gbm requires Some(gbm_bo); None is rejected with InvalidState
    assert_eq!(
        FileOwnedBacking::new(right_gbm, None, Rc::clone(&device)).err(),
        Some(ResourceError::InvalidState)
    );

    // GemOwner::Right requires None; succeeds
    assert!(FileOwnedBacking::new(right_val, None, Rc::clone(&device)).is_ok());
}

#[test]
fn c0_2ci_scanout_file_owned_discharge_right_exactly_one_close_gem() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let mut registry =
        DrmCleanupRegistry::new_with_io(device_key, IncarnationId::first(), Box::new(io));
    let right = DrmCleanupRight::new(device_key, IncarnationId::first(), 55, 66, GemOwner::Right);
    let fo = FileOwnedBacking::new(right, None, device).unwrap();

    fo.discharge(&mut registry).unwrap();

    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(55), CleanupCall::CloseGem(66)]
    );
}

#[test]
fn c0_2ci_scanout_file_owned_discharge_right_retry_on_close_gem_failure() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    io.fail_gem.set(true);

    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let mut registry =
        DrmCleanupRegistry::new_with_io(device_key, IncarnationId::first(), Box::new(io.clone()));
    let right = DrmCleanupRight::new(device_key, IncarnationId::first(), 55, 66, GemOwner::Right);
    let fo = FileOwnedBacking::new(right, None, device).unwrap();

    let (_err, returned_fo) = fo.discharge(&mut registry).unwrap_err();
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(55), CleanupCall::CloseGem(66)]
    );

    // Clear failure and retry
    io.fail_gem.set(false);
    returned_fo.discharge(&mut registry).unwrap();

    // RMFB was NOT re-issued on retry; exactly one remove_fb and close_gem was retried!
    assert_eq!(
        calls.borrow().as_slice(),
        &[
            CleanupCall::RemoveFb(55),
            CleanupCall::CloseGem(66),
            CleanupCall::CloseGem(66)
        ]
    );
}

#[test]
fn c0_2ci_scanout_discharging_file_owned_leaves_shared_intact() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let mut registry =
        DrmCleanupRegistry::new_with_io(device_key, IncarnationId::first(), Box::new(io));
    let right = DrmCleanupRight::new(device_key, IncarnationId::first(), 70, 71, GemOwner::Right);
    let fo = FileOwnedBacking::new(right, None, device).unwrap();

    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let alloc = ScanoutAllocation::new(Some(fo), shared);

    let mut service = ResourceService::new(device_key, IncarnationId::first());
    // F2-M1: adopt refuses a file-owned payload directly.
    let held = service
        .adopt_with_registry(AllocationPayload::Scanout(alloc), &mut registry)
        .unwrap();
    let key = held.key();

    // A pending GPU obligation belongs to the *shared* half (R4): it is
    // never satisfied by discharging file_owned.
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();
    drop(held);

    {
        let entry = service.entries.get(&key).unwrap();
        let mut payload = entry.payload.borrow_mut();
        let Some(AllocationPayload::Scanout(alloc)) = payload.as_mut() else {
            panic!("expected Scanout payload");
        };
        alloc.discharge_file_owned(&mut registry).unwrap();
        assert!(alloc.file_owned().is_none());
    }
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(70), CleanupCall::CloseGem(71)]
    );

    // Shared half remains genuinely intact: the entry is not destroyable
    // and a write reservation stays Busy, purely on the strength of the
    // still-pending GPU obligation -- file_owned discharge touched neither.
    service.service_ready();
    assert!(
        service.contains(&key),
        "entry must survive: shared's GPU obligation is still pending"
    );
    assert!(matches!(
        service.reserve(key, UseKind::Write),
        Err(ResourceError::Busy)
    ));

    // Discharging the shared-side proof is what finally frees it.
    service.apply_validated_proof(key, gpu).unwrap();
    service.service_ready();
    assert!(!service.contains(&key));
}

// This exercises the `ResourceService`-level all-or-nothing reservation
// mechanism `acquire_managed_scanout_bo` relies on, not the platform
// function itself (which needs a real `ScanoutBo`/`Arc<VkContext>`; see
// `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan` in
// `adapter_tests.rs` for that half, hardware-gated).
#[test]
fn c0_2ci_scanout_all_or_nothing_reservation_retains_gpu_obligation_on_partial_failure() {
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let mut service = ResourceService::new(device_key, IncarnationId::first());

    let display_alloc = ScanoutAllocation::new(
        None,
        SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        ),
    );
    let display_held = service
        .adopt(AllocationPayload::Scanout(display_alloc))
        .unwrap();
    let display_key = display_held.key();

    let renderer_alloc = CopiedSourceAllocation::mock(
        ash::vk::Semaphore::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        crate::kms::vk::scanout::CopiedSourceOwnership::ForeignAwaitingSink,
    );
    let renderer_held = service
        .adopt(AllocationPayload::CopiedSource(renderer_alloc))
        .unwrap();
    let renderer_key = renderer_held.key();

    // Reader on renderer so renderer write reservation will be busy
    let _renderer_read = service.reserve(renderer_key, UseKind::Read).unwrap();

    // All-or-nothing: reserving display succeeds
    let display_res = service.reserve(display_key, UseKind::Write);
    assert!(display_res.is_ok());
    let display_lease = display_res.unwrap();

    // GPU obligation armed during preparation
    let display_gpu = service.register(display_key, ObligationKind::Gpu).unwrap();

    // Renderer reservation fails with Busy
    let renderer_res = service.reserve(renderer_key, UseKind::Write);
    assert!(matches!(renderer_res, Err(ResourceError::Busy)));

    // On failure of second reservation, drop first reservation without clearing GPU obligation
    drop(display_lease);

    // GPU obligation on display remains intact and armed, blocking new write
    assert_eq!(
        service
            .entries
            .get(&display_key)
            .unwrap()
            .pending_obligation_count(),
        1
    );
    assert!(matches!(
        service.reserve(display_key, UseKind::Write),
        Err(ResourceError::Busy)
    ));

    service
        .apply_validated_proof(display_key, display_gpu)
        .unwrap();
    service.service_ready();
    assert!(service.reserve(display_key, UseKind::Write).is_ok());
}

// This exercises the `ResourceService`-level obligation mechanism that
// makes "cancel recording ends recording, not in-flight work" true --
// `cancel_scanout_bo_recording` itself only ever touches pool-level
// `BoState`, never the service, so this is the whole of what there is to
// prove deterministically. The platform function is exercised for real in
// `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan`
// (`adapter_tests.rs`, hardware-gated).
#[test]
fn c0_2ci_scanout_write_reservation_stays_busy_until_gpu_proof_applied() {
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let mut service = ResourceService::new(device_key, IncarnationId::first());
    let alloc = ScanoutAllocation::new(
        None,
        SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        ),
    );
    let held = service.adopt(AllocationPayload::Scanout(alloc)).unwrap();
    let key = held.key();

    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    // Cancel recording: does NOT cancel in-flight GPU dispatch
    assert!(matches!(
        service.reserve(key, UseKind::Write),
        Err(ResourceError::Busy)
    ));

    // Only actual GPU proof completes it
    service.apply_validated_proof(key, gpu).unwrap();
    service.service_ready();
    assert!(service.reserve(key, UseKind::Write).is_ok());
}

#[test]
fn c0_2ci_scanout_partial_grouped_replacement_leaves_shared_source_retained() {
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let mut service = ResourceService::new(device_key, IncarnationId::first());
    let shared_source = ScanoutAllocation::new(
        None,
        SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        ),
    );
    let held = service
        .adopt(AllocationPayload::Scanout(shared_source))
        .unwrap();
    let key = held.key();

    // Output 1 and Output 2 both register obligations on the shared source
    let out1_kms = service.register(key, ObligationKind::KmsRelease).unwrap();
    let out2_kms = service.register(key, ObligationKind::KmsRelease).unwrap();

    // Grouped commit replaces Output 1's buffer
    service.apply_validated_proof(key, out1_kms).unwrap();
    service.service_ready();

    // Shared source remains retained because Output 2 still holds its obligation (R4)
    assert!(matches!(
        service.reserve(key, UseKind::Write),
        Err(ResourceError::Busy)
    ));

    // Output 2 replaces buffer
    service.apply_validated_proof(key, out2_kms).unwrap();
    service.service_ready();

    // Now shared source is eligible for reuse
    assert!(service.reserve(key, UseKind::Write).is_ok());
}

// `c0_2ci_scanout_topology_reuse_of_bo_index` used to live here; its
// assertion (`pool.display_pool().bos.len() == 0`) held trivially on the
// always-empty `ScanoutBoPool::for_tests()` fixture regardless of what
// `detach_managed_entries` does. Proving detach actually clears a real
// `managed_key` needs a real `ScanoutBo` (`Arc<VkContext>`, not `Option`),
// so that coverage now lives in
// `c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan`
// (`adapter_tests.rs`, hardware-gated).

/// Opens the DRM render node paired with the first real `/dev/dri/cardN`,
/// per the plan's 9.5 fixture note: `Device::for_tests()` is a Unix socket
/// and cannot back a `GbmDevice`. `None` when no real DRM hardware is
/// present.
fn open_test_render_node() -> Option<crate::drm::Device> {
    let card = crate::kms::executor::test_support::TestDevice::open_real_drm_or_ignore()?;
    let render = crate::kms::render_node::open_for_card(&card).ok()?;
    crate::drm::Device::open_render_node(render.path().to_str()?).ok()
}

/// 9.5 (round-3 B-1), real case: a `GbmDevice` over a real DRM render node,
/// not the fake family inventory. Also closes B-12's `GemOwner::Gbm`
/// coverage (F-2): the right's transport (the counting mock, so `RMFB` is
/// observed rather than issued against the render node) sees zero
/// `CloseGem` -- the real `gbm_bo`'s own drop is the sole GEM closer.
#[test]
#[ignore = "requires a real DRM render node; run explicitly"]
fn c0_2ci_fd_family_barrier_real_gbm_payload_drm() {
    let Some(device) = open_test_render_node() else {
        panic!("requires a real DRM render node; none available");
    };
    let device = Rc::new(device);
    let weak = Rc::downgrade(&device);

    let gbm_device =
        gbm::Device::new(Rc::clone(&device)).expect("gbm_create_device on render node");
    let gbm_bo = gbm_device
        .create_buffer_object::<()>(
            64,
            64,
            gbm::Format::Xrgb8888,
            gbm::BufferObjectFlags::RENDERING | gbm::BufferObjectFlags::SCANOUT,
        )
        .expect("create real gbm buffer object");

    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut service = ResourceService::new(device_key, incarnation);

    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    // F2-M3: the registry holds its own alias too (`new_with_device_and_io`),
    // distinct from the payload's -- a device-less registry cannot show
    // that the *registry* performs the description's last close (step 3);
    // it would only show that the payload's own discharge happened to be
    // the last one, which is the reverse of what R5 step 3 requires (this
    // is F1-M1's exact mistake, repeated here in the real-GBM case).
    let mut registry = DrmCleanupRegistry::new_with_device_and_io(
        Rc::clone(&device),
        device_key,
        incarnation,
        Box::new(io),
    );
    registry.detach_fake_submitters();
    registry.reap_fake_helper();
    registry.close_fake_control();

    let right = registry.register_right(9101, 9102, GemOwner::Gbm);
    let fo = FileOwnedBacking::new(right, Some(gbm_bo), Rc::clone(&device)).unwrap();
    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let alloc = ScanoutAllocation::new(Some(fo), shared);
    let held = service
        .adopt_with_registry(AllocationPayload::Scanout(alloc), &mut registry)
        .unwrap();
    let key = held.key();

    // `gbm_device` was only ever a factory for `gbm_bo`; it holds its own
    // alias and must go, same as the test's original `device` binding, so
    // only the registry's own alias and the payload's alias remain.
    drop(gbm_device);
    drop(device);

    // The barrier is mintable *while the payload still holds its alias*
    // (R5) -- reap/control-alias closure are the only preconditions here,
    // exactly as for the deterministic case in F-1/F-1b.
    let proof = registry
        .try_mint_file_family_closed(|registry, discharge_key| {
            assert_eq!(discharge_key, key);
            let entry = service.entries.get(&discharge_key).expect("entry present");
            let mut payload = entry.payload.borrow_mut();
            let result = match payload.as_mut() {
                Some(AllocationPayload::Scanout(alloc)) => alloc.discharge_file_owned(registry),
                _ => Ok(()),
            };
            // Step 2 (discharging the payload's alias -- the real gbm_bo's
            // GEM_CLOSE) is not step 3 (the registry's own last close): the
            // registry still holds its own alias right here, so the
            // description is not yet closed. With the registry holding no
            // alias of its own, this assertion would be checking a no-op.
            assert!(
                weak.upgrade().is_some(),
                "the payload's own discharge must not be the description's last close"
            );
            result
        })
        .unwrap();

    assert!(registry.is_family_closed());
    // B-12: zero CloseGem for GemOwner::Gbm -- RMFB is the only ioctl the
    // right itself issues; the real gbm_bo's own drop (already run, inside
    // `discharge_file_owned` above) was the sole GEM closer.
    assert_eq!(calls.borrow().as_slice(), &[CleanupCall::RemoveFb(9101)]);
    // F2-B2: the alias is unregistered once discharged through the barrier
    // -- the same bookkeeping the normal path (service_ready_with_registry)
    // performs, so a later teardown never hands this key to a callback with
    // nothing left to look up.
    assert_eq!(registry.payload_aliases(), 0);

    // Only now -- after the mint itself performs step 3 -- is the
    // description's last alias (the registry's own) closed.
    assert!(weak.upgrade().is_none());

    let calls_before_retire = calls.borrow().len();
    registry.retire_closed_family(proof).unwrap();
    assert_eq!(calls.borrow().len(), calls_before_retire);
}

#[test]
fn c0_2ci_release_fresh_adoption_reclaims_untouched_lease() {
    // F2-M2: register_managed_scanout_bo's all-or-nothing rollback needs a
    // way to un-adopt a payload it adopted moments ago when a paired
    // adoption fails afterward.
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let payload = match service.release_fresh_adoption(held) {
        Ok(payload @ AllocationPayload::Spy(_)) => payload,
        other => panic!("expected the Spy payload back, got {other:?}"),
    };
    assert!(!service.contains(&key));
    assert_eq!(
        drops.get(),
        0,
        "release_fresh_adoption hands the payload back; it must not drop it"
    );
    drop(payload);
    assert_eq!(
        drops.get(),
        1,
        "the caller's own drop is what finally runs it"
    );
}

#[test]
fn c0_2ci_release_fresh_adoption_refuses_when_something_else_is_using_it() {
    let (mut service, held, _drops) = spy_service();
    let key = held.key();
    let _extra_use = service.reserve(key, UseKind::Read).unwrap();
    match service.release_fresh_adoption(held) {
        Err(_lease) => {}
        Ok(payload) => {
            panic!("must not hard-reclaim while something else uses the entry, got {payload:?}")
        }
    }
    assert!(
        service.contains(&key),
        "the lease was handed back, not consumed; the entry must still be there"
    );
}

#[test]
fn c0_2ci_scanout_adopt_with_registry_registers_file_owned_alias_only() {
    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut service = ResourceService::new(device_key, incarnation);
    let mut registry = DrmCleanupRegistry::new_with_io(
        device_key,
        incarnation,
        Box::new(MockCleanupIo::new(Rc::new(RefCell::new(Vec::new())))),
    );

    // B-2: adopt_with_registry registers the alias for a payload carrying
    // one (Scanout with file_owned: Some), and only for that kind.
    let right = DrmCleanupRight::new(device_key, incarnation, 90, 91, GemOwner::Right);
    let fo = FileOwnedBacking::new(right, None, Rc::clone(&device)).unwrap();
    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let alloc = ScanoutAllocation::new(Some(fo), shared);
    let held = service
        .adopt_with_registry(AllocationPayload::Scanout(alloc), &mut registry)
        .unwrap();
    let key = held.key();
    assert_eq!(registry.payload_aliases(), 1);

    // A Scanout payload with file_owned: None registers nothing.
    let shared_only = ScanoutAllocation::new(
        None,
        SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        ),
    );
    let _shared_only_held = service
        .adopt_with_registry(AllocationPayload::Scanout(shared_only), &mut registry)
        .unwrap();
    assert_eq!(registry.payload_aliases(), 1);

    drop(held);
    registry.unregister_payload_alias(key);
    assert_eq!(registry.payload_aliases(), 0);
}

#[test]
fn c0_2ci_scanout_service_ready_with_registry_discharges_before_destroy() {
    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut service = ResourceService::new(device_key, incarnation);

    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut registry = DrmCleanupRegistry::new_with_io(
        device_key,
        incarnation,
        Box::new(MockCleanupIo::new(Rc::clone(&calls))),
    );

    let right = DrmCleanupRight::new(device_key, incarnation, 92, 93, GemOwner::Right);
    let fo = FileOwnedBacking::new(right, None, device).unwrap();
    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let alloc = ScanoutAllocation::new(Some(fo), shared);
    let held = service
        .adopt_with_registry(AllocationPayload::Scanout(alloc), &mut registry)
        .unwrap();
    let key = held.key();

    // Ordinary release: nothing else pending, only the Retain lease drops.
    drop(held);

    // B-2: service_ready_with_registry must discharge file_owned through
    // the registry before dropping the payload -- neither ScanoutAllocation
    // nor FileOwnedBacking has a Drop that closes the FB/GEM, so plain
    // service_ready here would silently leak them.
    let transitions = service.service_ready_with_registry(&mut registry);
    assert_eq!(transitions, vec![key]);
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(92), CleanupCall::CloseGem(93)]
    );
    assert!(!service.contains(&key));
    // F2-B2: the alias must be unregistered on successful discharge -- a
    // stale key here is exactly what makes the fd-family barrier walk
    // hand a since-destroyed key to the discharge callback at teardown.
    assert_eq!(registry.payload_aliases(), 0);
}

#[test]
fn c0_2ci_scanout_service_ready_with_registry_unregisters_only_the_discharged_alias() {
    // F2-B2: releasing one payload normally must not disturb a second,
    // still-outstanding payload's alias -- the barrier walk later must
    // invoke its discharge callback exactly once, for that second key only.
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut service = ResourceService::new(device_key, incarnation);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut registry = DrmCleanupRegistry::new_with_io(
        device_key,
        incarnation,
        Box::new(MockCleanupIo::new(Rc::clone(&calls))),
    );
    registry.detach_fake_submitters();
    registry.reap_fake_helper();
    registry.close_fake_control();

    let make_alloc = |fb: u32, gem: u32| {
        let device = Rc::new(crate::drm::Device::for_tests().unwrap());
        let right = DrmCleanupRight::new(device_key, incarnation, fb, gem, GemOwner::Right);
        let fo = FileOwnedBacking::new(right, None, device).unwrap();
        let shared = SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        );
        ScanoutAllocation::new(Some(fo), shared)
    };

    let released_alloc = make_alloc(96, 97);
    let released_held = service
        .adopt_with_registry(AllocationPayload::Scanout(released_alloc), &mut registry)
        .unwrap();

    let outstanding_alloc = make_alloc(98, 99);
    let outstanding_held = service
        .adopt_with_registry(AllocationPayload::Scanout(outstanding_alloc), &mut registry)
        .unwrap();
    let outstanding_key = outstanding_held.key();
    // Kept alive (not dropped): a live Retain lease is what makes this
    // payload genuinely still outstanding when the first payload releases,
    // rather than both becoming destroyable on the same tick.

    // Release the first payload normally.
    drop(released_held);
    service.service_ready_with_registry(&mut registry);
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(96), CleanupCall::CloseGem(97)]
    );
    assert_eq!(registry.payload_aliases(), 1);

    // Now mint the barrier: the callback must run exactly once, for the
    // still-outstanding key only.
    let discharge_count = Rc::new(Cell::new(0));
    let proof = registry
        .try_mint_file_family_closed(|registry, discharge_key| {
            assert_eq!(discharge_key, outstanding_key);
            discharge_count.set(discharge_count.get() + 1);
            let entry = service.entries.get(&discharge_key).expect("entry present");
            let mut payload = entry.payload.borrow_mut();
            match payload.as_mut() {
                Some(AllocationPayload::Scanout(alloc)) => alloc.discharge_file_owned(registry),
                _ => Ok(()),
            }
        })
        .unwrap();
    assert_eq!(discharge_count.get(), 1);
    assert_eq!(
        calls.borrow().as_slice(),
        &[
            CleanupCall::RemoveFb(96),
            CleanupCall::CloseGem(97),
            CleanupCall::RemoveFb(98),
            CleanupCall::CloseGem(99),
        ]
    );
    registry.retire_closed_family(proof).unwrap();
}

#[test]
fn c0_2ci_scanout_service_ready_with_registry_retries_failed_discharge() {
    // F2-B3: a failed discharge must keep the backing in `file_owned`, not
    // reinstall it and immediately take it right back out into the `Err`
    // (which left `file_owned` `None` after a *failed* discharge, so the
    // right at `FramebufferRemoved`, the device alias and, for `Gbm`, the
    // gbm_bo were silently dropped on the very next tick).
    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut service = ResourceService::new(device_key, incarnation);

    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(Rc::clone(&calls));
    io.fail_gem.set(true);
    let mut registry =
        DrmCleanupRegistry::new_with_io(device_key, incarnation, Box::new(io.clone()));

    let right = DrmCleanupRight::new(device_key, incarnation, 100, 101, GemOwner::Right);
    let fo = FileOwnedBacking::new(right, None, device).unwrap();
    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let alloc = ScanoutAllocation::new(Some(fo), shared);
    let held = service
        .adopt_with_registry(AllocationPayload::Scanout(alloc), &mut registry)
        .unwrap();
    let key = held.key();
    drop(held);

    // First tick: discharge fails at close_gem. The entry must survive with
    // its file-owned half intact, retryable from FramebufferRemoved.
    service.service_ready_with_registry(&mut registry);
    assert!(
        service.contains(&key),
        "entry must survive a failed discharge, not be dropped undischarged"
    );
    {
        let entry = service.entries.get(&key).unwrap();
        let payload = entry.payload.borrow();
        match payload.as_ref() {
            Some(AllocationPayload::Scanout(alloc)) => {
                let fo = alloc
                    .file_owned()
                    .expect("file_owned must survive a failed discharge");
                assert_eq!(fo.right().state(), RightState::FramebufferRemoved);
            }
            other => panic!("expected Scanout payload, got {other:?}"),
        }
    }
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(100), CleanupCall::CloseGem(101)]
    );
    assert_eq!(
        registry.payload_aliases(),
        1,
        "alias must not be unregistered on a failed discharge"
    );

    // Clear the failure and retry: RMFB is not re-issued, only CloseGem is
    // retried, and the entry is now destroyed.
    io.fail_gem.set(false);
    service.service_ready_with_registry(&mut registry);
    assert!(!service.contains(&key));
    assert_eq!(
        calls.borrow().as_slice(),
        &[
            CleanupCall::RemoveFb(100),
            CleanupCall::CloseGem(101),
            CleanupCall::CloseGem(101),
        ]
    );
    assert_eq!(registry.payload_aliases(), 0);
}

#[test]
fn c0_2ci_scanout_apply_teardown_release_refuses_live_file_owned() {
    let device = Rc::new(crate::drm::Device::for_tests().unwrap());
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut service = ResourceService::new(device_key, incarnation);
    let mut registry = DrmCleanupRegistry::new_with_io(
        device_key,
        incarnation,
        Box::new(MockCleanupIo::new(Rc::new(RefCell::new(Vec::new())))),
    );

    let right = DrmCleanupRight::new(device_key, incarnation, 94, 95, GemOwner::Right);
    let fo = FileOwnedBacking::new(right, None, device).unwrap();
    let shared = SharedBacking::mock(
        ash::vk::Image::null(),
        ash::vk::DeviceMemory::null(),
        ash::vk::ImageView::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        None,
    );
    let alloc = ScanoutAllocation::new(Some(fo), shared);
    // F2-M1: adopt refuses a file-owned payload directly.
    let held = service
        .adopt_with_registry(AllocationPayload::Scanout(alloc), &mut registry)
        .unwrap();
    let key = held.key();

    let crtc = CrtcKey::new(
        device_key,
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(1).unwrap()),
    );
    let member = GroupMember::new(crtc, 1, 1);
    let commit = CommitId::for_tests(402);
    let kms = service.register_kms(key, commit, member).unwrap();

    service.freeze(key).unwrap();
    service
        .record_kms_discharged(key, kms, commit, member)
        .unwrap();

    // M-10: the KMS obligation is legitimately Discharged, but file_owned
    // is still Some -- a live DRM framebuffer/GEM handle. Teardown release
    // proves only the KMS disposition; it must refuse until file_owned is
    // discharged separately (by the barrier or ordinary release), or this
    // is B-2's ioctl-after-barrier mechanism.
    let supervisor = RetainingSupervisor::new();
    let proof = supervisor.issue_teardown_release(incarnation, vec![key]);
    assert_eq!(
        service.apply_teardown_release(proof).err().unwrap(),
        ResourceError::InvalidProof
    );
}

// B-15's decisive test -- real `read_scanout_region` readback (the root
// IncludeInferiors snapshot path) plus a real async Vulkan submission
// polled through `poll_gpu`, never a fabricated
// `apply_validated_proof`/`test_signal` pair. It needs `KmsBackend`'s
// private `mod tests` fixtures (`for_tests_with_vk_live_scene`,
// `create_live_window`, `fill_rectangle`) that only that module's own test
// tree can reach, so it lives in `backend.rs`'s `mod tests` as
// `c0_2ci_read_source_scratch_regression_vulkan`, next to
// `root_get_image_reads_scanout_pixels_not_root_storage`, whose fixture it
// extends with managed source/scratch allocations.

#[test]
fn c0_2ci_read_uncertain_submission_leaves_source_and_staging_retained() {
    let (mut service, source_held, source_drops) = spy_service();
    let source_key = source_held.key();
    let source_read = service.register(source_key, ObligationKind::Read).unwrap();

    let (staging_held, staging_drops) = {
        let drops = Rc::new(Cell::new(0));
        let held = service
            .adopt(AllocationPayload::Spy(SpyAllocation {
                drops: Rc::clone(&drops),
            }))
            .unwrap();
        (held, drops)
    };
    let staging_key = staging_held.key();
    let staging_read = service.register(staging_key, ObligationKind::Read).unwrap();

    let mut batch = CoreRetirementBatch::new(Vec::new(), Vec::new(), true);
    batch.bind_read_obligation(ReadObligation::new(
        source_held,
        source_read,
        Some(staging_held),
        Some(staging_read),
    ));
    batch.test_ticket_status = Some(Err(ash::vk::Result::ERROR_DEVICE_LOST));

    service.register_batch(batch);
    let poll_res = service.poll_gpu(Instant::now());
    assert_eq!(poll_res, Err(ResourceError::Frozen));

    service.service_ready();
    // Uncertainty keeps both retained and frozen
    assert_eq!(source_drops.get(), 0);
    assert_eq!(staging_drops.get(), 0);
    assert!(service.entries.get(&source_key).unwrap().frozen());
    assert!(service.entries.get(&staging_key).unwrap().frozen());
}

// F4-B1: `GpuObligation.context` is `Option<Arc<VkContext>>` (the F5
// amendment at F2-m2) -- `#[cfg(test)] GpuObligation::for_tests_stub` builds
// one with `context: None`, which is legal *only* because every test below
// binds it through `CoreRetirementBatch::test_ticket_status`, which
// intercepts `ticket_status()` before it ever touches `context`. That
// restores these to deterministic, unignored `c0_2ci_` tests: the mechanism
// under test (`poll_gpu`/`validate_gpu_batch`/`commit_gpu_batch`/
// `quarantine_gpu_batch`) only ever consults `ticket_status()`'s return
// value, never how a real ticket reached it, so flipping
// `test_ticket_status` in place through `ResourceService::pending_batches_mut()`
// exercises the exact same branches as a real ticket would without needing a
// live device to construct the value at all.

#[test]
fn c0_2ci_gpu_batch_late_invalid_proof_is_atomic() {
    let (mut service, held_a, drops_a) = spy_service();
    let key_a = held_a.key();
    let gpu_a = service.register(key_a, ObligationKind::Gpu).unwrap();

    let (held_b, drops_b) = {
        let drops = Rc::new(Cell::new(0));
        let held = service
            .adopt(AllocationPayload::Spy(SpyAllocation {
                drops: Rc::clone(&drops),
            }))
            .unwrap();
        (held, drops)
    };
    let key_b = held_b.key();
    let invalid_gpu_b = ObligationId(9999);

    let batch_drop_counter = Rc::new(Cell::new(0));
    let mut batch = CoreRetirementBatch::new(
        vec![held_a, held_b],
        vec![vk::DescriptorSet::from_raw(0)],
        true,
    );
    batch.drop_counter = Some(Rc::clone(&batch_drop_counter));
    batch.bind_ticket(GpuObligation::for_tests_stub(
        vec![(key_a, gpu_a), (key_b, invalid_gpu_b)],
        crate::kms::render::platform::FenceTicket::for_tests_stub(),
    ));
    batch.test_ticket_status = Some(Ok(true));

    service.register_batch(batch);
    let poll_res = service.poll_gpu(Instant::now());
    assert_eq!(poll_res, Err(ResourceError::Frozen));

    // Valid obligation A followed by invalid B changes neither entry:
    // Entry A's obligation must NOT be removed
    let entry_a = service.entries.get(&key_a).unwrap();
    assert!(
        entry_a
            .availability
            .borrow()
            .pending_obligations
            .contains_key(&gpu_a)
    );
    // All actual allocation/descriptor counters at zero destruction
    service.service_ready();
    assert_eq!(drops_a.get(), 0);
    assert_eq!(drops_b.get(), 0);
    assert_eq!(batch_drop_counter.get(), 0);

    // Cover inverse order: invalid B followed by valid A
    let (mut service2, held_c, drops_c) = spy_service();
    let key_c = held_c.key();
    let gpu_c = service2.register(key_c, ObligationKind::Gpu).unwrap();
    let invalid_key = AllocationKey {
        device: service2.device(),
        incarnation: service2.incarnation(),
        generation: 9999,
    };

    let mut batch2 =
        CoreRetirementBatch::new(vec![held_c], vec![vk::DescriptorSet::from_raw(0)], true);
    batch2.bind_ticket(GpuObligation::for_tests_stub(
        vec![(invalid_key, ObligationId(1)), (key_c, gpu_c)],
        crate::kms::render::platform::FenceTicket::for_tests_stub(),
    ));
    batch2.test_ticket_status = Some(Ok(true));

    service2.register_batch(batch2);
    let poll_res2 = service2.poll_gpu(Instant::now());
    assert_eq!(poll_res2, Err(ResourceError::Frozen));
    let entry_c = service2.entries.get(&key_c).unwrap();
    assert!(
        entry_c
            .availability
            .borrow()
            .pending_obligations
            .contains_key(&gpu_c)
    );
    service2.service_ready();
    assert_eq!(drops_c.get(), 0);
}

#[test]
fn c0_2ci_gpu_batch_freeze_lookup_failure_handled() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    let stale_key = AllocationKey {
        device: service.device(),
        incarnation: service.incarnation(),
        generation: 8888,
    };

    let mut batch =
        CoreRetirementBatch::new(vec![held], vec![vk::DescriptorSet::from_raw(0)], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(
        vec![(key, gpu), (stale_key, ObligationId(1))],
        crate::kms::render::platform::FenceTicket::for_tests_stub(),
    ));
    batch.test_ticket_status = Some(Ok(true));

    service.register_batch(batch);
    // validate fails because stale_key is Detached
    let res = service.poll_gpu(Instant::now());
    assert_eq!(res, Err(ResourceError::Frozen));

    // Quarantine roots the batch and freezes existing correlated key without crashing on stale_key
    assert_eq!(service.quarantined_batches().len(), 1);
    assert!(service.entries.get(&key).unwrap().frozen());
    service.service_ready();
    assert_eq!(drops.get(), 0);
}

#[test]
fn c0_2ci_gpu_ticket_error_quarantines_batch() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    let mut batch =
        CoreRetirementBatch::new(vec![held], vec![vk::DescriptorSet::from_raw(0)], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(
        vec![(key, gpu)],
        crate::kms::render::platform::FenceTicket::for_tests_stub(),
    ));
    batch.test_ticket_status = Some(Err(ash::vk::Result::ERROR_DEVICE_LOST));

    service.register_batch(batch);
    assert_eq!(service.poll_gpu(Instant::now()), Err(ResourceError::Frozen));

    assert_eq!(service.quarantined_batches().len(), 1);
    assert!(service.entries.get(&key).unwrap().frozen());
    service.service_ready();
    assert_eq!(drops.get(), 0);
}

#[test]
fn c0_2ci_gpu_empty_ticket_when_possibly_dispatched_quarantines_batch() {
    let (mut service, held, drops) = spy_service();
    let _key = held.key();

    let batch = CoreRetirementBatch::new(vec![held], vec![vk::DescriptorSet::from_raw(0)], true);
    service.register_batch(batch);

    // Empty ticket when possibly_dispatched = true returns Err(ERROR_UNKNOWN)
    assert_eq!(service.poll_gpu(Instant::now()), Err(ResourceError::Frozen));
    assert_eq!(service.quarantined_batches().len(), 1);
    service.service_ready();
    assert_eq!(drops.get(), 0);
}

#[test]
fn c0_2ci_gpu_dropped_frame_metadata_with_live_ticket() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    let ticket = crate::kms::render::platform::FenceTicket::for_tests_stub();
    let mut batch =
        CoreRetirementBatch::new(vec![held], vec![vk::DescriptorSet::from_raw(0)], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(vec![(key, gpu)], ticket));
    batch.test_ticket_status = Some(Ok(false));

    service.register_batch(batch);

    // Simulate frame metadata dropped (e.g. replaced by newer damage/frame)
    // The batch and its underlying allocation must NOT be dropped while ticket is unsignaled
    service.poll_gpu(Instant::now()).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 0);
    assert_eq!(service.pending_batches().len(), 1);

    // Now ticket signals
    service.pending_batches_mut()[0].test_ticket_status = Some(Ok(true));
    service.poll_gpu(Instant::now()).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 1);
    assert_eq!(service.pending_batches().len(), 0);
}

// F4-B1: real-fence variant. The deterministic test above proves the state
// machine (retained while unsignaled, released once signaled) via
// `test_ticket_status`; this proves the actual `ticket_status()` ->
// `poll_signaled_result(&Arc<VkContext>)` path with a genuine submission and
// fence driving it, with no `test_ticket_status` override at all. It waits
// for the real submission to retire before polling, so it makes no claim
// about the unsignaled window (that would race a no-op submission -- see
// F4-M1) -- its evidence is that the real path retires the batch and
// releases the allocation once the device genuinely signals.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn c0_2ci_gpu_dropped_frame_metadata_with_live_ticket_vulkan() {
    let vk = real_vk_context();
    let ops_pool = crate::kms::vk::ops::OpsCommandPool::new(Arc::clone(&vk)).expect("ops pool");
    let fence_pool = crate::kms::render::platform::FencePool::new(Arc::clone(&vk));

    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    let ticket = fence_pool.acquire().expect("acquire real fence ticket");
    crate::kms::vk::ops::submit_one_shot_op_async(&vk, ops_pool.handle(), &ticket, |_vk, _cb| {
        Ok(())
    })
    .expect("submit real no-op");

    let mut batch =
        CoreRetirementBatch::new(vec![held], vec![vk::DescriptorSet::from_raw(0)], true);
    batch.bind_ticket(GpuObligation::new(
        vec![(key, gpu)],
        ticket.clone(),
        Arc::clone(&vk),
    ));
    service.register_batch(batch);

    // Wait for the real submission to retire, then let the service observe
    // the genuine signal through the real `poll_signaled_result` path.
    ticket.wait(&vk).expect("wait for real ticket");
    service.poll_gpu(Instant::now()).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 1);
    assert_eq!(service.pending_batches().len(), 0);
}

#[test]
fn c0_2ci_scratch_free_after_composite_error() {
    let (mut service, scratch_held, scratch_drops) = spy_service();
    let _scratch_key = scratch_held.key();

    // In Composite error before GPU submission, the scratch lease is dropped on error exit
    drop(scratch_held);
    service.service_ready();
    // Dropped without any pending GPU obligation destroys the scratch immediately
    assert_eq!(scratch_drops.get(), 1);
}

#[test]
fn c0_2ci_descriptor_reset_exclusion_until_gpu_signaled() {
    let (mut service, held, _drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    let ticket = crate::kms::render::platform::FenceTicket::for_tests_stub();
    let descriptor = vk::DescriptorSet::from_raw(42);
    let mut batch = CoreRetirementBatch::new(vec![held], vec![descriptor], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(vec![(key, gpu)], ticket));
    batch.test_ticket_status = Some(Ok(false));

    service.register_batch(batch);

    // Polling while unsignaled does not retire the batch, retaining descriptor slot 42
    service.poll_gpu(Instant::now()).unwrap();
    assert_eq!(service.pending_batches().len(), 1);
    assert_eq!(
        service.pending_batches()[0].descriptor_slots(),
        &[descriptor]
    );

    // When signaled, polling retires and releases descriptor slot ownership
    service.pending_batches_mut()[0].test_ticket_status = Some(Ok(true));
    service.poll_gpu(Instant::now()).unwrap();
    assert_eq!(service.pending_batches().len(), 0);
}

// F4-B1: real-fence variant, same rationale as the dropped-frame-metadata
// variant above -- proves the real `ticket_status()`/`poll_signaled_result`
// path retires a batch and releases its descriptor slot once a genuine
// submission genuinely signals, without racing the unsignaled window.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn c0_2ci_descriptor_reset_exclusion_until_gpu_signaled_vulkan() {
    let vk = real_vk_context();
    let ops_pool = crate::kms::vk::ops::OpsCommandPool::new(Arc::clone(&vk)).expect("ops pool");
    let fence_pool = crate::kms::render::platform::FencePool::new(Arc::clone(&vk));

    let (mut service, held, _drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    let ticket = fence_pool.acquire().expect("acquire real fence ticket");
    let descriptor = vk::DescriptorSet::from_raw(42);
    crate::kms::vk::ops::submit_one_shot_op_async(&vk, ops_pool.handle(), &ticket, |_vk, _cb| {
        Ok(())
    })
    .expect("submit real no-op");

    let mut batch = CoreRetirementBatch::new(vec![held], vec![descriptor], true);
    batch.bind_ticket(GpuObligation::new(
        vec![(key, gpu)],
        ticket.clone(),
        Arc::clone(&vk),
    ));
    service.register_batch(batch);

    ticket.wait(&vk).expect("wait for real ticket");
    service.poll_gpu(Instant::now()).unwrap();
    assert_eq!(service.pending_batches().len(), 0);
}

#[test]
fn c0_2ci_progress_no_composition() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    let ticket = crate::kms::render::platform::FenceTicket::for_tests_stub();
    let mut batch =
        CoreRetirementBatch::new(vec![held], vec![vk::DescriptorSet::from_raw(0)], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(vec![(key, gpu)], ticket));
    batch.test_ticket_status = Some(Ok(false));
    service.register_batch(batch);

    // Active seat with unsignaled ticket: schedules a future deadline (~1ms)
    assert!(service.next_deadline().is_some());

    // M-16: seat inactive (VT-away / DPMS-off) pauses the serviced-time
    // *budget*, not progress -- a real submission can still complete while
    // the display is dark, and the core loop must keep polling for it or it
    // is never observed until an unrelated fd happens to wake the loop. The
    // pre-fix behaviour returned `None` here (conflating "don't count this
    // time" with "don't look"); the deadline must still be scheduled.
    service.set_seat_active(false, Instant::now());
    assert!(
        service.next_deadline().is_some(),
        "a pending ticket must still be polled while the seat is inactive (M-16)"
    );

    // Seat returns active
    service.set_seat_active(true, Instant::now());
    assert!(service.next_deadline().is_some());

    // Signal the ticket
    service.pending_batches_mut()[0].test_ticket_status = Some(Ok(true));

    // Service completions runs outside composition, allocation completes
    let ready = service.service_completions(Instant::now()).unwrap();
    assert_eq!(ready, vec![key]);
    assert_eq!(drops.get(), 1);
    assert_eq!(service.pending_batches().len(), 0);

    // Failed ticket closes route without repeated immediate deadlines
    let (held2, _drops2) = {
        let drops = Rc::new(Cell::new(0));
        let held = service
            .adopt(AllocationPayload::Spy(SpyAllocation {
                drops: Rc::clone(&drops),
            }))
            .unwrap();
        (held, drops)
    };
    let mut batch2 =
        CoreRetirementBatch::new(vec![held2], vec![vk::DescriptorSet::from_raw(0)], true);
    batch2.bind_ticket(GpuObligation::for_tests_stub(
        vec![],
        crate::kms::render::platform::FenceTicket::for_tests_stub(),
    ));
    batch2.test_ticket_status = Some(Err(ash::vk::Result::ERROR_DEVICE_LOST));
    service.register_batch(batch2);

    let err = service.service_completions(Instant::now());
    assert_eq!(err, Err(ResourceError::Frozen));
    assert_eq!(service.quarantined_batches().len(), 1);
    assert_eq!(service.pending_batches().len(), 0);
    // Quarantined failed batch does not schedule repeat deadlines
    assert!(service.next_deadline().is_none());
}

fn unsignaled_batch(service: &mut ResourceService, held: AllocationLease) {
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();
    let ticket = crate::kms::render::platform::FenceTicket::for_tests_stub();
    let mut batch =
        CoreRetirementBatch::new(vec![held], vec![vk::DescriptorSet::from_raw(0)], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(vec![(key, gpu)], ticket));
    batch.test_ticket_status = Some(Ok(false));
    service.register_batch(batch);
}

#[test]
fn c0_2ci_serviced_time_pauses_during_seat_inactive_and_expires() {
    let (mut service, held, drops) = spy_service();

    let base = Instant::now();
    // Set the budget BEFORE registering (B-11: the deadline is stamped at
    // registration from whatever `max_serviced_duration` is then).
    service.max_serviced_duration = std::time::Duration::from_millis(50);
    unsignaled_batch(&mut service, held);

    // Seat goes inactive (VT-away) for 500ms
    service.set_seat_active(false, base);
    let _ = service.service_completions(base + std::time::Duration::from_millis(500));
    // Must NOT expire because serviced time paused
    assert_eq!(drops.get(), 0);
    assert_eq!(service.pending_batches().len(), 1);

    // Seat resumes
    service.set_seat_active(true, base + std::time::Duration::from_millis(500));
    // Advance serviced time by 60ms (> 50ms max_serviced_duration)
    let _ = service.service_completions(base + std::time::Duration::from_millis(530));
    let exp = service.service_completions(base + std::time::Duration::from_millis(570));
    assert_eq!(exp, Err(ResourceError::Frozen));
    assert_eq!(service.quarantined_batches().len(), 1);
    assert_eq!(service.pending_batches().len(), 0);
    // B-11: expiry never flips a service-wide `exhausted` -- new admission
    // must still work after this batch's own deadline passed.
    assert!(!service.is_exhausted());
}

/// B-11 decisive test: two batches registered at different points on the
/// serviced-time timeline expire independently -- each carries its OWN
/// deadline from its OWN registration, never a single global accumulator
/// compared against one shared threshold (which would expire every pending
/// batch in the service at once, regardless of when each actually
/// registered). Mutation check: replacing the per-batch deadline with the
/// pre-fix global `serviced_elapsed >= max_serviced_duration` check makes
/// batch 2 expire alongside batch 1 at t=55ms, failing this test.
#[test]
fn c0_2ci_serviced_deadline_is_per_batch_not_global() {
    let (mut service, held_a, drops_a) = spy_service();
    let drops_b = Rc::new(Cell::new(0));
    let held_b = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops_b),
        }))
        .unwrap();

    let base = Instant::now();
    service.max_serviced_duration = std::time::Duration::from_millis(50);

    // Batch 1 registers at serviced_elapsed = 0 -> deadline = 50ms.
    unsignaled_batch(&mut service, held_a);

    // Prime `last_serviced` at t=base (no prior sample to diff against yet,
    // so this call itself must not advance `serviced_elapsed`).
    let _ = service.service_completions(base);
    assert_eq!(service.pending_batches().len(), 1);

    // Advance serviced time by 40ms (batch 1 not yet expired: 40 < 50).
    let _ = service.service_completions(base + std::time::Duration::from_millis(40));
    assert_eq!(service.pending_batches().len(), 1);
    assert_eq!(service.quarantined_batches().len(), 0);

    // Batch 2 registers now, at serviced_elapsed = 40ms -> deadline = 90ms.
    unsignaled_batch(&mut service, held_b);
    assert_eq!(service.pending_batches().len(), 2);

    // Advance cumulative serviced time to 55ms: batch 1's 50ms deadline has
    // passed, batch 2's 90ms deadline has not.
    let result = service.service_completions(base + std::time::Duration::from_millis(55));
    assert_eq!(result, Err(ResourceError::Frozen));
    assert_eq!(
        service.pending_batches().len(),
        1,
        "only batch 2 should remain pending"
    );
    assert_eq!(
        service.quarantined_batches().len(),
        1,
        "only batch 1 should have expired"
    );
    assert_eq!(drops_a.get(), 0, "quarantine does not drop the allocation");
    assert_eq!(drops_b.get(), 0);
    assert!(
        !service.is_exhausted(),
        "one batch's expiry must never flip a service-wide exhausted flag"
    );

    // Advance to 95ms cumulative: batch 2's 90ms deadline has now passed.
    let result2 = service.service_completions(base + std::time::Duration::from_millis(95));
    assert_eq!(result2, Err(ResourceError::Frozen));
    assert_eq!(service.pending_batches().len(), 0);
    assert_eq!(service.quarantined_batches().len(), 2);
    assert!(!service.is_exhausted());
}

/// B-11 decisive test: a batch registered only after 5s of PRIOR service
/// (some other batch/servicing already consumed that serviced time) is not
/// expired on its first poll -- its deadline is relative to its OWN
/// registration, not to when the service started running. Mutation check:
/// comparing against the pre-fix `serviced_elapsed >= max_serviced_duration`
/// (a single global counter never reset per batch) would expire this batch
/// immediately, since `serviced_elapsed` is already past `max_serviced_duration`
/// by the time it registers.
#[test]
fn c0_2ci_serviced_deadline_not_expired_on_first_poll_after_prior_service() {
    let (mut service, held, drops) = spy_service();
    let base = Instant::now();

    // Run 5s of prior serviced time with nothing pending.
    let _ = service.service_completions(base);
    let _ = service.service_completions(base + std::time::Duration::from_secs(5));

    // Now register a batch; its deadline is 5s (elapsed so far) + 5s
    // (default max_serviced_duration) = 10s, not `max_serviced_duration`
    // measured from zero.
    unsignaled_batch(&mut service, held);

    // First poll, barely after registration: must not be expired.
    let result = service.service_completions(
        base + std::time::Duration::from_secs(5) + std::time::Duration::from_millis(1),
    );
    assert!(result.is_ok());
    assert_eq!(service.pending_batches().len(), 1);
    assert_eq!(service.quarantined_batches().len(), 0);
    assert_eq!(drops.get(), 0);
}

#[test]
fn c0_2ci_completion_waiter_registration_and_recheck() {
    let mut registry = WaiterRegistry::new();
    let generation = 42;

    registry.register(ResourceWaiter::new(generation, ResourceConsumer::Pool));
    registry.register(ResourceWaiter::new(
        generation,
        ResourceConsumer::DirectCapacity,
    ));
    // Duplicate registration is coalesced
    registry.register(ResourceWaiter::new(generation, ResourceConsumer::Pool));

    assert!(registry.is_registered(&ResourceWaiter::new(generation, ResourceConsumer::Pool)));
    assert!(registry.is_registered(&ResourceWaiter::new(
        generation,
        ResourceConsumer::DirectCapacity
    )));

    // On eligibility edge, notify_eligible enqueues wakes and clears registrations
    registry.notify_eligible(generation);

    assert!(!registry.is_registered(&ResourceWaiter::new(generation, ResourceConsumer::Pool)));
    assert!(!registry.is_registered(&ResourceWaiter::new(
        generation,
        ResourceConsumer::DirectCapacity
    )));

    let mut wakes = Vec::new();
    while let Some(w) = registry.pop_wake() {
        wakes.push(w);
    }
    assert_eq!(wakes.len(), 2);
    assert!(wakes.contains(&ResourceConsumer::Pool));
    assert!(wakes.contains(&ResourceConsumer::DirectCapacity));
}

/// M-14: a real-shaped `LegacyDrained` proof for `issue_handover_permit`,
/// matching `incarnation` the way the backend's genuine
/// `issue_legacy_drained` output would.
fn legacy_drained_for_tests(
    incarnation: IncarnationId,
) -> crate::kms::render::platform::LegacyDrained {
    crate::kms::render::platform::LegacyDrained {
        incarnation,
        lifecycle: LifecycleEpochId::first(),
    }
}

#[test]
fn c0_2ci_transport_gate_vocabulary_and_table() {
    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut gate = TransportGate::new_legacy(
        device,
        incarnation,
        Box::new(FakeDirectOwnershipState::new()),
    );

    // 1. In Legacy: all classes allowed
    for class in [
        WriterClass::Primary,
        WriterClass::Unflip,
        WriterClass::Modeset,
        WriterClass::Dpms,
        WriterClass::Vt,
        WriterClass::Topology,
        WriterClass::Cursor,
        WriterClass::Gamma,
        WriterClass::HelperMutation,
    ] {
        assert!(gate.allows_legacy(class));
    }

    // 2. In Quiescing: all classes false
    gate.begin_quiescing().unwrap();
    assert_eq!(gate.state(), TransportState::Quiescing);
    for class in [
        WriterClass::Primary,
        WriterClass::Unflip,
        WriterClass::Modeset,
        WriterClass::Dpms,
        WriterClass::Vt,
        WriterClass::Topology,
        WriterClass::Cursor,
        WriterClass::Gamma,
        WriterClass::HelperMutation,
    ] {
        assert!(!gate.allows_legacy(class));
    }

    // 3. In test-only Owner: all classes false
    let permit = gate
        .issue_handover_permit(
            legacy_drained_for_tests(incarnation),
            &[],
            &WriterCoverageProof::new_for_tests(),
            RecipientReservation::new_for_tests(),
        )
        .unwrap();
    gate.publish_owner(permit).unwrap();
    assert_eq!(gate.state(), TransportState::Owner);
    for class in [
        WriterClass::Primary,
        WriterClass::Unflip,
        WriterClass::Modeset,
        WriterClass::Dpms,
        WriterClass::Vt,
        WriterClass::Topology,
        WriterClass::Cursor,
        WriterClass::Gamma,
        WriterClass::HelperMutation,
    ] {
        assert!(!gate.allows_legacy(class));
    }

    // 4. In Closed: all classes false
    gate.close().unwrap();
    assert_eq!(gate.state(), TransportState::Closed);
    for class in [
        WriterClass::Primary,
        WriterClass::Unflip,
        WriterClass::Modeset,
        WriterClass::Dpms,
        WriterClass::Vt,
        WriterClass::Topology,
        WriterClass::Cursor,
        WriterClass::Gamma,
        WriterClass::HelperMutation,
    ] {
        assert!(!gate.allows_legacy(class));
    }

    // Closed cannot return to Legacy on same incarnation
    assert_eq!(gate.begin_quiescing(), Err(ResourceError::Detached));
}

/// M-13 decisive test: `begin_quiescing`'s Busy precondition reads the real
/// direct-ownership/unflip state through the `DirectOwnershipState` trait
/// supplied at construction, not a setter on the gate itself (there is no
/// `set_direct_scanout_active`/`set_unflip_pending` on `TransportGate` any
/// more). The fake records every query it was asked, so this test also
/// proves the gate consults live state on each call rather than a value
/// cached once. Mutation check: reverting to the pre-fix free-floating
/// setters would still pass the busy/unblocked assertions below by
/// construction, but the `busy_query_count`/`unflip_query_count`
/// assertions would fail (nothing on the gate would ever call them).
#[test]
fn c0_2ci_transport_gate_direct_scanout_precondition() {
    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let ownership = FakeDirectOwnershipState::new();
    let mut gate = TransportGate::new_legacy(device, incarnation, Box::new(ownership.clone()));

    // Active direct scanout blocks quiescing
    ownership.set_busy(true);
    assert_eq!(gate.begin_quiescing(), Err(ResourceError::Busy));
    assert_eq!(gate.state(), TransportState::Legacy);

    ownership.set_busy(false);
    // Pending unflip blocks quiescing
    ownership.set_unflip_outstanding(true);
    assert_eq!(gate.begin_quiescing(), Err(ResourceError::Busy));
    assert_eq!(gate.state(), TransportState::Legacy);

    // After unflip retires, begin_quiescing succeeds
    ownership.set_unflip_outstanding(false);
    assert!(gate.begin_quiescing().is_ok());
    assert_eq!(gate.state(), TransportState::Quiescing);

    // The gate asked the real state on every attempt (3 calls to
    // begin_quiescing above; `direct_ownership_busy` is queried every call,
    // `unflip_outstanding` on every call where busy was already false since
    // `||` short-circuits), never a cached value.
    assert!(ownership.busy_query_count() >= 3);
    assert!(ownership.unflip_query_count() >= 2);
}

#[test]
fn c0_2ci_transport_gate_owner_write_contract() {
    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let foreign_device = DrmDeviceKey {
        major: 226,
        minor: 1,
    };
    let incarnation = IncarnationId::first();
    let mut gate = TransportGate::new_legacy(
        device,
        incarnation,
        Box::new(FakeDirectOwnershipState::new()),
    );

    // Legacy cannot authorize owner write
    assert_eq!(
        gate.authorize_owner_write(WriterClass::Primary)
            .unwrap_err(),
        ResourceError::Detached
    );

    gate.begin_quiescing().unwrap();
    let permit = gate
        .issue_handover_permit(
            legacy_drained_for_tests(incarnation),
            &[],
            &WriterCoverageProof::new_for_tests(),
            RecipientReservation::new_for_tests(),
        )
        .unwrap();
    gate.publish_owner(permit).unwrap();

    // 1. Authorize grant of Primary class
    let grant1 = gate.authorize_owner_write(WriterClass::Primary).unwrap();
    assert_eq!(grant1.class(), WriterClass::Primary);
    assert_eq!(grant1.device(), device);
    assert_eq!(grant1.incarnation(), incarnation);
    assert_eq!(gate.outstanding_owner_writes(), 1);

    // Consuming consumes the grant and decrements count
    assert!(gate.consume_owner_write(grant1).is_ok());
    assert_eq!(gate.outstanding_owner_writes(), 0);

    // Reconstructed or replayed serial is rejected
    let replayed =
        OwnerWriteGrant::reconstruct_for_tests(device, incarnation, WriterClass::Primary, 1);
    let (err, returned_grant) = gate.consume_owner_write(replayed).unwrap_err();
    assert_eq!(err, ResourceError::InvalidProof);
    drop(returned_grant);

    // 2. Mismatched device closes transport
    let foreign_grant = OwnerWriteGrant::reconstruct_for_tests(
        foreign_device,
        incarnation,
        WriterClass::Primary,
        2,
    );
    let (err2, _) = gate.consume_owner_write(foreign_grant).unwrap_err();
    assert_eq!(err2, ResourceError::WrongIncarnation);
    assert_eq!(gate.state(), TransportState::Closed);

    // 3. Test dropped grant leaves charge and closes admission
    let mut gate2 = TransportGate::new_legacy(
        device,
        incarnation,
        Box::new(FakeDirectOwnershipState::new()),
    );
    gate2.begin_quiescing().unwrap();
    let permit2 = gate2
        .issue_handover_permit(
            legacy_drained_for_tests(incarnation),
            &[],
            &WriterCoverageProof::new_for_tests(),
            RecipientReservation::new_for_tests(),
        )
        .unwrap();
    gate2.publish_owner(permit2).unwrap();

    let grant_to_drop = gate2.authorize_owner_write(WriterClass::Cursor).unwrap();
    assert_eq!(gate2.outstanding_owner_writes(), 1);
    drop(grant_to_drop);

    // Dropped grant leaves charge
    assert_eq!(gate2.outstanding_owner_writes(), 1);
    // And closes admission: subsequent authorization returns Detached
    assert_eq!(
        gate2
            .authorize_owner_write(WriterClass::Cursor)
            .unwrap_err(),
        ResourceError::Detached
    );

    // M-14: outstanding writes block begin_quiescing AND close.
    assert_eq!(gate2.begin_quiescing(), Err(ResourceError::Busy));
    assert_eq!(gate2.close(), Err(ResourceError::Busy));
    assert_eq!(
        gate2.state(),
        TransportState::Owner,
        "a refused close must not change state"
    );

    // revoke_owner_writes clears the charge and restores admission
    let revoked = gate2.revoke_owner_writes();
    assert_eq!(revoked, 1);
    assert_eq!(gate2.outstanding_owner_writes(), 0);
    let grant3 = gate2.authorize_owner_write(WriterClass::Cursor).unwrap();
    // Consuming it (rather than dropping it) leaves nothing outstanding, so
    // close() succeeds.
    gate2.consume_owner_write(grant3).unwrap();
    assert_eq!(gate2.outstanding_owner_writes(), 0);
    assert!(gate2.close().is_ok());
    assert_eq!(gate2.state(), TransportState::Closed);
}

/// Minor (round-1 review) decisive test: `consume_owner_write`'s
/// bookkeeping used `if outstanding_owner_writes > 0 { -= 1 }`, which
/// silently does nothing -- instead of surfacing a bug -- when a serial is
/// legitimately consumed but the counter has already desynced from
/// `issued_serials`. That desync cannot happen through the normal
/// `authorize_owner_write`/`consume_owner_write` pairing, so this test
/// forces it with the test-only `set_outstanding_owner_writes_for_tests`
/// backdoor. Mutation check: reverting to the pre-fix
/// `if outstanding_owner_writes > 0 { -= 1 }` makes this test fail (it
/// would return `Ok(())` instead of `Err((InvalidState, _))`).
#[test]
fn c0_2ci_transport_gate_consume_owner_write_checked_subtraction() {
    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut gate = TransportGate::new_legacy(
        device,
        incarnation,
        Box::new(FakeDirectOwnershipState::new()),
    );
    gate.begin_quiescing().unwrap();
    let permit = gate
        .issue_handover_permit(
            legacy_drained_for_tests(incarnation),
            &[],
            &WriterCoverageProof::new_for_tests(),
            RecipientReservation::new_for_tests(),
        )
        .unwrap();
    gate.publish_owner(permit).unwrap();

    let grant = gate.authorize_owner_write(WriterClass::Primary).unwrap();
    // Force the desync: the serial above is legitimately issued and about
    // to be legitimately consumed, but the counter is already at 0.
    gate.set_outstanding_owner_writes_for_tests(0);

    let (err, returned_grant) = gate.consume_owner_write(grant).unwrap_err();
    assert_eq!(err, ResourceError::InvalidState);
    drop(returned_grant);
    // The counter never wrapped around to `usize::MAX`.
    assert_eq!(gate.outstanding_owner_writes(), 0);
}

/// M-14 decisive test: `close()` refuses while a grant is outstanding and
/// succeeds once the charge is actually resolved (consumed or revoked).
/// Mutation check: reverting `close()` to unconditionally set `Closed`
/// (its pre-fix shape) makes the first assertion below fail.
#[test]
fn c0_2ci_transport_gate_close_refuses_outstanding_grants() {
    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut gate = TransportGate::new_legacy(
        device,
        incarnation,
        Box::new(FakeDirectOwnershipState::new()),
    );
    gate.begin_quiescing().unwrap();
    let permit = gate
        .issue_handover_permit(
            legacy_drained_for_tests(incarnation),
            &[],
            &WriterCoverageProof::new_for_tests(),
            RecipientReservation::new_for_tests(),
        )
        .unwrap();
    gate.publish_owner(permit).unwrap();

    let grant = gate.authorize_owner_write(WriterClass::Primary).unwrap();
    assert_eq!(gate.outstanding_owner_writes(), 1);

    // Refuses while the grant is outstanding, and does not change state.
    assert_eq!(gate.close(), Err(ResourceError::Busy));
    assert_eq!(gate.state(), TransportState::Owner);

    // Consuming the grant clears the charge; close now succeeds.
    gate.consume_owner_write(grant).unwrap();
    assert_eq!(gate.outstanding_owner_writes(), 0);
    assert!(gate.close().is_ok());
    assert_eq!(gate.state(), TransportState::Closed);
}

/// M-14 decisive test: `issue_handover_permit` refuses a `LegacyDrained`
/// proof for a foreign incarnation, and refuses when the final drain
/// dispositions include a backend failure -- it no longer accepts whatever
/// the caller claims (R9: proofs are never fabricated). Mutation check:
/// dropping either check makes the corresponding assertion below fail.
#[test]
fn c0_2ci_transport_gate_handover_validates_proof_and_dispositions() {
    use crate::kms::render::backend::{LegacyEventCancellation, LegacyEventDisposition};

    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let foreign_incarnation = IncarnationId::from_raw(incarnation.get() + 1);

    let mut gate = TransportGate::new_legacy(
        device,
        incarnation,
        Box::new(FakeDirectOwnershipState::new()),
    );
    gate.begin_quiescing().unwrap();

    // Foreign incarnation's proof is refused.
    let err = gate
        .issue_handover_permit(
            legacy_drained_for_tests(foreign_incarnation),
            &[],
            &WriterCoverageProof::new_for_tests(),
            RecipientReservation::new_for_tests(),
        )
        .unwrap_err();
    assert_eq!(err, ResourceError::WrongIncarnation);
    assert_eq!(
        gate.state(),
        TransportState::Quiescing,
        "a refused permit must not change state"
    );

    // A backend failure among the final dispositions is refused too.
    let err2 = gate
        .issue_handover_permit(
            legacy_drained_for_tests(incarnation),
            &[LegacyEventDisposition::Cancelled(
                LegacyEventCancellation::BackendFailure,
            )],
            &WriterCoverageProof::new_for_tests(),
            RecipientReservation::new_for_tests(),
        )
        .unwrap_err();
    assert_eq!(err2, ResourceError::InvalidProof);

    // The real proof with only non-failure dispositions succeeds.
    let permit = gate
        .issue_handover_permit(
            legacy_drained_for_tests(incarnation),
            &[LegacyEventDisposition::Applied],
            &WriterCoverageProof::new_for_tests(),
            RecipientReservation::new_for_tests(),
        )
        .unwrap();
    assert!(gate.publish_owner(permit).is_ok());
}

// B-10/R11 (F-5b): `c0_2ci_transport_gate_writer_boundary_enforcement` stood
// here -- it drove no entry point, asserted `allows_legacy` directly, and
// its device-B "unrelated device unaffected" claim proved nothing about any
// real sink. Deleted per the fix handoff; the real per-sink tests
// (`c0_2ci_sink_*`) beneath the real entry points replace it.

/// B-10/R11: a `TransportGate` for `device`/`incarnation`, driven all the
/// way to `Owner` (Legacy -> Quiescing -> handover -> publish), for tests
/// that need to mint a real `OwnerWriteGrant` matching a specific device/
/// incarnation identity.
fn owner_gate_for_tests(device: DrmDeviceKey, incarnation: IncarnationId) -> TransportGate {
    let mut gate = TransportGate::new_legacy(
        device,
        incarnation,
        Box::new(FakeDirectOwnershipState::new()),
    );
    gate.begin_quiescing().unwrap();
    let permit = gate
        .issue_handover_permit(
            legacy_drained_for_tests(incarnation),
            &[],
            &WriterCoverageProof::new_for_tests(),
            RecipientReservation::new_for_tests(),
        )
        .unwrap();
    gate.publish_owner(permit).unwrap();
    gate
}

/// B-10/R11: a `TransportGate` at `target`, independent of any
/// `PlatformBackend`. The five `crate::drm::{page_flip,modeset}` sinks
/// below take a plain `legacy_write_permitted: bool` rather than the gate
/// itself (R8: no production issuer of `OwnerWriteGrant` exists for these
/// classes in this stage -- only the executor's "helper mutation" sink,
/// tested separately, actually consumes a grant), so the gate's own
/// device/incarnation identity never has to match anything; only
/// `allows_legacy(class)` -- which is state-only -- is read from it.
fn sink_gate_at_state(target: TransportState) -> TransportGate {
    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut gate = TransportGate::new_legacy(
        device,
        incarnation,
        Box::new(FakeDirectOwnershipState::new()),
    );
    if target == TransportState::Legacy {
        return gate;
    }
    gate.begin_quiescing().unwrap();
    if target == TransportState::Quiescing {
        return gate;
    }
    if target == TransportState::Owner {
        let permit = gate
            .issue_handover_permit(
                legacy_drained_for_tests(incarnation),
                &[],
                &WriterCoverageProof::new_for_tests(),
                RecipientReservation::new_for_tests(),
            )
            .unwrap();
        gate.publish_owner(permit).unwrap();
        return gate;
    }
    debug_assert_eq!(target, TransportState::Closed);
    gate.close().unwrap();
    gate
}

/// B-10/R11, 6.5a/6.5b: drives `sink` (a real `crate::drm` entry point,
/// wrapped so every call site here shares one shape) through all four gate
/// states and asserts the ioctl was reached (real device fd error,
/// `raw_os_error().is_some()`, since `PlatformBackend::for_tests()`'s
/// device is a `UnixStream`, not a DRM node) iff the state is `Legacy`.
/// Mutation check (performed for `disable_output`, representative of all
/// five): deleting the sink's `if !legacy_write_permitted { return ... }`
/// makes the non-Legacy assertions fail (`raw_os_error()` becomes `Some`
/// there too, since the call falls through to the real ioctl).
fn assert_sink_gated_four_states(
    class: WriterClass,
    mut sink: impl FnMut(bool) -> std::io::Result<()>,
) {
    for state in [
        TransportState::Legacy,
        TransportState::Quiescing,
        TransportState::Owner,
        TransportState::Closed,
    ] {
        let gate = sink_gate_at_state(state);
        let permitted = gate.allows_legacy(class);
        let err = sink(permitted).expect_err("Device::for_tests() never succeeds a real commit");
        let reached_ioctl = err.raw_os_error().is_some();
        assert_eq!(
            reached_ioctl,
            state == TransportState::Legacy,
            "state={state:?} class={class:?} permitted={permitted} err={err}",
        );
    }
}

#[test]
fn c0_2ci_sink_legacy_page_flip_gate_four_states() {
    let platform = crate::kms::render::platform::PlatformBackend::for_tests();
    let device = Rc::clone(&platform.devices[0].device);
    let output = &platform.outputs[0].output;
    let fb = ::drm::control::from_u32(1).unwrap();
    assert_sink_gated_four_states(WriterClass::Primary, |permitted| {
        let mut out_fence = -1;
        crate::drm::page_flip::submit_flip_with_fences(
            &device,
            output,
            fb,
            -1,
            &mut out_fence,
            permitted,
        )
    });
}

#[test]
fn c0_2ci_sink_direct_atomic_flip_gate_four_states() {
    let platform = crate::kms::render::platform::PlatformBackend::for_tests();
    let device = Rc::clone(&platform.devices[0].device);
    let output = &platform.outputs[0].output;
    let fb = ::drm::control::from_u32(1).unwrap();
    let plane_states = [crate::drm::modeset::DirectScanoutPlaneState {
        output,
        src_x: 0,
        src_y: 0,
        src_w: 800,
        src_h: 600,
    }];
    assert_sink_gated_four_states(WriterClass::Primary, |permitted| {
        crate::drm::modeset::submit_direct_scanout(&device, fb, &plane_states, permitted)
    });
}

#[test]
fn c0_2ci_sink_composed_unflip_gate_four_states() {
    let platform = crate::kms::render::platform::PlatformBackend::for_tests();
    let device = Rc::clone(&platform.devices[0].device);
    let output = &platform.outputs[0].output;
    let fb = ::drm::control::from_u32(1).unwrap();
    let planes = [crate::drm::modeset::ComposedScanoutPlaneState { output, fb }];
    assert_sink_gated_four_states(WriterClass::Unflip, |permitted| {
        crate::drm::modeset::submit_composed_scanout(&device, &planes, permitted)
    });
}

#[test]
fn c0_2ci_sink_modeset_install_gate_four_states() {
    let platform = crate::kms::render::platform::PlatformBackend::for_tests();
    let device = Rc::clone(&platform.devices[0].device);
    let output = &platform.outputs[0].output;
    let fb = ::drm::control::from_u32(1).unwrap();
    assert_sink_gated_four_states(WriterClass::Modeset, |permitted| {
        crate::drm::modeset::commit_modeset(&device, output, fb, permitted)
    });
}

#[test]
fn c0_2ci_sink_output_disable_gate_four_states() {
    let platform = crate::kms::render::platform::PlatformBackend::for_tests();
    let device = Rc::clone(&platform.devices[0].device);
    let output = &platform.outputs[0].output;
    assert_sink_gated_four_states(WriterClass::Modeset, |permitted| {
        crate::drm::modeset::disable_output(&device, output, permitted)
    });
}

/// B-10/R11 (helper mutation): unlike the five DRM sinks above, this sink
/// is the one this stage actually wires an `OwnerWriteGrant` through --
/// `KmsIoExecutor::send_authorized`, called via the private
/// `pub(crate)` wrapper because its signature names the `pub(crate)`
/// `TransportGate`/`OwnerWriteGrant` types (the public `send`/
/// `dispatch_blocking_at_boundary` always pass `None`, so no production or
/// external caller changed shape). Drives the real function four ways
/// through a real spawned helper subprocess, observing "was the request
/// actually put on the wire" via `poll_reply()` (a refused
/// `send_authorized` never touches `in_flight`, so `poll_reply()` returns
/// `None` immediately -- see `KmsIoExecutor::poll_reply`, line ~865: `let
/// in_flight = self.in_flight.as_ref()?;`) versus a real accepted outcome
/// from the helper. Mutation checks: (1) deleting the `authorize_write`
/// call in `send_authorized` makes the Quiescing case actually dispatch
/// (`poll_reply()` stops returning `None`); (2) making
/// `consume_owner_write` not consume (e.g. skip `grant.consumed.set(true)`)
/// makes the Owner-with-matching-grant case's
/// `outstanding_owner_writes() == 0` assertion fail.
#[test]
fn c0_2ci_sink_helper_mutation_gate_four_way() {
    use crate::kms::executor::{
        HostCallReservation, SendError, SubmittingProof,
        test_support::{self, ScriptedReply},
    };

    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let request = test_support::small_atomic_request_for_tests();
    let reservation = || HostCallReservation::Submitting(SubmittingProof::for_tests());

    // 1. Legacy: `owner_write = None` (what every real call site passes,
    // R8) -- permitted, the real helper subprocess actually processes it.
    let mut executor =
        test_support::spawn_scripted_helper_for_tests(ScriptedReply::Accepted { mask: 0, fds: 0 });
    executor
        .send_authorized(&request, reservation(), None)
        .expect("Legacy (no gate) permits");
    test_support::wait_readable(
        executor.control_fd().expect("fd"),
        std::time::Duration::from_secs(30),
    );
    match executor.poll_reply().expect("the real helper replied") {
        crate::kms::executor::HostCallEvent::Outcome { outcome, .. } => {
            assert!(
                matches!(
                    outcome,
                    crate::kms::executor::HostCallOutcome::Accepted { .. }
                ),
                "expected the real helper to have processed the request: {outcome:?}"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }

    // 2. Quiescing: refused before the wire -- the helper never sees it.
    let mut executor =
        test_support::spawn_scripted_helper_for_tests(ScriptedReply::Accepted { mask: 0, fds: 0 });
    let mut gate = TransportGate::new_legacy(
        device,
        incarnation,
        Box::new(FakeDirectOwnershipState::new()),
    );
    gate.begin_quiescing().unwrap();
    let dummy_grant =
        OwnerWriteGrant::reconstruct_for_tests(device, incarnation, WriterClass::HelperMutation, 1);
    let err = executor
        .send_authorized(&request, reservation(), Some((&mut gate, dummy_grant)))
        .unwrap_err();
    assert_eq!(err, SendError::TransportGateRefused);
    assert!(
        executor.poll_reply().is_none(),
        "a refused send must never have touched `in_flight`"
    );

    // 3. Owner + matching grant: permitted, and the grant is consumed at
    // this exact send boundary (R7).
    let mut executor =
        test_support::spawn_scripted_helper_for_tests(ScriptedReply::Accepted { mask: 0, fds: 0 });
    let mut gate = owner_gate_for_tests(device, incarnation);
    let grant = gate
        .authorize_owner_write(WriterClass::HelperMutation)
        .unwrap();
    assert_eq!(gate.outstanding_owner_writes(), 1);
    executor
        .send_authorized(&request, reservation(), Some((&mut gate, grant)))
        .expect("Owner + matching grant permits");
    assert_eq!(
        gate.outstanding_owner_writes(),
        0,
        "the grant must be consumed at send, not merely authorized"
    );
    test_support::wait_readable(
        executor.control_fd().expect("fd"),
        std::time::Duration::from_secs(30),
    );
    match executor.poll_reply().expect("the real helper replied") {
        crate::kms::executor::HostCallEvent::Outcome { outcome, .. } => {
            assert!(
                matches!(
                    outcome,
                    crate::kms::executor::HostCallOutcome::Accepted { .. }
                ),
                "expected the real helper to have processed the request: {outcome:?}"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }

    // 4. Owner + a grant of the wrong class ("without [a valid] grant" for
    // this class): refused, not consumed -- and per the lost-role-token
    // rule, dropping the unconsumed grant closes admission.
    let mut executor =
        test_support::spawn_scripted_helper_for_tests(ScriptedReply::Accepted { mask: 0, fds: 0 });
    let mut gate = owner_gate_for_tests(device, incarnation);
    let wrong_class_grant = gate.authorize_owner_write(WriterClass::Primary).unwrap();
    let err = executor
        .send_authorized(
            &request,
            reservation(),
            Some((&mut gate, wrong_class_grant)),
        )
        .unwrap_err();
    assert_eq!(err, SendError::TransportGateRefused);
    assert!(
        executor.poll_reply().is_none(),
        "a refused send must never have touched `in_flight`"
    );
    assert_eq!(
        gate.authorize_owner_write(WriterClass::HelperMutation)
            .unwrap_err(),
        ResourceError::Detached,
        "the dropped, unconsumed grant must have closed admission"
    );
}

struct TestFenceQuery<F>(F);
impl<
    F: FnMut(std::os::fd::BorrowedFd<'_>) -> std::io::Result<crate::kms::owner::fences::FenceStatus>,
> crate::kms::owner::fences::FenceQuery for TestFenceQuery<F>
{
    fn status(
        &mut self,
        fd: std::os::fd::BorrowedFd<'_>,
    ) -> std::io::Result<crate::kms::owner::fences::FenceStatus> {
        (self.0)(fd)
    }
}

struct DummyPollSet;
impl crate::kms::owner::fences::FencePollSet for DummyPollSet {
    fn register(&mut self, _fd: std::os::fd::BorrowedFd<'_>, _token: u64) -> std::io::Result<()> {
        Ok(())
    }
    fn unregister(&mut self, _fd: std::os::fd::BorrowedFd<'_>) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn c0_2ci_commit_owner_integration_with_actual_leases() {
    use crate::kms::owner::{
        device::DeviceCommitOwner, fences::FenceStatus, identity::CommitId,
        lifecycle::LifecycleEpochId, test_fixtures::*,
    };
    use std::{os::fd::BorrowedFd, time::Instant};

    let dummy_crtc = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(10).unwrap()),
    );
    let member = GroupMember::new(dummy_crtc, 1, 1);
    assert!(GroupMember::validate_unique(&[member]));

    // Part A: Deterministic by-value consumer test
    let (mut service, old_lease, old_drops) = spy_service();
    let old_key = old_lease.key();
    let new_drops = Rc::new(Cell::new(0));
    let new_lease = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&new_drops),
        }))
        .unwrap();
    let _new_key = new_lease.key();

    let old_kms = service
        .register(old_key, ObligationKind::KmsRelease)
        .unwrap();
    let _old_gpu = service.register(old_key, ObligationKind::Gpu).unwrap();

    let commit_id = CommitId::for_tests(1);
    let mut consumer = CommitResourceConsumer::new();

    let old = CommitResources::new(
        vec![old_lease],
        None,
        None,
        None,
        vec![member],
        vec![(old_key, old_kms, member)],
    );
    let new = CommitResources::new(vec![new_lease], None, None, None, vec![member], Vec::new());

    let accepted = crate::kms::owner::ledger::Submitted::new(vec![old], vec![new]).accepted();
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: commit_id,
                resources: accepted,
            },
            &mut service,
        )
        .unwrap();

    assert_eq!(old_drops.get(), 0);
    assert_eq!(new_drops.get(), 0);

    // Part B: Owner integration with DeviceCommitOwner<CommitResources>
    // Test both supported orders of page flip vs fence evidence
    for order in [0, 1] {
        let (mut service, old_lease, old_drops) = spy_service();
        let old_key = old_lease.key();
        let new_drops = Rc::new(Cell::new(0));
        let new_lease = service
            .adopt(AllocationPayload::Spy(SpyAllocation {
                drops: Rc::clone(&new_drops),
            }))
            .unwrap();

        let old_kms = service
            .register(old_key, ObligationKind::KmsRelease)
            .unwrap();
        let mut owner = DeviceCommitOwner::<CommitResources>::new(
            IncarnationId::first(),
            LifecycleEpochId::first(),
            1,
        );
        let clock_key = crate::kms::owner::clock::ClockKey {
            hardware_crtc: 1,
            epoch: crate::kms::owner::identity::ClockEpochId::first(),
        };
        owner
            .install_clock(clock_key, LifecycleEpochId::first(), 1)
            .unwrap();
        owner.clock_mut(clock_key).unwrap().install_reference(100);
        let context = fast_context_for_crtcs(&[(1, clock_key)]);
        let desc = single_active_crtc_with_present(1);
        let member = GroupMember::new(dummy_crtc, 1, 1);
        let old_res = CommitResources::new(
            vec![old_lease],
            None,
            None,
            None,
            vec![member],
            vec![(old_key, old_kms, member)],
        );
        let new_res =
            CommitResources::new(vec![new_lease], None, None, None, vec![member], Vec::new());
        let ledger = crate::kms::owner::ledger::Submitted::new(vec![old_res], vec![new_res]);
        let (commit, _) = owner.begin_with_context(&desc, ledger, context).unwrap();
        owner.mark_dispatched_for_tests();

        let page_event = event_for_current_record(&owner, 1, 100, 1, 0);

        let (r, w) = nix::unistd::pipe().expect("pipe");
        let event = crate::kms::executor::HostCallEvent::Outcome {
            correlation: owner_correlation(commit),
            outcome: crate::kms::executor::HostCallOutcome::Accepted {
                helper_duration_ns: 0,
                round_trip_ns: 0,
                out_fence_mask: 0b1,
                out_fences: vec![w],
            },
        };
        owner.apply_host_call_event(event);

        let mut consumer = CommitResourceConsumer::new();

        if order == 0 {
            // HardwareComplete before Presented
            let mut query = TestFenceQuery(|_fd: BorrowedFd<'_>| Ok(FenceStatus::Success));
            let mut poll_set = DummyPollSet;
            let hw_events = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
            for ev in hw_events {
                consumer.consume(ev, &mut service).unwrap();
            }
            assert_eq!(old_drops.get(), 0);
            assert_eq!(new_drops.get(), 0);

            let flip_events =
                owner.apply_drm_event(IncarnationId::first(), page_event, Instant::now());
            for ev in flip_events {
                consumer.consume(ev, &mut service).unwrap();
            }
            assert_eq!(old_drops.get(), 0);
            assert_eq!(new_drops.get(), 0);
        } else {
            // Presented before HardwareComplete
            let flip_events =
                owner.apply_drm_event(IncarnationId::first(), page_event, Instant::now());
            for ev in flip_events {
                consumer.consume(ev, &mut service).unwrap();
            }
            assert_eq!(old_drops.get(), 0);
            assert_eq!(new_drops.get(), 0);

            let mut query = TestFenceQuery(|_fd: BorrowedFd<'_>| Ok(FenceStatus::Success));
            let mut poll_set = DummyPollSet;
            let hw_events = owner.observe_fences(&mut query, &mut poll_set, Instant::now());
            for ev in hw_events {
                consumer.consume(ev, &mut service).unwrap();
            }
            assert_eq!(old_drops.get(), 0);
            assert_eq!(new_drops.get(), 0);
        }
        drop(r);
    }
}

#[test]
fn c0_2ci_commit_hardware_complete_discharges_old_only() {
    use crate::kms::owner::identity::CommitId;

    let (mut service, old_a, drops_old_a) = spy_service();
    let old_a_key = old_a.key();

    let drops_old_b = Rc::new(Cell::new(0));
    let old_b = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops_old_b),
        }))
        .unwrap();
    let old_b_key = old_b.key();

    let drops_new_a = Rc::new(Cell::new(0));
    let new_a = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops_new_a),
        }))
        .unwrap();
    let new_a_key = new_a.key();

    let crtc1 = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(10).unwrap()),
    );
    let crtc2 = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(20).unwrap()),
    );
    let member1 = GroupMember::new(crtc1, 1, 1);
    let member2 = GroupMember::new(crtc2, 1, 1);

    let commit_id = CommitId::for_tests(42);

    // Register KMS release obligations on old displaced buffers and on new buffer
    let old_a_kms = service.register_kms(old_a_key, commit_id, member1).unwrap();
    let old_b_kms = service.register_kms(old_b_key, commit_id, member2).unwrap();
    let new_a_kms = service.register_kms(new_a_key, commit_id, member1).unwrap();

    let old_res_a = CommitResources::new(
        vec![old_a],
        None,
        None,
        None,
        vec![member1],
        vec![(old_a_key, old_a_kms, member1)],
    );
    let old_res_b = CommitResources::new(
        vec![old_b],
        None,
        None,
        None,
        vec![member2],
        vec![(old_b_key, old_b_kms, member2)],
    );
    // new commit only updates member1 (partial replacement):
    let new_res_a = CommitResources::new(
        vec![new_a],
        None,
        None,
        None,
        vec![member1],
        vec![(new_a_key, new_a_kms, member1)],
    );

    let mut consumer = CommitResourceConsumer::new();

    // Feed through CompletionRetired
    let accepted =
        crate::kms::owner::ledger::Submitted::new(vec![old_res_a, old_res_b], vec![new_res_a])
            .accepted();

    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: commit_id,
                resources: accepted,
            },
            &mut service,
        )
        .unwrap();

    // Before HardwareComplete, all obligations are still pending
    assert!(service.has_pending_obligations(&old_a_key));
    assert!(service.has_pending_obligations(&old_b_key));
    assert!(service.has_pending_obligations(&new_a_key));

    // The ONLY KMS proof reaches the service via the owner's HardwareComplete event.
    // The test body calls NO apply_validated_proof!
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: commit_id },
            &mut service,
        )
        .unwrap();

    // 1. Matching old_a (member1) had its KMS obligation discharged!
    assert!(!service.has_pending_obligations(&old_a_key));

    // 2. Non-matching old_b (member2) was NOT discharged: partial grouped replacement discharges ONLY matching GroupMember!
    assert!(service.has_pending_obligations(&old_b_key));

    // 3. New set (new_a) was NEVER discharged by this commit's HardwareComplete!
    assert!(service.has_pending_obligations(&new_a_key));

    // Run on_available on old_a_key: since its obligation was discharged and it has no other holds,
    // on_available frees old_a from releasing_resources!
    consumer.on_available(&[old_a_key], &mut service).unwrap();
    service.service_ready();
    assert_eq!(drops_old_a.get(), 1); // Discharged and freed!

    // old_b and new_a still held
    consumer.on_available(&[old_b_key], &mut service).unwrap();
    service.service_ready();
    assert_eq!(drops_old_b.get(), 0);
    assert_eq!(drops_new_a.get(), 0);

    // Cancel remaining obligations and verify KmsDisposition after cancel is NOT Discharged
    service.cancel(old_b_key, old_b_kms).unwrap();
    assert_ne!(
        service.kms_disposition(old_b_key, old_b_kms),
        Some(crate::kms::render::resources::handoff::KmsDisposition::Discharged)
    );

    service.cancel(new_a_key, new_a_kms).unwrap();
    assert_ne!(
        service.kms_disposition(new_a_key, new_a_kms),
        Some(crate::kms::render::resources::handoff::KmsDisposition::Discharged)
    );

    // Drop consumer so remaining leases are released
    drop(consumer);
    service.service_ready();
    assert_eq!(drops_old_b.get(), 1);
    assert_eq!(drops_new_a.get(), 1);
}

#[test]
fn c0_2ci_commit_resources_still_current_cancels_not_discharges() {
    use crate::kms::owner::identity::CommitId;

    let (mut service, old, drops_old) = spy_service();
    let old_key = old.key();

    let crtc1 = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(10).unwrap()),
    );
    let member1 = GroupMember::new(crtc1, 1, 1);
    let commit_id = CommitId::for_tests(7);
    let old_kms = service.register_kms(old_key, commit_id, member1).unwrap();

    let mut consumer = CommitResourceConsumer::new();

    let old_res = CommitResources::new(
        vec![old],
        None,
        None,
        None,
        vec![member1],
        vec![(old_key, old_kms, member1)],
    );

    // Rejection: displacement never occurred!
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::ResourcesStillCurrent {
                commit: commit_id,
                resources: vec![old_res],
            },
            &mut service,
        )
        .unwrap();

    // Obligation was CANCELLED, not discharged with proof.
    // The consumer restored old_res into current_resources.
    assert_eq!(consumer.current_resources.len(), 1);
    assert_eq!(drops_old.get(), 0);
    assert_ne!(
        service.kms_disposition(old_key, old_kms),
        Some(crate::kms::render::resources::handoff::KmsDisposition::Discharged)
    );

    // Dropping current_resources drops old lease; since obligation was cancelled, it frees immediately.
    consumer.current_resources.clear();
    service.service_ready();
    assert_eq!(drops_old.get(), 1);
}

#[test]
fn c0_2ci_commit_topology_replacement_reused_numeric_crtc() {
    use crate::kms::owner::identity::CommitId;

    let (mut service, old, drops_old) = spy_service();
    let old_key = old.key();

    let crtc1 = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(10).unwrap()),
    );
    // Generation 1 member
    let old_member = GroupMember::new(crtc1, 1, 1);
    // Generation 2 member reuses same numeric CRTC handle, but generation is 2!
    let new_member = GroupMember::new(crtc1, 2, 1);
    assert_ne!(old_member, new_member);

    let commit_id = CommitId::for_tests(99);
    let old_kms = service
        .register_kms(old_key, commit_id, old_member)
        .unwrap();

    let mut consumer = CommitResourceConsumer::new();

    let old_res = CommitResources::new(
        vec![old],
        None,
        None,
        None,
        vec![old_member],
        vec![(old_key, old_kms, old_member)],
    );
    // Commit membership is new_member (generation 2)
    let new_res = CommitResources::new(vec![], None, None, None, vec![new_member], vec![]);
    let accepted =
        crate::kms::owner::ledger::Submitted::new(vec![old_res], vec![new_res]).accepted();

    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: commit_id,
                resources: accepted,
            },
            &mut service,
        )
        .unwrap();

    // HardwareComplete arrives for commit (which has membership [new_member])
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: commit_id },
            &mut service,
        )
        .unwrap();

    // Because new_member != old_member, the obligation for old_member is NOT discharged!
    assert!(service.has_pending_obligations(&old_key));
    consumer.on_available(&[old_key], &mut service).unwrap();
    service.service_ready();
    assert_eq!(drops_old.get(), 0); // Stale evidence cannot discharge the old record!

    // Even if consumer is dropped, the pending obligation keeps it alive!
    drop(consumer);
    service.service_ready();
    assert_eq!(drops_old.get(), 0);

    // Cleaning up
    service.cancel(old_key, old_kms).unwrap();
    service.service_ready();
    assert_eq!(drops_old.get(), 1);
}

#[test]
fn c0_2ci_present_release_consumption_and_completion_suppression() {
    use crate::kms::{
        owner::{
            device::OwnerEvent,
            identity::{CommitId, IncarnationId},
            record::TerminalState,
        },
        render::{
            platform::CrtcKey,
            present_completion::PinnedWake,
            resources::{
                commit::{CommitResourceConsumer, CommitResources, GroupMember, PresentRelease},
                present::{
                    CompletionDisposition, PresentDisposition, PresentKey, ReleaseDisposition,
                },
            },
        },
    };
    use yserver_core::backend::{CompletedPresentEvent, PresentWake};

    let (mut service, old_alloc, _drops_old) = spy_service();
    let old_key = old_alloc.key();

    let drops_new = Rc::new(Cell::new(0));
    let new_alloc = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops_new),
        }))
        .unwrap();

    let crtc = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(1).unwrap()),
    );
    let member = GroupMember::new(crtc, 1, 1);
    let commit_1 = CommitId::for_tests(10);
    let device_key = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::from_raw(1);

    let event1 = CompletedPresentEvent {
        client_id: yserver_protocol::x11::ClientId(1),
        serial: 1,
        host_xid: 0x100,
        dst_host_xid: 0x200,
        options: 0,
        present_id: 101,
        window_generation: 0,
        crtc_id: 1,
        crtc_epoch: 1,
        msc_offset: 0,
        completion_clock: None,
        wake: PresentWake::Pixmap { idle_fence_xid: 0 },
        completion_mode: 0,
        emit_idle: true,
    };
    let release1 = PresentRelease::new(event1, Some(PinnedWake::None));
    let present_key1 = PresentKey::new(device_key, incarnation, commit_1, 101);

    let mut consumer = CommitResourceConsumer::new();
    consumer.record_present_disposition(present_key1, PresentDisposition::pending());

    let old_res = CommitResources::new(
        vec![old_alloc],
        None,
        None,
        Some(release1),
        vec![member],
        vec![],
    );
    let new_res = CommitResources::new(vec![new_alloc], None, None, None, vec![member], vec![]);

    let submitted = crate::kms::owner::ledger::Submitted::new(vec![old_res], vec![new_res]);
    consumer
        .consume(
            OwnerEvent::CompletionRetired {
                commit: commit_1,
                resources: submitted.accepted(),
            },
            &mut service,
        )
        .unwrap();

    // 1. Presented: marks completion Emitted, but keeps release Retained
    consumer
        .consume(
            OwnerEvent::Presented {
                commit: commit_1,
                samples: std::collections::BTreeMap::new(),
            },
            &mut service,
        )
        .unwrap();

    let disp1 = consumer
        .present_disposition(&present_key1)
        .expect("disposition exists");
    assert_eq!(disp1.completion, CompletionDisposition::Emitted);
    assert_eq!(disp1.release, ReleaseDisposition::Retained);
    assert!(
        consumer.take_released_presents().is_empty(),
        "Presented must not release wake or source"
    );

    // 2. Terminal { FailedBeforeSubmit }: suppresses completion, release remains Retained
    let commit_2 = CommitId::for_tests(20);
    let present_key2 = PresentKey::new(device_key, incarnation, commit_2, 202);
    consumer.record_present_disposition(present_key2, PresentDisposition::pending());

    consumer
        .consume(
            OwnerEvent::Terminal {
                commit: commit_2,
                terminal: TerminalState::FailedBeforeSubmit(
                    crate::kms::owner::record::FailureCause::IoctlRejected { errno: libc::EBUSY },
                ),
            },
            &mut service,
        )
        .unwrap();

    let disp2 = consumer
        .present_disposition(&present_key2)
        .expect("disposition 2 exists");
    assert_eq!(disp2.completion, CompletionDisposition::Suppressed);
    assert_eq!(disp2.release, ReleaseDisposition::Retained);
    assert!(
        consumer.take_released_presents().is_empty(),
        "FailedBeforeSubmit cannot signal release"
    );

    // 3. on_available: when resources become releasable, present release is extracted
    // and disposition becomes Released
    assert!(service.is_releasable(&old_key));
    consumer.on_available(&[old_key], &mut service).unwrap();

    let disp1_after = consumer
        .present_disposition(&present_key1)
        .expect("disposition 1 after");
    assert_eq!(disp1_after.completion, CompletionDisposition::Emitted);
    assert_eq!(disp1_after.release, ReleaseDisposition::Released);

    let released = consumer.take_released_presents();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].event.present_id, 101);
    assert!(released[0].wake.is_some());
    assert!(consumer.take_released_presents().is_empty());
}

#[test]
fn c0_2ci_commit_grouped_skip_and_duplicate_protection() {
    use crate::kms::owner::identity::CommitId;

    let (mut service, old_a, drops_old_a) = spy_service();
    let old_a_key = old_a.key();

    let drops_old_b = Rc::new(Cell::new(0));
    let old_b = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops_old_b),
        }))
        .unwrap();
    let _old_b_key = old_b.key();

    let crtc1 = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(10).unwrap()),
    );
    let crtc2 = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(20).unwrap()),
    );
    let member1 = GroupMember::new(crtc1, 1, 1);
    let member2 = GroupMember::new(crtc2, 1, 1);

    // Grouped commit that changes ONLY crtc1 (member1).
    // member2 is retained unchanged across this grouped commit, so it registers NO KMS obligation (round-4 m-1).
    let commit_id = CommitId::for_tests(88);
    let old_a_kms = service.register_kms(old_a_key, commit_id, member1).unwrap();

    let mut consumer = CommitResourceConsumer::new();

    let old_res = CommitResources::new(
        vec![old_a],
        None,
        None,
        None,
        vec![member1, member2],
        vec![(old_a_key, old_a_kms, member1)],
    );
    let new_res = CommitResources::new(vec![], None, None, None, vec![member1, member2], vec![]);
    let accepted =
        crate::kms::owner::ledger::Submitted::new(vec![old_res], vec![new_res]).accepted();

    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: commit_id,
                resources: accepted,
            },
            &mut service,
        )
        .unwrap();

    // HardwareComplete arrives
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: commit_id },
            &mut service,
        )
        .unwrap();

    // old_a was discharged
    consumer.on_available(&[old_a_key], &mut service).unwrap();
    service.service_ready();
    assert_eq!(drops_old_a.get(), 1);

    // old_b had NO obligation registered for this commit, so it is unaffected and alive
    assert_eq!(drops_old_b.get(), 0);
    drop(old_b);
    service.service_ready();
    assert_eq!(drops_old_b.get(), 1);

    // Duplicate HardwareComplete notification cannot double-discharge or error
    assert!(
        consumer
            .consume(
                crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: commit_id },
                &mut service,
            )
            .is_ok()
    );

    // Duplicate Presented notification is harmless
    assert!(
        consumer
            .consume(
                crate::kms::owner::device::OwnerEvent::Presented {
                    commit: commit_id,
                    samples: std::collections::BTreeMap::new(),
                },
                &mut service,
            )
            .is_ok()
    );
}

#[test]
fn c0_2ci_capacity_six_roles_and_busy_rejection() {
    let mut capacity = DirectCapacity::new();
    let mut held = Vec::new();
    for role in [
        DirectRole::Current,
        DirectRole::Submitted,
        DirectRole::Successor,
        DirectRole::Preparing,
        DirectRole::OrdinaryRetirement,
        DirectRole::ExitRetirement,
    ] {
        held.push(capacity.reserve(role).unwrap());
    }
    assert_eq!(capacity.occupied(), 6);
    assert!(matches!(
        capacity.reserve(DirectRole::Preparing),
        Err(ResourceError::Busy)
    ));
    assert!(!capacity.can_enter_direct());
}

#[test]
fn c0_2ci_capacity_transitions_and_move_into_reserved() {
    let mut capacity = DirectCapacity::new();
    assert!(capacity.can_enter_direct());

    // Preparing candidate failure: clean up candidate and cancel role
    let prep = capacity.reserve(DirectRole::Preparing).unwrap();
    assert_eq!(capacity.occupied(), 1);
    assert!(!capacity.is_vacant(DirectRole::Preparing));
    assert!(capacity.cancel_reservation(prep).is_ok());
    assert_eq!(capacity.occupied(), 0);
    assert!(capacity.is_vacant(DirectRole::Preparing));

    // Successful candidate preparation moves to Successor
    let mut prep = capacity.reserve(DirectRole::Preparing).unwrap();
    let serial1 = prep.serial();
    assert!(capacity.move_role(&mut prep, DirectRole::Successor).is_ok());
    assert_eq!(prep.role(), DirectRole::Successor);
    assert_eq!(prep.serial(), serial1);
    assert!(capacity.is_vacant(DirectRole::Preparing));
    assert!(!capacity.is_vacant(DirectRole::Successor));

    // Successor moves to Submitted
    let mut succ = prep;
    assert!(capacity.move_role(&mut succ, DirectRole::Submitted).is_ok());
    assert_eq!(succ.role(), DirectRole::Submitted);

    // Initial commit completes: Submitted moves to Current
    let mut cur = succ;
    assert!(capacity.move_role(&mut cur, DirectRole::Current).is_ok());
    assert_eq!(cur.role(), DirectRole::Current);

    // Next commit preparation: reserve Preparing, attach to commit resources
    let prep2 = capacity.reserve(DirectRole::Preparing).unwrap();
    let res = CommitResources::new(vec![], None, None, None, vec![], vec![]);
    let mut res = capacity.attach(prep2, res).unwrap();
    assert!(res.direct_role.is_some());

    // Replacement: reserve OrdinaryRetirement before dispatch
    let reserved_retire = capacity.reserve(DirectRole::OrdinaryRetirement).unwrap();
    let retire_serial = reserved_retire.serial();
    assert!(retire_serial > cur.serial());

    // On completion, old current moves into pre-reserved retirement role
    let mut old_current = cur;
    assert!(
        capacity
            .move_into_reserved(&mut old_current, reserved_retire)
            .is_ok()
    );
    assert_eq!(old_current.role(), DirectRole::OrdinaryRetirement);
    assert_eq!(old_current.serial(), retire_serial);
    assert!(capacity.is_vacant(DirectRole::Current));

    // Finish old current once release obligations are satisfied
    assert!(capacity.finish_role(old_current).is_ok());
    assert!(capacity.is_vacant(DirectRole::OrdinaryRetirement));

    // Also finish the attached res preparing role so Preparing becomes vacant
    assert!(
        capacity
            .finish_role(res.direct_role.take().unwrap())
            .is_ok()
    );
    assert!(capacity.is_vacant(DirectRole::Preparing));

    // Test invalid move into reserved preserves source ownership and reserved token
    let mut same_role_occ = capacity.reserve(DirectRole::Preparing).unwrap();
    let same_role_reserved = capacity.reserve(DirectRole::ExitRetirement).unwrap();
    // Tamper with role
    same_role_occ.role = DirectRole::ExitRetirement;
    let (err, recovered) = capacity
        .move_into_reserved(&mut same_role_occ, same_role_reserved)
        .err()
        .unwrap();
    assert_eq!(err, ResourceError::InvalidState);
    assert_eq!(recovered.role(), DirectRole::ExitRetirement);
    // Cleanup recovered tokens
    let _ = capacity.finish_role(recovered);
    let _ = capacity.finish_role(same_role_occ);
}

#[test]
fn c0_2ci_capacity_exit_retirement_and_unflip_transitions() {
    let mut capacity = DirectCapacity::new();
    assert!(capacity.can_enter_direct());

    // A is in OrdinaryRetirement
    let a_retire = capacity.reserve(DirectRole::OrdinaryRetirement).unwrap();
    // B is Current
    let mut b_current = capacity.reserve(DirectRole::Current).unwrap();

    // B unflipped while A awaits release: B moves to ExitRetirement
    let b_exit_reserve = capacity.reserve(DirectRole::ExitRetirement).unwrap();
    assert!(
        capacity
            .move_into_reserved(&mut b_current, b_exit_reserve)
            .is_ok()
    );
    assert_eq!(b_current.role(), DirectRole::ExitRetirement);

    // Both retirement positions are now occupied: direct re-entry is blocked
    assert_eq!(capacity.occupied(), 2);
    assert!(!capacity.can_enter_direct());

    // A finishes release obligations: OrdinaryRetirement is freed
    assert!(capacity.finish_role(a_retire).is_ok());
    assert_eq!(capacity.occupied(), 1);
    // Direct re-entry is STILL blocked because ExitRetirement is occupied
    assert!(!capacity.can_enter_direct());

    // B finishes exit release obligations: ExitRetirement is freed
    assert!(capacity.finish_role(b_current).is_ok());
    assert_eq!(capacity.occupied(), 0);
    // Now both retirement roles are vacant: direct re-entry is permitted
    assert!(capacity.can_enter_direct());
}

#[test]
fn c0_2ci_capacity_delayed_on_available_discharges_and_unblocks() {
    let (mut service, old_alloc, drops_old) = spy_service();
    let old_key = old_alloc.key();

    let drops_new_spy = Rc::new(Cell::new(0));
    let new_lease = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&drops_new_spy),
        }))
        .unwrap();

    let crtc = CrtcKey::new(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(1).unwrap()),
    );
    let member = GroupMember::new(crtc, 1, 1);
    let commit_id = crate::kms::owner::identity::CommitId::for_tests(101);

    let mut consumer = CommitResourceConsumer::new();

    // 1. Ordinary retirement flow through consume and on_available
    let old_retire_slot = consumer
        .capacity
        .reserve(DirectRole::OrdinaryRetirement)
        .unwrap();
    let old_kms = service
        .register(old_key, ObligationKind::KmsRelease)
        .unwrap();
    let old_gpu = service.register(old_key, ObligationKind::Gpu).unwrap();

    let old_res = CommitResources::new(
        vec![old_alloc],
        None,
        None,
        None,
        vec![member],
        vec![(old_key, old_kms, member)],
    )
    .with_direct_role(old_retire_slot);

    let new_res = CommitResources::new(vec![new_lease], None, None, None, vec![member], vec![]);

    // Commit accepted and retired
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: commit_id,
                resources: crate::kms::owner::ledger::Submitted::new(vec![old_res], vec![new_res])
                    .accepted(),
            },
            &mut service,
        )
        .unwrap();

    // OrdinaryRetirement is occupied
    assert!(matches!(
        consumer.capacity.reserve(DirectRole::OrdinaryRetirement),
        Err(ResourceError::Busy)
    ));

    // HardwareComplete discharges KMS obligation only
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: commit_id },
            &mut service,
        )
        .unwrap();

    // GPU obligation is STILL pending, so on_available does not yet free OrdinaryRetirement
    consumer.on_available(&[old_key], &mut service).unwrap();
    assert!(matches!(
        consumer.capacity.reserve(DirectRole::OrdinaryRetirement),
        Err(ResourceError::Busy)
    ));
    assert_eq!(drops_old.get(), 0);

    // Delayed GPU completion arrives
    service.apply_validated_proof(old_key, old_gpu).unwrap();
    consumer.on_available(&[old_key], &mut service).unwrap();
    service.service_ready();

    // OrdinaryRetirement is now finished and freed, direct admission scheduled
    assert!(consumer.direct_admission_scheduled);
    assert_eq!(drops_old.get(), 1);
    // Reservation is now unblocked!
    let next_retire = consumer
        .capacity
        .reserve(DirectRole::OrdinaryRetirement)
        .unwrap();
    assert!(consumer.capacity.finish_role(next_retire).is_ok());

    // 2. Rejection flow through consume and on_available
    let rej_drops = Rc::new(Cell::new(0));
    let rej_lease = service
        .adopt(AllocationPayload::Spy(SpyAllocation {
            drops: Rc::clone(&rej_drops),
        }))
        .unwrap();
    let rej_key = rej_lease.key();
    let rej_gpu = service.register(rej_key, ObligationKind::Gpu).unwrap();
    let rej_kms = service
        .register(rej_key, ObligationKind::KmsRelease)
        .unwrap();

    let rej_prep = consumer.capacity.reserve(DirectRole::Preparing).unwrap();
    let rej_res = CommitResources::new(
        vec![rej_lease],
        None,
        None,
        None,
        vec![member],
        vec![(rej_key, rej_kms, member)],
    )
    .with_direct_role(rej_prep);

    let rej_commit = crate::kms::owner::identity::CommitId::for_tests(102);

    // Rejection event arrives
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::ResourcesReleased {
                commit: rej_commit,
                resources: vec![rej_res],
            },
            &mut service,
        )
        .unwrap();

    // Preparing is occupied, so another Preparing reservation fails
    assert!(matches!(
        consumer.capacity.reserve(DirectRole::Preparing),
        Err(ResourceError::Busy)
    ));

    // on_available before GPU completion does NOT free it
    consumer.on_available(&[rej_key], &mut service).unwrap();
    assert!(matches!(
        consumer.capacity.reserve(DirectRole::Preparing),
        Err(ResourceError::Busy)
    ));

    // Delayed GPU completion arrives
    service.apply_validated_proof(rej_key, rej_gpu).unwrap();
    consumer.on_available(&[rej_key], &mut service).unwrap();
    service.service_ready();

    // Preparing reservation is now freed and unblocked!
    assert_eq!(rej_drops.get(), 1);
    let next_prep = consumer.capacity.reserve(DirectRole::Preparing).unwrap();
    assert!(consumer.capacity.finish_role(next_prep).is_ok());
}

#[test]
fn c0_2ci_capacity_unexpected_token_drop_closes_admission() {
    let mut capacity = DirectCapacity::new();
    assert!(capacity.can_enter_direct());

    // Reserve a role and drop it without cancel_reservation or finish_role
    {
        let token = capacity.reserve(DirectRole::Current).unwrap();
        assert_eq!(capacity.occupied(), 1);
        drop(token);
    }

    // Dropping token unexpectedly must close admission with its slot remaining charged
    assert_eq!(capacity.occupied(), 1);
    assert!(capacity.is_admission_closed());
    assert!(!capacity.can_enter_direct());
    assert!(matches!(
        capacity.reserve(DirectRole::Preparing),
        Err(ResourceError::Busy)
    ));
}

#[test]
fn c0_2ci_handoff_failure_returns_bundle_and_slot_by_value() {
    let (service, _old_lease, old_drops) = spy_service();
    let new_drops = Rc::new(Cell::new(0));
    let device = service.device();
    let incarnation = service.incarnation();

    let owner =
        DeviceCommitOwner::<CommitResources>::new(incarnation, LifecycleEpochId::first(), 1);
    let consumer = CommitResourceConsumer::new();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(calls);
    let drm = DrmCleanupRegistry::new_with_io(device, incarnation, Box::new(io));
    let ingress = CompletionIngress::new();

    let bundle = IncarnationBundle::new(owner, service, consumer, drm, None, ingress);

    let mut supervisor = RetainingSupervisor::new();
    let wrong_slot = supervisor.reserve_slot(device, IncarnationId::from_raw(999));

    let result = supervisor.router.transfer(wrong_slot, bundle);
    let Err((error, slot, bundle)) = result else {
        panic!("invalid recipient accepted");
    };
    assert_eq!(error, ResourceError::WrongIncarnation);
    assert_eq!(bundle.owner.incarnation(), incarnation);
    assert_eq!(old_drops.get(), 0);
    assert_eq!(new_drops.get(), 0);
    assert_eq!(slot.incarnation(), IncarnationId::from_raw(999));
}

#[test]
fn c0_2ci_handoff_success_routes_late_events_and_completions() {
    let (mut service, old_lease, old_drops) = spy_service();
    let old_key = old_lease.key();
    let device = service.device();
    let incarnation = service.incarnation();

    let old_kms = service
        .register(old_key, ObligationKind::KmsRelease)
        .unwrap();
    let old_gpu = service.register(old_key, ObligationKind::Gpu).unwrap();

    let crtc = CrtcKey::new(
        device,
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(1).unwrap()),
    );
    let member = GroupMember::new(crtc, 1, 1);
    let commit_id = CommitId::for_tests(201);

    let old_res = CommitResources::new(
        vec![old_lease],
        None,
        None,
        None,
        vec![member],
        vec![(old_key, old_kms, member)],
    )
    .with_commit_id(commit_id);

    let mut consumer = CommitResourceConsumer::new();
    // Old resources awaiting release
    consumer.releasing_resources.push(old_res);

    let owner =
        DeviceCommitOwner::<CommitResources>::new(incarnation, LifecycleEpochId::first(), 1);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(calls);
    let drm = DrmCleanupRegistry::new_with_io(device, incarnation, Box::new(io));
    let ingress = CompletionIngress::new();

    let bundle = IncarnationBundle::new(owner, service, consumer, drm, None, ingress);

    let mut supervisor = RetainingSupervisor::new();
    let slot = supervisor.reserve_slot(device, incarnation);

    // Transfer succeeds
    assert!(supervisor.router.transfer(slot, bundle).is_ok());

    // Deliver late event to ingress
    supervisor
        .router
        .deliver_event(
            incarnation,
            OwnerEvent::HardwareComplete { commit: commit_id },
        )
        .unwrap();

    // Deliver late returned descriptor to ingress
    let (r, w) = nix::unistd::pipe().unwrap();
    supervisor
        .router
        .deliver_descriptor(incarnation, r)
        .unwrap();
    drop(w);

    // Process router service turn
    supervisor.router.service(Instant::now());

    // HardwareComplete was consumed, but GPU is still pending: old allocation not yet destroyed
    assert_eq!(old_drops.get(), 0);

    // Deliver late GPU completion
    {
        let bundle_ref = supervisor.router.get_bundle_mut(&incarnation).unwrap();
        bundle_ref
            .resources
            .apply_validated_proof(old_key, old_gpu)
            .unwrap();
    }

    // Process service turn again
    supervisor.router.service(Instant::now());
    {
        let bundle_ref = supervisor.router.get_bundle_mut(&incarnation).unwrap();
        bundle_ref.resources.service_ready();
    }

    // Now old allocation is destroyed exactly once under recipient
    assert_eq!(old_drops.get(), 1);
}

#[test]
fn c0_2ci_handoff_unresolved_kms_rejects_teardown_release() {
    let (mut service, old_lease, drops) = spy_service();
    let old_key = old_lease.key();
    let device = service.device();
    let incarnation = service.incarnation();

    let crtc = CrtcKey::new(
        device,
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(1).unwrap()),
    );
    let member = GroupMember::new(crtc, 1, 1);
    let commit = CommitId::for_tests(301);

    // 1. Register KMS and GPU obligations
    let old_kms = service.register_kms(old_key, commit, member).unwrap();
    let old_gpu = service.register(old_key, ObligationKind::Gpu).unwrap();

    // Quarantine: acceptance unknown freezes the allocation
    service.freeze(old_key).unwrap();

    // Satisfy GPU completion
    service.apply_validated_proof(old_key, old_gpu).unwrap();

    let supervisor = RetainingSupervisor::new();

    // Try teardown release while KMS obligation is Outstanding
    let proof = supervisor.issue_teardown_release(incarnation, vec![old_key]);
    assert_eq!(
        service.apply_teardown_release(proof).err().unwrap(),
        ResourceError::InvalidProof
    );
    assert_eq!(drops.get(), 0);

    // Try device barrier for a SECOND device key (different device)
    let second_device = DrmDeviceKey {
        major: 226,
        minor: 1,
    };
    service.record_device_barrier(DeviceBarrier::FileFamilyClosed(second_device));
    let proof = supervisor.issue_teardown_release(incarnation, vec![old_key]);
    assert_eq!(
        service.apply_teardown_release(proof).err().unwrap(),
        ResourceError::InvalidProof
    );

    // Try stale-generation / mismatched CRTC PriorBufferReleased
    let stale_member = GroupMember::new(crtc, 2, 1); // topology generation 2 != 1
    assert!(
        service
            .record_kms_discharged(old_key, old_kms, commit, stale_member)
            .is_err()
    );
    let proof = supervisor.issue_teardown_release(incarnation, vec![old_key]);
    assert_eq!(
        service.apply_teardown_release(proof).err().unwrap(),
        ResourceError::InvalidProof
    );

    // Path A: Correlated PriorBufferReleased for the exact commit/CRTC generation
    service
        .record_kms_discharged(old_key, old_kms, commit, member)
        .unwrap();
    let proof = supervisor.issue_teardown_release(incarnation, vec![old_key]);
    assert!(service.apply_teardown_release(proof).is_ok());

    // Dropping lease and servicing destroys the allocation exactly once
    drop(old_lease);
    service.service_ready();
    assert_eq!(drops.get(), 1);

    // Path B (from reset fixture): complete-family closure after reap
    let (mut service_b, old_lease_b, drops_b) = spy_service();
    let old_key_b = old_lease_b.key();
    let _old_kms_b = service_b.register_kms(old_key_b, commit, member).unwrap();
    service_b.freeze(old_key_b).unwrap();

    // Family closed barrier for matching device
    service_b.record_device_barrier(DeviceBarrier::FileFamilyClosed(device));
    let proof_b = supervisor.issue_teardown_release(incarnation, vec![old_key_b]);
    assert!(service_b.apply_teardown_release(proof_b).is_ok());

    drop(old_lease_b);
    service_b.service_ready();
    assert_eq!(drops_b.get(), 1);
}

#[test]
fn c0_2ci_handoff_unavailable_recipient_and_duplicate_transfer() {
    let (service, _lease, _drops) = spy_service();
    let device = service.device();
    let incarnation = service.incarnation();

    let owner =
        DeviceCommitOwner::<CommitResources>::new(incarnation, LifecycleEpochId::first(), 1);
    let consumer = CommitResourceConsumer::new();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(calls);
    let drm = DrmCleanupRegistry::new_with_io(device, incarnation, Box::new(io));
    let ingress = CompletionIngress::new();

    let bundle = IncarnationBundle::new(owner, service, consumer, drm, None, ingress);

    let mut supervisor = RetainingSupervisor::new();
    let slot = supervisor.reserve_slot(device, incarnation);

    // First transfer succeeds
    assert!(supervisor.router.transfer(slot, bundle).is_ok());

    // Attempting duplicate transfer to the same incarnation fails with Busy
    let (service2, _lease2, _drops2) = spy_service();
    let owner2 =
        DeviceCommitOwner::<CommitResources>::new(incarnation, LifecycleEpochId::first(), 1);
    let consumer2 = CommitResourceConsumer::new();
    let calls2 = Rc::new(RefCell::new(Vec::new()));
    let io2 = MockCleanupIo::new(calls2);
    let drm2 = DrmCleanupRegistry::new_with_io(device, incarnation, Box::new(io2));
    let ingress2 = CompletionIngress::new();
    let bundle2 = IncarnationBundle::new(owner2, service2, consumer2, drm2, None, ingress2);
    let slot2 = supervisor.reserve_slot(device, incarnation);

    let result = supervisor.router.transfer(slot2, bundle2);
    let Err((err, _recovered_slot, recovered_bundle)) = result else {
        panic!("duplicate transfer should fail");
    };
    assert_eq!(err, ResourceError::Busy);
    assert_eq!(recovered_bundle.owner.incarnation(), incarnation);

    // Delivering to an unregistered incarnation returns Detached
    assert_eq!(
        supervisor
            .router
            .deliver_event(
                IncarnationId::from_raw(888),
                OwnerEvent::HardwareComplete {
                    commit: CommitId::for_tests(1)
                }
            )
            .err()
            .unwrap(),
        ResourceError::Detached
    );
}

fn test_crtc_key(major: u32, minor: u32, handle: u32) -> CrtcKey {
    CrtcKey::new(
        DrmDeviceKey { major, minor },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(handle).unwrap()),
    )
}

#[test]
fn c0_2ci_commit_terminal_completed_does_not_freeze_and_becomes_releasable() {
    use crate::kms::owner::{identity::CommitId, record::TerminalState};

    let (mut service, old, drops_old) = spy_service();
    let old_key = old.key();

    let (new, drops_new) = {
        let drops = Rc::new(Cell::new(0));
        let alloc = service
            .adopt(AllocationPayload::Spy(SpyAllocation {
                drops: Rc::clone(&drops),
            }))
            .unwrap();
        (alloc, drops)
    };
    let new_key = new.key();

    let dev = service.device();
    let crtc = test_crtc_key(dev.major, dev.minor, 1);
    let member = GroupMember::new(crtc, 1, 1);

    let commit_id = CommitId::for_tests(701);
    let old_kms = service.register_kms(old_key, commit_id, member).unwrap();
    let new_kms = service.register_kms(new_key, commit_id, member).unwrap();

    let old_res = CommitResources::new(
        vec![old],
        None,
        None,
        None,
        vec![member],
        vec![(old_key, old_kms, member)],
    );
    let new_res = CommitResources::new(
        vec![new],
        None,
        None,
        None,
        vec![member],
        vec![(new_key, new_kms, member)],
    );

    let mut consumer = CommitResourceConsumer::new();

    // 1. HardwareComplete arrives (can arrive before or after CompletionRetired)
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: commit_id },
            &mut service,
        )
        .unwrap();

    // 2. CompletionRetired arrives
    let accepted =
        crate::kms::owner::ledger::Submitted::new(vec![old_res], vec![new_res]).accepted();
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: commit_id,
                resources: accepted,
            },
            &mut service,
        )
        .unwrap();

    // 3. Terminal { Completed } arrives
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::Terminal {
                commit: commit_id,
                terminal: TerminalState::Completed,
            },
            &mut service,
        )
        .unwrap();

    // Decisive B-8 assertion: Terminal { Completed } MUST NOT freeze resources!
    assert!(!service.is_frozen(&old_key));
    assert!(!service.is_frozen(&new_key));

    // Old KMS obligation was discharged by HardwareComplete
    assert!(!service.has_pending_obligations(&old_key));

    // Releasing resources is releasable and can be destroyed
    consumer.on_available(&[old_key], &mut service).unwrap();
    service.service_ready();
    assert_eq!(drops_old.get(), 1); // Not frozen, successfully dropped!

    // New resources are current, not dropped
    assert_eq!(drops_new.get(), 0);
    assert!(service.has_pending_obligations(&new_key));

    // Clean up
    service.cancel(new_key, new_kms).unwrap();
    drop(consumer);
    service.service_ready();
    assert_eq!(drops_new.get(), 1);
}

#[test]
fn c0_2ci_commit_terminal_failed_before_submit_does_not_freeze_current_set() {
    use crate::kms::owner::{
        identity::CommitId,
        record::{FailureCause, RefusalCause, TerminalState},
    };

    let (mut service, current, drops_current) = spy_service();
    let current_key = current.key();

    let dev = service.device();
    let crtc = test_crtc_key(dev.major, dev.minor, 1);
    let member = GroupMember::new(crtc, 1, 1);

    let commit_id = CommitId::for_tests(702);
    let current_kms = service
        .register_kms(current_key, commit_id, member)
        .unwrap();

    let current_res = CommitResources::new(
        vec![current],
        None,
        None,
        None,
        vec![member],
        vec![(current_key, current_kms, member)],
    );

    let mut consumer = CommitResourceConsumer::new();

    // Terminal { FailedBeforeSubmit } arrives BEFORE ResourcesStillCurrent on rejection (device.rs:2233-2246)
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::Terminal {
                commit: commit_id,
                terminal: TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                    RefusalCause::Reaped,
                )),
            },
            &mut service,
        )
        .unwrap();

    // ResourcesStillCurrent arrives
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::ResourcesStillCurrent {
                commit: commit_id,
                resources: vec![current_res],
            },
            &mut service,
        )
        .unwrap();

    // Decisive B-8 assertion: current set is neither frozen nor cancelled-to-discharged!
    assert!(!service.is_frozen(&current_key));
    assert_ne!(
        service.kms_disposition(current_key, current_kms),
        Some(crate::kms::render::resources::handoff::KmsDisposition::Discharged)
    );

    // Current resources are still held
    assert_eq!(consumer.current_resources.len(), 1);
    assert_eq!(drops_current.get(), 0);

    // Dropping current resources releases them without being blocked by freeze
    drop(consumer);
    service.service_ready();
    assert_eq!(drops_current.get(), 1);
}

#[test]
fn c0_2ci_commit_terminal_completion_unknown_freezes_only_that_commit() {
    use crate::kms::owner::{
        identity::CommitId,
        record::{TerminalState, UnknownCause},
    };

    let (mut service, alloc1, drops1) = spy_service();
    let key1 = alloc1.key();

    let (alloc2, drops2) = {
        let drops = Rc::new(Cell::new(0));
        let a = service
            .adopt(AllocationPayload::Spy(SpyAllocation {
                drops: Rc::clone(&drops),
            }))
            .unwrap();
        (a, drops)
    };
    let key2 = alloc2.key();

    let dev = service.device();
    let crtc = test_crtc_key(dev.major, dev.minor, 1);
    let member = GroupMember::new(crtc, 1, 1);

    let commit1 = CommitId::for_tests(703);
    let commit2 = CommitId::for_tests(704);

    let res1 = CommitResources::new(vec![alloc1], None, None, None, vec![member], vec![])
        .with_commit_id(commit1);
    let res2 = CommitResources::new(vec![alloc2], None, None, None, vec![member], vec![])
        .with_commit_id(commit2);

    let mut consumer = CommitResourceConsumer::new();
    consumer.releasing_resources.push(res1);
    consumer.releasing_resources.push(res2);

    // Terminal { CompletionUnknown } for commit1 only!
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::Terminal {
                commit: commit1,
                terminal: TerminalState::CompletionUnknown(UnknownCause::IncompleteFenceOutput {
                    expected: 1,
                    returned: 0,
                }),
            },
            &mut service,
        )
        .unwrap();

    // key1 is frozen, key2 is NOT frozen!
    assert!(service.is_frozen(&key1));
    assert!(!service.is_frozen(&key2));

    // Dropping consumer cannot free key1 because it is frozen
    drop(consumer);
    service.service_ready();
    assert_eq!(drops1.get(), 0); // Frozen, stays retained!
    assert_eq!(drops2.get(), 1); // Not frozen, drops normally!
}

#[test]
fn c0_2ci_commit_quarantined_closes_gate_and_freezes_only_that_commit() {
    use crate::kms::owner::identity::CommitId;

    let (mut service, alloc1, drops1) = spy_service();
    let key1 = alloc1.key();

    let (alloc2, drops2) = {
        let drops = Rc::new(Cell::new(0));
        let a = service
            .adopt(AllocationPayload::Spy(SpyAllocation {
                drops: Rc::clone(&drops),
            }))
            .unwrap();
        (a, drops)
    };
    let key2 = alloc2.key();

    let dev = service.device();
    let crtc = test_crtc_key(dev.major, dev.minor, 1);
    let member = GroupMember::new(crtc, 1, 1);

    let gate = TransportGate::new_legacy(
        dev,
        IncarnationId::first(),
        Box::new(FakeDirectOwnershipState::default()),
    );
    let gate_handle = gate.handle();

    let mut consumer = CommitResourceConsumer::new().with_gate_handle(gate_handle.clone());
    assert!(!gate_handle.is_closed());

    let commit1 = CommitId::for_tests(705);
    let commit2 = CommitId::for_tests(706);

    let res1 = CommitResources::new(vec![alloc1], None, None, None, vec![member], vec![])
        .with_commit_id(commit1);
    let res2 = CommitResources::new(vec![alloc2], None, None, None, vec![member], vec![])
        .with_commit_id(commit2);

    consumer.releasing_resources.push(res1);
    consumer.releasing_resources.push(res2);

    // Quarantined arrives for commit1
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::Quarantined { commit: commit1 },
            &mut service,
        )
        .unwrap();

    // M-15: Gate MUST be closed!
    assert!(gate_handle.is_closed());
    assert_eq!(gate.state(), TransportState::Closed);

    // M-15: Only commit1's entries are frozen!
    assert!(service.is_frozen(&key1));
    assert!(!service.is_frozen(&key2));

    drop(consumer);
    service.service_ready();
    assert_eq!(drops1.get(), 0); // key1 retained
    assert_eq!(drops2.get(), 1); // key2 dropped
}

#[test]
fn c0_2ci_commit_presented_selects_reference_crtc_sample() {
    use crate::kms::owner::{clock::ClockSample, identity::CommitId};

    let dev = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let commit = CommitId::for_tests(707);
    let key = PresentKey::new(dev, incarnation, commit, 1);

    let mut consumer = CommitResourceConsumer::new();

    // Record disposition with reference CRTC = 2
    let ref_crtc = 2;
    consumer.record_present_disposition_with_reference(
        key,
        PresentDisposition::pending(),
        ref_crtc,
    );

    let mut samples = BTreeMap::new();
    let sample1 = ClockSample {
        msc: 100,
        ust: 1000,
    };
    let sample2 = ClockSample {
        msc: 200,
        ust: 2000,
    };
    samples.insert(1, sample1);
    samples.insert(2, sample2);

    let mut service = ResourceService::new(dev, incarnation);
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::Presented { commit, samples },
            &mut service,
        )
        .unwrap();

    let disp = consumer
        .present_dispositions
        .get(&key)
        .expect("present disposition");
    assert_eq!(disp.completion, CompletionDisposition::Emitted);
    assert_eq!(disp.sample, Some(sample2)); // Selected CRTC 2 sample!
}

#[test]
fn c0_2ci_commit_register_dependencies_and_pre_ipc_cancellation() {
    use crate::kms::owner::identity::CommitId;

    let (mut service, old_alloc, drops_old) = spy_service();
    let old_key = old_alloc.key();

    let (new_alloc, drops_new) = {
        let drops = Rc::new(Cell::new(0));
        let a = service
            .adopt(AllocationPayload::Spy(SpyAllocation {
                drops: Rc::clone(&drops),
            }))
            .unwrap();
        (a, drops)
    };
    let _new_key = new_alloc.key();

    let dev = service.device();
    let crtc = test_crtc_key(dev.major, dev.minor, 1);
    let member = GroupMember::new(crtc, 1, 1);

    let commit = CommitId::for_tests(708);

    let old_res = CommitResources::new(vec![old_alloc], None, None, None, vec![member], vec![]);
    let new_res = CommitResources::new(vec![new_alloc], None, None, None, vec![member], vec![]);

    // 1. register_commit_dependencies registers displaced pairs before Submitted::new
    let submitted =
        register_commit_dependencies(commit, vec![old_res], vec![new_res], &mut service)
            .expect("register_commit_dependencies");

    // Old allocation has a registered KMS obligation
    assert!(service.has_pending_obligations(&old_key));

    // 2. Pre-IPC failure / cancel returns ownership of resources and cancels registrations
    let (old_returned, new_returned) = cancel_pre_ipc_commit(submitted, &mut service);

    assert_eq!(old_returned.len(), 1);
    assert_eq!(new_returned.len(), 1);

    // Obligation cancelled (not pending, not discharged)
    assert!(!service.has_pending_obligations(&old_key));

    // Dropping returned resources drops leases
    drop(old_returned);
    drop(new_returned);
    service.service_ready();
    assert_eq!(drops_old.get(), 1);
    assert_eq!(drops_new.get(), 1);
}

#[test]
fn c0_2ci_commit_group_member_validate_unique() {
    use crate::kms::owner::identity::CommitId;

    let dev = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let crtc1 = CrtcKey::new(
        dev,
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(1).unwrap()),
    );
    let crtc2 = CrtcKey::new(
        dev,
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(2).unwrap()),
    );

    let m1 = GroupMember::new(crtc1, 1, 1);
    let m2 = GroupMember::new(crtc2, 1, 1);
    let m1_dup = GroupMember::new(crtc1, 1, 1);

    assert!(GroupMember::validate_unique(&[m1, m2]));
    assert!(!GroupMember::validate_unique(&[m1, m2, m1_dup]));

    // Validation failure in register_commit_dependencies returns ResourceError::InvalidProof
    let mut service = ResourceService::new(dev, IncarnationId::first());
    let old_res = CommitResources::new(vec![], None, None, None, vec![m1, m1_dup], vec![]);
    let new_res = CommitResources::new(vec![], None, None, None, vec![m2], vec![]);
    let err = register_commit_dependencies(
        CommitId::for_tests(709),
        vec![old_res],
        vec![new_res],
        &mut service,
    );
    assert!(matches!(err, Err((ResourceError::InvalidProof, _, _))));
}

#[test]
fn c0_2ci_commit_discharge_atomic_validate_then_apply_failure_rolls_back() {
    use crate::kms::owner::identity::CommitId;

    let (mut service, old, drops_old) = spy_service();
    let old_key = old.key();

    let dev = service.device();
    let crtc = test_crtc_key(dev.major, dev.minor, 1);
    let member = GroupMember::new(crtc, 1, 1);

    let commit = CommitId::for_tests(710);
    let ob1 = service.register_kms(old_key, commit, member).unwrap();
    let ob2_invalid = ObligationId(999999);

    let old_res = CommitResources::new(
        vec![old],
        None,
        None,
        None,
        vec![member],
        vec![(old_key, ob1, member), (old_key, ob2_invalid, member)],
    );
    let new_res = CommitResources::new(vec![], None, None, None, vec![member], vec![]);
    let accepted =
        crate::kms::owner::ledger::Submitted::new(vec![old_res], vec![new_res]).accepted();

    let mut consumer = CommitResourceConsumer::new();
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit,
                resources: accepted,
            },
            &mut service,
        )
        .unwrap();

    // HardwareComplete arrives - atomic validate-all encounters ob2_invalid!
    let err = consumer.consume(
        crate::kms::owner::device::OwnerEvent::HardwareComplete { commit },
        &mut service,
    );
    assert_eq!(err, Err(ResourceError::InvalidProof));

    // M-4: Atomic validate-then-apply rolls back: ob1 was NOT applied and remains pending!
    assert!(service.has_pending_obligations(&old_key));
    assert_eq!(consumer.releasing_resources[0].kms_obligations.len(), 2);

    // Clean up
    service.cancel(old_key, ob1).unwrap();
    drop(consumer);
    service.service_ready();
    assert_eq!(drops_old.get(), 1);
}
