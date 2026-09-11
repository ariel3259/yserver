use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use ash::vk;

use super::{
    AllocationPayload, CommitResourceConsumer, CommitResources, CompletionIngress,
    CoreRetirementBatch, DeviceBarrier, DirectCapacity, DirectRole, DrmCleanupRegistry,
    DrmDeviceKey, GroupMember, IncarnationBundle, IncarnationId, ObligationKind, ResourceError,
    ResourceService, RetainingSupervisor, UseKind,
    gpu::GpuObligation,
    storage::StorageBacking,
    tests::{MockCleanupIo, spy_service},
};
use crate::kms::{
    owner::{
        device::{DeviceCommitOwner, OwnerEvent},
        identity::CommitId,
        lifecycle::LifecycleEpochId,
    },
    render::{
        PlatformBackend,
        platform::CrtcKey,
        store::{ImportedDmabufMetadata, ImportedDmabufPlane, Storage},
    },
    vk::device::VkContext,
};

fn crtc_key(major: u32, minor: u32, handle: u32) -> CrtcKey {
    CrtcKey::new(
        DrmDeviceKey { major, minor },
        ::drm::control::crtc::Handle::from(std::num::NonZeroU32::new(handle).unwrap()),
    )
}

fn live_platform() -> Option<PlatformBackend> {
    let mut p = PlatformBackend::for_tests();
    let vk = match VkContext::new() {
        Ok(v) => v,
        Err(_) => return None,
    };
    let ops_pool = match crate::kms::vk::ops::OpsCommandPool::new(Arc::clone(&vk)) {
        Ok(o) => o,
        Err(_) => return None,
    };
    let fence_pool = crate::kms::render::platform::FencePool::new(Arc::clone(&vk));
    p.vk = Some(vk);
    p.ops_command_pool = Some(ops_pool);
    p.fence_pool = Some(fence_pool);
    Some(p)
}

// ── 1. Native, imported and promoted storage ────────────────────────────────
#[test]
fn c0_2ci_adapter_storage_native_imported_promoted_lifecycle() {
    let dev = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let inc = IncarnationId::first();
    let mut service = ResourceService::new(dev, inc);

    // Native storage allocation
    let native_storage = Storage::for_tests_null(
        vk::Extent2D {
            width: 100,
            height: 100,
        },
        vk::Format::B8G8R8A8_UNORM,
    );
    let StorageBacking::Legacy(native_alloc) = native_storage.backing else {
        panic!("expected legacy storage");
    };
    assert!(!native_alloc.is_exportable());

    let native_lease = service
        .adopt(AllocationPayload::Storage(native_alloc))
        .unwrap();
    let native_key = native_lease.key();

    // Register KMS obligation
    let kms_ob = service
        .register(native_key, ObligationKind::KmsRelease)
        .unwrap();

    // Simulate drawable destruction by dropping the lease while KMS obligation is pending
    drop(native_lease);
    service.service_ready();
    assert!(
        service.contains(&native_key),
        "native storage must remain retained while KMS obligation is pending"
    );

    // Fulfilling the KMS obligation terminalizes the native allocation exactly once
    service.apply_validated_proof(native_key, kms_ob).unwrap();
    service.service_ready();
    assert!(
        !service.contains(&native_key),
        "native storage freed once KMS obligation is satisfied"
    );

    // Imported storage allocation
    let mut imported_storage = Storage::for_tests_null(
        vk::Extent2D {
            width: 200,
            height: 200,
        },
        vk::Format::B8G8R8A8_UNORM,
    );
    if let StorageBacking::Legacy(ref mut alloc) = imported_storage.backing {
        alloc.imported_dmabuf = Some(ImportedDmabufMetadata {
            fourcc: u32::from_le_bytes(*b"AR24"),
            vk_format: vk::Format::B8G8R8A8_UNORM,
            modifier: 0,
            implicit_layout: false,
            planes: vec![ImportedDmabufPlane {
                offset: 0,
                pitch: 800,
            }],
            width: 200,
            height: 200,
            depth: 24,
            bpp: 32,
        });
        alloc.promoted_exportable = true;
    }
    let StorageBacking::Legacy(imported_alloc) = imported_storage.backing else {
        panic!("expected legacy storage");
    };
    assert!(imported_alloc.is_exportable());
    let imported_lease = service
        .adopt(AllocationPayload::Storage(imported_alloc))
        .unwrap();
    let imported_key = imported_lease.key();
    drop(imported_lease);
    assert_eq!(service.service_ready(), vec![imported_key]);

    // Promoted storage allocation
    let mut promoted_storage = Storage::for_tests_null(
        vk::Extent2D {
            width: 300,
            height: 300,
        },
        vk::Format::B8G8R8A8_UNORM,
    );
    if let StorageBacking::Legacy(ref mut alloc) = promoted_storage.backing {
        alloc.promoted_exportable = true;
        alloc.export_stride = 1200;
        alloc.export_size = 360000;
    }
    let StorageBacking::Legacy(promoted_alloc) = promoted_storage.backing else {
        panic!("expected legacy storage");
    };
    assert!(promoted_alloc.is_exportable());
    let promoted_lease = service
        .adopt(AllocationPayload::Storage(promoted_alloc))
        .unwrap();
    let promoted_key = promoted_lease.key();
    drop(promoted_lease);
    assert_eq!(service.service_ready(), vec![promoted_key]);
}

