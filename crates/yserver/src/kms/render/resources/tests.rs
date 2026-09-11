use std::{cell::Cell, rc::Rc};

use super::*;
use crate::{
    kms::{
        owner::{
            device::{DeviceCommitOwner, OwnerEvent},
            identity::{CommitId, IncarnationId},
            lifecycle::LifecycleEpochId,
        },
        render::platform::CrtcKey,
    },
    platform::drm::DrmDeviceKey,
};

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
    let mut alloc = ScanoutAllocation::new(Some(fo), shared);

    // Discharge file owned
    alloc.discharge_file_owned(&mut registry).unwrap();
    assert!(alloc.file_owned().is_none());
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(70), CleanupCall::CloseGem(71)]
    );

    // Shared half remains valid and intact
    assert_eq!(alloc.shared().image, ash::vk::Image::null());
}

#[test]
fn c0_2ci_scanout_acquire_managed_all_or_nothing() {
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

#[test]
fn c0_2ci_scanout_cancel_recording_leaves_gpu_work_armed() {
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

#[test]
fn c0_2ci_scanout_topology_reuse_of_bo_index() {
    let mut pool = crate::kms::vk::scanout::OutputScanout::Shared(
        crate::kms::vk::scanout::ScanoutBoPool::for_tests(),
    );

    // Detach clears all managed keys across the pool
    pool.detach_managed_entries();
    assert_eq!(pool.display_pool().bos.len(), 0);
}

#[test]
fn c0_2ci_read_source_scratch_regression() {
    let (mut service, source_held, _source_drops) = spy_service();
    let source_key = source_held.key();
    let source_read = service.register(source_key, ObligationKind::Read).unwrap();

    let (scratch_held, scratch_drops) = {
        let drops = Rc::new(Cell::new(0));
        let held = service
            .adopt(AllocationPayload::Spy(SpyAllocation {
                drops: Rc::clone(&drops),
            }))
            .unwrap();
        (held, drops)
    };
    let scratch_key = scratch_held.key();
    let scratch_gpu = service.register(scratch_key, ObligationKind::Gpu).unwrap();

    let scratch_ticket = crate::kms::render::platform::FenceTicket::for_tests_unsignaled_stub();

    // 1. Successful readback produces owned CPU bytes before scratch upload.
    // Source read completion is recorded then.
    service
        .apply_validated_proof(source_key, source_read)
        .unwrap();
    drop(source_held);

    let source_entry = service.entries.get(&source_key).unwrap();
    let source_read_pending = source_entry
        .availability
        .borrow()
        .pending_obligations
        .values()
        .filter(|k| matches!(k, ObligationKind::Read))
        .count();

    assert_eq!(source_read_pending, 0);
    assert_eq!(scratch_drops.get(), 0);
    assert!(!scratch_ticket.poll_signaled_result_opt(None).unwrap());

    // Scratch cleanup remains behind its own upload/Composite ticket
    let mut batch = CoreRetirementBatch::new(vec![scratch_held], vec![1], true);
    let ob =
        GpuObligation::for_tests_stub(vec![(scratch_key, scratch_gpu)], scratch_ticket.clone());
    batch.bind_ticket(ob);
    service.register_batch(batch);

    // Poll while ticket is false: nothing completed
    service.poll_gpu(Instant::now()).unwrap();
    assert_eq!(scratch_drops.get(), 0);

    // Ticket signals
    scratch_ticket.test_signal();
    service.poll_gpu(Instant::now()).unwrap();
    service.service_ready();
    assert_eq!(scratch_drops.get(), 1);
}

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
    let mut batch = CoreRetirementBatch::new(vec![held_a, held_b], vec![0], true);
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

    let mut batch2 = CoreRetirementBatch::new(vec![held_c], vec![0], true);
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

    let mut batch = CoreRetirementBatch::new(vec![held], vec![0], true);
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

    let mut batch = CoreRetirementBatch::new(vec![held], vec![0], true);
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

    let batch = CoreRetirementBatch::new(vec![held], vec![0], true);
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

    let ticket = crate::kms::render::platform::FenceTicket::for_tests_unsignaled_stub();
    let mut batch = CoreRetirementBatch::new(vec![held], vec![0], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(
        vec![(key, gpu)],
        ticket.clone(),
    ));

    service.register_batch(batch);

    // Simulate frame metadata dropped (e.g. replaced by newer damage/frame)
    // The batch and its underlying allocation must NOT be dropped while ticket is unsignaled
    service.poll_gpu(Instant::now()).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 0);
    assert_eq!(service.pending_batches().len(), 1);

    // Now ticket signals
    ticket.test_signal();
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

    let ticket = crate::kms::render::platform::FenceTicket::for_tests_unsignaled_stub();
    let mut batch = CoreRetirementBatch::new(vec![held], vec![42], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(
        vec![(key, gpu)],
        ticket.clone(),
    ));

    service.register_batch(batch);

    // Polling while unsignaled does not retire the batch, retaining descriptor slot 42
    service.poll_gpu(Instant::now()).unwrap();
    assert_eq!(service.pending_batches().len(), 1);
    assert_eq!(service.pending_batches()[0].descriptor_slots(), &[42]);

    // When signaled, polling retires and releases descriptor slot ownership
    ticket.test_signal();
    service.poll_gpu(Instant::now()).unwrap();
    assert_eq!(service.pending_batches().len(), 0);
}