// ── 2. Old layout during relayout/promotion ─────────────────────────────────
#[test]
fn c0_2ci_adapter_old_layout_during_relayout_promotion() {
    let (mut service, old_lease, old_drops) = spy_service();
    let old_key = old_lease.key();

    // Read use active on old layout
    let read_use = service.reserve(old_key, UseKind::Read).unwrap();

    // Attempting exclusive Write use during relayout fails with Busy
    assert_eq!(
        service.reserve(old_key, UseKind::Write).unwrap_err(),
        ResourceError::Busy
    );

    // Old lease survives while read is active
    drop(old_lease);
    service.service_ready();
    assert_eq!(old_drops.get(), 0);

    // Finishing read use allows destruction of old layout
    drop(read_use);
    service.service_ready();
    assert_eq!(old_drops.get(), 1);
}

// ── 3. Shared BO and copied source/sink pair ────────────────────────────────
#[test]
fn c0_2ci_adapter_shared_bo_and_copied_source_sink_order() {
    let (mut service, shared_lease, shared_drops) = spy_service();
    let shared_key = shared_lease.key();

    let (mut sink_service, sink_lease, sink_drops) = spy_service();
    let sink_key = sink_lease.key();

    // Register KMS obligation on sink, GPU obligation on shared source
    let sink_kms = sink_service
        .register(sink_key, ObligationKind::KmsRelease)
        .unwrap();
    let shared_gpu = service.register(shared_key, ObligationKind::Gpu).unwrap();

    // Drops leases
    drop(shared_lease);
    drop(sink_lease);

    // Neither is freed yet
    service.service_ready();
    sink_service.service_ready();
    assert_eq!(shared_drops.get(), 0);
    assert_eq!(sink_drops.get(), 0);

    // Completing KMS on sink does NOT free shared source
    sink_service
        .apply_validated_proof(sink_key, sink_kms)
        .unwrap();
    sink_service.service_ready();
    assert_eq!(sink_drops.get(), 1);
    assert_eq!(shared_drops.get(), 0);

    // Completing GPU on shared source frees it
    service
        .apply_validated_proof(shared_key, shared_gpu)
        .unwrap();
    service.service_ready();
    assert_eq!(shared_drops.get(), 1);
}

// ── 4. Root snapshot then scratch Composite ─────────────────────────────────
#[test]
fn c0_2ci_adapter_root_snapshot_scratch_composite() {
    let (mut service, src_lease, src_drops) = spy_service();
    let src_key = src_lease.key();
    let (mut scratch_service, scratch_lease, scratch_drops) = spy_service();
    let scratch_key = scratch_lease.key();

    // Read on source ends once CPU/GPU copy to scratch finishes
    let read_use = service.reserve(src_key, UseKind::Read).unwrap();
    let scratch_gpu = scratch_service
        .register(scratch_key, ObligationKind::Gpu)
        .unwrap();

    // Source read completes
    drop(read_use);
    drop(src_lease);
    service.service_ready();
    // Source is freed at CPU/GPU copy completion
    assert_eq!(src_drops.get(), 1);

    // Scratch remains retained for GPU composite execution
    drop(scratch_lease);
    scratch_service.service_ready();
    assert_eq!(scratch_drops.get(), 0);

    // When GPU finishes, scratch frees once
    scratch_service
        .apply_validated_proof(scratch_key, scratch_gpu)
        .unwrap();
    scratch_service.service_ready();
    assert_eq!(scratch_drops.get(), 1);
}

// ── 5. Uncertain GPU/read submit ───────────────────────────────────────────
#[test]
fn c0_2ci_adapter_uncertain_gpu_read_submit_retention() {
    let (mut service, lease, drops) = spy_service();
    let key = lease.key();

    // Register obligation, then freeze upon uncertain submission
    let gpu_ob = service.register(key, ObligationKind::Gpu).unwrap();
    service.freeze(key).unwrap();
    drop(lease);

    // Normal proof does not unfreeze quarantined allocation
    service.apply_validated_proof(key, gpu_ob).unwrap();
    service.service_ready();
    assert_eq!(drops.get(), 0, "frozen entry remains retained");

    // Must not be freed or retried
    assert!(service.contains(&key));
}

// ── 6. VT-away / DPMS-off / idle scene ──────────────────────────────────────
#[test]
fn c0_2ci_adapter_vt_away_dpms_off_idle_service_progress() {
    let (mut dummy_service, dummy_lease, dummy_drops) = spy_service();
    let dummy_key = dummy_lease.key();
    let dummy_gpu = dummy_service
        .register(dummy_key, ObligationKind::Gpu)
        .unwrap();

    let ticket = crate::kms::render::platform::FenceTicket::for_tests_unsignaled_stub();
    let mut batch = CoreRetirementBatch::new(vec![dummy_lease], vec![0], true);
    batch.bind_ticket(GpuObligation::for_tests_stub(
        vec![(dummy_key, dummy_gpu)],
        ticket,
    ));
    dummy_service.register_batch(batch);

    let start = Instant::now();
    dummy_service.max_serviced_duration = Duration::from_millis(50);

    // VT away pauses serviced elapsed time
    dummy_service.set_seat_active(false, start);
    let _ = dummy_service.service_completions(start + Duration::from_millis(500));
    assert_eq!(dummy_service.pending_batches().len(), 1);

    // Return to active seat
    dummy_service.set_seat_active(true, start + Duration::from_millis(500));
    let _ = dummy_service.service_completions(start + Duration::from_millis(530));
    let exp = dummy_service.service_completions(start + Duration::from_millis(570));
    // Expired serviced time quarantines batch without crashing
    assert_eq!(exp, Err(ResourceError::Frozen));
    assert_eq!(dummy_service.pending_batches().len(), 0);
    assert_eq!(dummy_drops.get(), 0);
}