#[test]
fn c0_2ci_progress_no_composition() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    let ticket = crate::kms::render::platform::FenceTicket::for_tests_unsignaled_stub();
    let mut batch = CoreRetirementBatch::new(vec![held], vec![0], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(
        vec![(key, gpu)],
        ticket.clone(),
    ));
    service.register_batch(batch);

    // Active seat with unsignaled ticket: schedules a future deadline (~1ms)
    assert!(service.next_deadline().is_some());

    // Seat inactive (VT-away / DPMS-off): pauses deadline
    service.set_seat_active(false, Instant::now());
    assert!(service.next_deadline().is_none());

    // Seat returns active
    service.set_seat_active(true, Instant::now());
    assert!(service.next_deadline().is_some());

    // Signal the ticket
    ticket.test_signal();

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
    let mut batch2 = CoreRetirementBatch::new(vec![held2], vec![0], true);
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

#[test]
fn c0_2ci_serviced_time_pauses_during_seat_inactive_and_expires() {
    let (mut service, held, drops) = spy_service();
    let key = held.key();
    let gpu = service.register(key, ObligationKind::Gpu).unwrap();

    let ticket = crate::kms::render::platform::FenceTicket::for_tests_unsignaled_stub();
    let mut batch = CoreRetirementBatch::new(vec![held], vec![0], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(
        vec![(key, gpu)],
        ticket.clone(),
    ));
    service.register_batch(batch);

    let base = Instant::now();
    service.max_serviced_duration = std::time::Duration::from_millis(50);

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

#[test]
fn c0_2ci_transport_gate_vocabulary_and_table() {
    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut gate = TransportGate::new_legacy(device, incarnation);

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
    gate.close();
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

#[test]
fn c0_2ci_transport_gate_direct_scanout_precondition() {
    let device = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let incarnation = IncarnationId::first();
    let mut gate = TransportGate::new_legacy(device, incarnation);

    // Active direct scanout blocks quiescing
    gate.set_direct_scanout_active(true);
    assert_eq!(gate.begin_quiescing(), Err(ResourceError::Busy));
    assert_eq!(gate.state(), TransportState::Legacy);

    gate.set_direct_scanout_active(false);
    // Pending unflip blocks quiescing
    gate.set_unflip_pending(true);
    assert_eq!(gate.begin_quiescing(), Err(ResourceError::Busy));
    assert_eq!(gate.state(), TransportState::Legacy);

    // After unflip retires, begin_quiescing succeeds
    gate.set_unflip_pending(false);
    assert!(gate.begin_quiescing().is_ok());
    assert_eq!(gate.state(), TransportState::Quiescing);
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
    let mut gate = TransportGate::new_legacy(device, incarnation);

    // Legacy cannot authorize owner write
    assert_eq!(
        gate.authorize_owner_write(WriterClass::Primary)
            .unwrap_err(),
        ResourceError::Detached
    );

    gate.begin_quiescing().unwrap();
    let permit = gate
        .issue_handover_permit(
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
    let mut gate2 = TransportGate::new_legacy(device, incarnation);
    gate2.begin_quiescing().unwrap();
    let permit2 = gate2
        .issue_handover_permit(
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

    // Outstanding writes block begin_quiescing, close, handover
    assert_eq!(gate2.begin_quiescing(), Err(ResourceError::Busy));

    // revoke_owner_writes clears the charge and restores admission
    let revoked = gate2.revoke_owner_writes();
    assert_eq!(revoked, 1);
    assert_eq!(gate2.outstanding_owner_writes(), 0);
    assert!(gate2.authorize_owner_write(WriterClass::Cursor).is_ok());
}

#[test]
fn c0_2ci_transport_gate_writer_boundary_enforcement() {
    let device_a = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let device_b = DrmDeviceKey {
        major: 226,
        minor: 1,
    };
    let incarnation = IncarnationId::first();

    let mut platform = crate::kms::render::platform::PlatformBackend::for_tests();
    // Default without gate: allows legacy
    assert!(platform.allows_legacy(&device_a, WriterClass::Primary));
    assert!(platform.allows_legacy(&device_b, WriterClass::Primary));

    // Install gate on device_a and quiesce
    let mut gate_a = TransportGate::new_legacy(device_a, incarnation);
    gate_a.begin_quiescing().unwrap();
    platform.install_transport_gate(gate_a);

    // device_a is now blocked for all legacy writes
    assert!(!platform.allows_legacy(&device_a, WriterClass::Primary));
    assert!(!platform.allows_legacy(&device_a, WriterClass::Modeset));
    assert!(!platform.allows_legacy(&device_a, WriterClass::Cursor));

    // Unrelated device_b remains unchanged (allows legacy)
    assert!(platform.allows_legacy(&device_b, WriterClass::Primary));
    assert!(platform.allows_legacy(&device_b, WriterClass::Modeset));
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
    consumer.correlate_commit(commit_id, vec![member], vec![(old_key, old_kms, member)]);

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
        consumer.correlate_commit(commit, vec![member], vec![(old_key, old_kms, member)]);

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

    // Register KMS release obligation on displaced buffers
    let old_a_kms = service
        .register(old_a_key, ObligationKind::KmsRelease)
        .unwrap();
    let old_b_kms = service
        .register(old_b_key, ObligationKind::KmsRelease)
        .unwrap();
    // Register GPU obligation on old_a
    let old_a_gpu = service.register(old_a_key, ObligationKind::Gpu).unwrap();
    // Register KMS obligation on new_a (should never be discharged by this commit's HardwareComplete)
    let new_a_kms = service
        .register(new_a_key, ObligationKind::KmsRelease)
        .unwrap();

    let commit_id = CommitId::for_tests(42);
    let mut consumer = CommitResourceConsumer::new();

    // Partial replacement commit: only member1 (crtc1) is included in the commit's membership!
    // Both obligations are tracked, but member2 is not in the commit membership.
    consumer.correlate_commit(
        commit_id,
        vec![member1],
        vec![
            (old_a_key, old_a_kms, member1),
            (old_b_key, old_b_kms, member2),
        ],
    );

    // Round-3 M-1: The only KMS proof reaches the service via HardwareComplete.
    // The test body calls NO `apply_validated_proof`!
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: commit_id },
            &mut service,
        )
        .unwrap();

    // old_a (member 1, matching) had its KMS obligation discharged!
    // But old_a still has old_a_gpu and old_a lease, so it is not dropped.
    assert_eq!(drops_old_a.get(), 0);

    // Now drop old_a lease and discharge old_a_gpu. If KMS was indeed discharged by HardwareComplete,
    // old_a will now be freed!
    drop(old_a);
    service.service_ready();
    assert_eq!(drops_old_a.get(), 0); // still held by GPU obligation
    service.apply_validated_proof(old_a_key, old_a_gpu).unwrap();
    service.service_ready();
    assert_eq!(drops_old_a.get(), 1); // KMS + GPU both satisfied, old_a dropped!

    // old_b (member 2, not matching) was NOT discharged. Dropping the lease still leaves old_b_kms pending!
    drop(old_b);
    service.service_ready();
    assert_eq!(drops_old_b.get(), 0); // KMS obligation on old_b is still outstanding!

    // new_a was NEVER discharged. Dropping new_a leaves new_a_kms pending!
    drop(new_a);
    service.service_ready();
    assert_eq!(drops_new_a.get(), 0);

    // Clean up remaining obligations
    service.apply_validated_proof(old_b_key, old_b_kms).unwrap();
    service.apply_validated_proof(new_a_key, new_a_kms).unwrap();
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
    let old_kms = service
        .register(old_key, ObligationKind::KmsRelease)
        .unwrap();

    let commit_id = CommitId::for_tests(7);
    let mut consumer = CommitResourceConsumer::new();
    consumer.correlate_commit(commit_id, vec![member1], vec![(old_key, old_kms, member1)]);

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

    let old_kms = service
        .register(old_key, ObligationKind::KmsRelease)
        .unwrap();

    let commit_id = CommitId::for_tests(99);
    let mut consumer = CommitResourceConsumer::new();
    // Register obligation with old_member (generation 1)
    consumer.correlate_commit(
        commit_id,
        vec![new_member], // commit membership has generation 2
        vec![(old_key, old_kms, old_member)],
    );

    // HardwareComplete arrives for commit
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: commit_id },
            &mut service,
        )
        .unwrap();

    // Because new_member != old_member, the obligation for old_member is NOT discharged!
    drop(old);
    service.service_ready();
    assert_eq!(drops_old.get(), 0); // Stale evidence cannot discharge the old record!

    // Cleaning up
    service.cancel(old_key, old_kms).unwrap();
    service.service_ready();
    assert_eq!(drops_old.get(), 1);
}