// ── 7. Grouped A/B frame, reversed output evidence ──────────────────────────
#[test]
fn c0_2ci_adapter_grouped_frame_reversed_evidence() {
    let (mut service, old_a, drops_a) = spy_service();
    let (mut service_b, old_b, drops_b) = spy_service();
    let key_a = old_a.key();
    let key_b = old_b.key();

    let dev = service.device();
    let crtc_a = crtc_key(dev.major, dev.minor, 1);
    let crtc_b = crtc_key(dev.major, dev.minor, 2);
    let member_a = GroupMember::new(crtc_a, 1, 1);
    let member_b = GroupMember::new(crtc_b, 1, 1);
    let commit = CommitId::for_tests(701);

    let ob_a = service.register_kms(key_a, commit, member_a).unwrap();
    let ob_b = service_b.register_kms(key_b, commit, member_b).unwrap();

    drop(old_a);
    drop(old_b);

    // Reversed output evidence: CRTC B arrives before reference CRTC A
    service_b
        .record_kms_discharged(key_b, ob_b, commit, member_b)
        .unwrap();
    service_b.apply_validated_proof(key_b, ob_b).unwrap();
    service_b.service_ready();
    assert_eq!(drops_b.get(), 1);
    assert_eq!(drops_a.get(), 0);

    // CRTC A arrives later
    service
        .record_kms_discharged(key_a, ob_a, commit, member_a)
        .unwrap();
    service.apply_validated_proof(key_a, ob_a).unwrap();
    service.service_ready();
    assert_eq!(drops_a.get(), 1);
}

// ── 8. Rejection, accepted Skip and supersession ────────────────────────────
#[test]
fn c0_2ci_adapter_rejection_accepted_skip_supersession() {
    let (mut service, old_lease, old_drops) = spy_service();
    let old_key = old_lease.key();
    let dev = service.device();
    let crtc = crtc_key(dev.major, dev.minor, 1);
    let member = GroupMember::new(crtc, 1, 1);
    let commit = CommitId::for_tests(801);

    let kms_ob = service.register_kms(old_key, commit, member).unwrap();
    let mut consumer = CommitResourceConsumer::new();
    consumer.correlate_commit(commit, vec![member], vec![(old_key, kms_ob, member)]);

    let res = CommitResources::new(
        vec![old_lease],
        None,
        None,
        None,
        vec![member],
        vec![(old_key, kms_ob, member)],
    );

    // Rejection cancels obligations rather than discharging
    let event = OwnerEvent::ResourcesStillCurrent {
        commit,
        resources: vec![res],
    };
    consumer.consume(event, &mut service).unwrap();

    // Obligation is cancelled, so not pending
    assert!(!service.has_pending_obligations(&old_key));
    // Old lease is still held in current_resources after rejection
    service.service_ready();
    assert_eq!(old_drops.get(), 0);

    // Dropping consumer releases the current resources
    drop(consumer);
    service.service_ready();
    assert_eq!(old_drops.get(), 1);
}

// ── 9. Preparing failure and A/B/C-D-E burst ────────────────────────────────
#[test]
fn c0_2ci_adapter_preparing_failure_and_burst_capacity() {
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
    assert!(!capacity.can_enter_direct());

    // Seventh reservation rejected
    assert_eq!(
        capacity.reserve(DirectRole::Preparing).err().unwrap(),
        ResourceError::Busy
    );

    // Proven failure on preparing cancels role reservation
    let preparing_slot = held.remove(3); // DirectRole::Preparing
    assert!(capacity.cancel_reservation(preparing_slot).is_ok());
    assert_eq!(capacity.occupied(), 5);

    // Preparing is now available again
    let new_prep = capacity.reserve(DirectRole::Preparing).unwrap();
    assert_eq!(capacity.occupied(), 6);
    drop(new_prep);
}

// ── 10. Unflip with ordinary retirement occupied ────────────────────
#[test]
fn c0_2ci_adapter_unflip_ordinary_retirement_occupied() {
    let mut capacity = DirectCapacity::new();
    let mut current = capacity.reserve(DirectRole::Current).unwrap();
    let ordinary = capacity.reserve(DirectRole::OrdinaryRetirement).unwrap();
    let exit_res = capacity.reserve(DirectRole::ExitRetirement).unwrap();

    // OrdinaryRetirement is occupied; unflip uses pre-reserved ExitRetirement
    let result = capacity.move_into_reserved(&mut current, exit_res);
    assert!(result.is_ok());
    assert_eq!(current.role, DirectRole::ExitRetirement);

    // Clean up
    let _ = capacity.finish_role(current);
    let _ = capacity.finish_role(ordinary);
}

// ── 11. Unknown -> detach -> late reply -> helper reap ──────────────────────
#[test]
fn c0_2ci_adapter_unknown_detach_late_reply_reap() {
    let (service, lease, drops) = spy_service();
    let _key = lease.key();
    let dev = service.device();
    let inc = service.incarnation();

    let owner = DeviceCommitOwner::<CommitResources>::new(inc, LifecycleEpochId::first(), 1);
    let consumer = CommitResourceConsumer::new();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(calls);
    let drm = DrmCleanupRegistry::new_with_io(dev, inc, Box::new(io));
    let ingress = CompletionIngress::new();

    let bundle = IncarnationBundle::new(owner, service, consumer, drm, None, ingress);

    let mut supervisor = RetainingSupervisor::new();
    let slot = supervisor.reserve_slot(dev, inc);
    assert!(supervisor.router.transfer(slot, bundle).is_ok());

    // Deliver late descriptor
    let (r, w) = nix::unistd::pipe().unwrap();
    supervisor.router.deliver_descriptor(inc, r).unwrap();
    drop(w);

    // Family closed barrier issued upon complete helper reap
    {
        let bundle_ref = supervisor.router.get_bundle_mut(&inc).unwrap();
        bundle_ref
            .resources
            .record_device_barrier(DeviceBarrier::FileFamilyClosed(dev));
    }

    drop(lease);
    {
        let bundle_ref = supervisor.router.get_bundle_mut(&inc).unwrap();
        bundle_ref.resources.service_ready();
    }
    assert_eq!(drops.get(), 1);
}

// ── 12. Duplicate/stale evidence and aliasing ───────────────────────────────
#[test]
fn c0_2ci_adapter_duplicate_stale_evidence_aliasing() {
    let (mut service, lease, drops) = spy_service();
    let key = lease.key();
    let ob = service.register(key, ObligationKind::KmsRelease).unwrap();

    // First proof succeeds
    assert!(service.apply_validated_proof(key, ob).is_ok());

    // Replay / duplicate proof is rejected
    assert_eq!(
        service.apply_validated_proof(key, ob).err().unwrap(),
        ResourceError::InvalidProof
    );

    drop(lease);
    service.service_ready();
    // Exactly once destruction
    assert_eq!(drops.get(), 1);
}

// ── 13. Live Vulkan smoke ───────────────────────────────────────────────────
#[test]
#[ignore = "needs live Vulkan ICD"]
fn c0_2ci_live_lifetime_adapters_vulkan() {
    // R12: Environmental skip must be reported honestly, never fake a pass.
    let platform = match live_platform() {
        Some(p) => p,
        None => {
            panic!("environmental skip: no live Vulkan ICD available; not claiming pass");
        }
    };

    let dev = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let inc = IncarnationId::first();
    let mut service = ResourceService::new(dev, inc);

    // 1. Allocate native storage via real PlatformBackend + Vulkan
    let storage = platform
        .allocate_drawable_storage(64, 64, 32)
        .expect("allocate_drawable_storage");

    let StorageBacking::Legacy(storage_alloc) = storage.backing else {
        panic!("expected legacy storage allocation");
    };

    let lease = service
        .adopt(AllocationPayload::Storage(storage_alloc))
        .expect("adopt storage");
    let key = lease.key();

    // Retain managed lease, register GPU obligation
    let gpu_ob = service
        .register(key, ObligationKind::Gpu)
        .expect("register gpu");

    // Drop lease while GPU work is pending
    drop(lease);
    service.service_ready();
    assert!(
        service.contains(&key),
        "allocation must survive while GPU obligation is pending"
    );

    // Discharge GPU obligation and observe real cleanup
    service
        .apply_validated_proof(key, gpu_ob)
        .expect("apply proof");
    service.service_ready();
    assert!(
        !service.contains(&key),
        "allocation freed once GPU completes"
    );
}