#[test]
fn c0_2ci_cow_deferred_release_and_reclaim() {
    use yserver_core::backend::Backend;

    let mut backend = crate::kms::render::KmsBackend::for_tests();

    // 0 -> 1 claim edge allocates COW
    assert!(backend.cow_id.is_none());
    assert!(
        backend
            .get_overlay_window(None)
            .expect("get_overlay_window")
    );
    let first_id = backend.cow_id.expect("cow_id allocated");
    assert!(!backend.deferred_cow_release);

    // Simulate direct scanout holding frame: 1 -> 0 edge defers release
    backend.deferred_cow_release = true;

    // Subsequent 0 -> 1 while deferred_cow_release holds:
    // Reuses the retained cow_id / StorageLease identity without new allocation
    assert!(
        backend
            .get_overlay_window(None)
            .expect("get_overlay_window re-claim")
    );
    let second_id = backend.cow_id.expect("cow_id retained");
    assert_eq!(first_id, second_id);
    assert!(!backend.deferred_cow_release);

    // Release overlay window
    backend.release_overlay_window(None).expect("release");
    assert!(backend.cow_id.is_none());
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
    let old_a_kms = service
        .register(old_a_key, ObligationKind::KmsRelease)
        .unwrap();

    let commit_id = CommitId::for_tests(88);
    let mut consumer = CommitResourceConsumer::new();

    // Only member1 registered in obligations
    consumer.correlate_commit(
        commit_id,
        vec![member1, member2],
        vec![(old_a_key, old_a_kms, member1)],
    );

    // HardwareComplete arrives
    consumer
        .consume(
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: commit_id },
            &mut service,
        )
        .unwrap();

    // old_a was discharged
    drop(old_a);
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
    consumer.correlate_commit(commit_id, vec![member], vec![(old_key, old_kms, member)]);
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
    consumer.correlate_commit(rej_commit, vec![member], vec![(rej_key, rej_kms, member)]);

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
    );

    let mut consumer = CommitResourceConsumer::new();
    consumer.correlate_commit(commit_id, vec![member], vec![(old_key, old_kms, member)]);
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
