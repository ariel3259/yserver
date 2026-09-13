use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use ash::vk::{self, Handle};

use super::{
    AllocationPayload, CommitResourceConsumer, CommitResources, CompletionDisposition,
    CompletionIngress, CopiedSourceAllocation, CoreRetirementBatch, DeviceBarrier, DirectCapacity,
    DirectRole, DrmCleanupRegistry, DrmDeviceKey, FileOwnedBacking, GemOwner, GroupMember,
    IncarnationBundle, IncarnationId, ObligationKind, PresentDisposition, PresentKey,
    ResourceError, ResourceService, RetainingSupervisor, ScanoutAllocation, SharedBacking, UseKind,
    WriterClass,
    gpu::GpuObligation,
    storage::StorageBacking,
    tests::{CleanupCall, MockCleanupIo, open_test_render_node, owner_gate_for_tests, spy_service},
};
use crate::kms::{
    owner::{
        clock::ClockSample,
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
    let dev = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let inc = IncarnationId::first();
    let mut renderer_service = ResourceService::new(dev, inc);
    let mut display_service = ResourceService::new(dev, inc);

    let renderer_alloc = CopiedSourceAllocation::mock(
        ash::vk::Semaphore::null(),
        crate::kms::vk::scanout::TransferResources::empty(),
        crate::kms::vk::scanout::CopiedSourceOwnership::ForeignAwaitingSink,
    );
    let renderer_lease = renderer_service
        .adopt(AllocationPayload::CopiedSource(renderer_alloc))
        .unwrap();
    let renderer_key = renderer_lease.key();

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
    let display_lease = display_service
        .adopt(AllocationPayload::Scanout(display_alloc))
        .unwrap();
    let display_key = display_lease.key();

    // Register KMS obligation on display sink, GPU on both, FOREIGN return on renderer source
    let kms_disp = display_service
        .register(display_key, ObligationKind::KmsRelease)
        .unwrap();
    let gpu_disp = display_service
        .register(display_key, ObligationKind::Gpu)
        .unwrap();
    let foreign_rend = renderer_service
        .register(renderer_key, ObligationKind::ForeignReturn)
        .unwrap();
    let gpu_rend = renderer_service
        .register(renderer_key, ObligationKind::Gpu)
        .unwrap();

    // Drop leases: both allocations stay pinned by their outstanding obligations
    drop(display_lease);
    drop(renderer_lease);
    display_service.service_ready();
    renderer_service.service_ready();
    assert!(display_service.contains(&display_key));
    assert!(renderer_service.contains(&renderer_key));

    // KMS release on sink must NOT allow premature reuse of renderer or display
    display_service
        .apply_validated_proof(display_key, kms_disp)
        .unwrap();
    display_service.service_ready();
    assert!(display_service.contains(&display_key));
    assert!(renderer_service.contains(&renderer_key));
    assert!(matches!(
        renderer_service.reserve(renderer_key, UseKind::Write),
        Err(ResourceError::Busy)
    ));

    // Complete display GPU work: frees display sink allocation
    display_service
        .apply_validated_proof(display_key, gpu_disp)
        .unwrap();
    display_service.service_ready();
    assert!(!display_service.contains(&display_key));

    // Renderer source is STILL pinned by GPU and ForeignReturn dependencies
    assert!(renderer_service.contains(&renderer_key));
    assert!(matches!(
        renderer_service.reserve(renderer_key, UseKind::Write),
        Err(ResourceError::Busy)
    ));

    // Complete renderer GPU work: still pinned by ForeignReturn
    renderer_service
        .apply_validated_proof(renderer_key, gpu_rend)
        .unwrap();
    renderer_service.service_ready();
    assert!(renderer_service.contains(&renderer_key));
    assert!(matches!(
        renderer_service.reserve(renderer_key, UseKind::Write),
        Err(ResourceError::Busy)
    ));

    // Discharge FOREIGN return dependency: renderer source is now freed and eligible for reuse
    renderer_service
        .apply_validated_proof(renderer_key, foreign_rend)
        .unwrap();
    renderer_service.service_ready();
    assert!(!renderer_service.contains(&renderer_key));
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
//
// F4-B1: `GpuObligation.context` is `Option<Arc<VkContext>>` and
// `#[cfg(test)] for_tests_stub` builds one with `context: None` -- legal
// here because, as before, this test never lets a real ticket reach the
// device: `test_ticket_status` intercepts `ticket_status()` first. No live
// device is needed to construct the value any more, so this is deterministic
// again.
#[test]
fn c0_2ci_adapter_vt_away_dpms_off_idle_service_progress() {
    let (mut dummy_service, dummy_lease, dummy_drops) = spy_service();
    let dummy_key = dummy_lease.key();
    let dummy_gpu = dummy_service
        .register(dummy_key, ObligationKind::Gpu)
        .unwrap();

    let ticket = crate::kms::render::platform::FenceTicket::for_tests_stub();
    let mut batch = CoreRetirementBatch::new(
        vec![dummy_lease],
        vec![vk::DescriptorSet::from_raw(0)],
        true,
    );
    batch.bind_ticket(GpuObligation::for_tests_stub(
        vec![(dummy_key, dummy_gpu)],
        ticket,
    ));
    batch.test_ticket_status = Some(Ok(false));

    let start = Instant::now();
    // B-11: the budget must be set BEFORE registering -- the batch's
    // deadline is stamped from `max_serviced_duration` as of its own
    // registration, not re-read from a mutable field on every poll.
    dummy_service.max_serviced_duration = Duration::from_millis(50);
    dummy_service.register_batch(batch);

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
    let dev = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let inc = IncarnationId::first();
    let mut service = ResourceService::new(dev, inc);

    let shared_alloc = ScanoutAllocation::new(
        None,
        SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        ),
    );
    let shared_lease = service
        .adopt(AllocationPayload::Scanout(shared_alloc))
        .unwrap();
    let shared_key = shared_lease.key();

    let crtc_a = crtc_key(dev.major, dev.minor, 1);
    let crtc_b = crtc_key(dev.major, dev.minor, 2);
    let member_a = GroupMember::new(crtc_a, 1, 1);
    let member_b = GroupMember::new(crtc_b, 1, 1);
    let commit = CommitId::for_tests(701);

    let ob_a = service.register_kms(shared_key, commit, member_a).unwrap();
    let ob_b = service.register_kms(shared_key, commit, member_b).unwrap();

    let mut consumer = CommitResourceConsumer::new();
    let present_key = PresentKey::new(dev, inc, commit, 1);
    // Reference CRTC is CRTC 2 (crtc_b)
    consumer.record_present_disposition_with_reference(
        present_key,
        PresentDisposition::pending(),
        2,
    );

    let mut res = CommitResources::new(
        vec![shared_lease],
        None,
        None,
        None,
        vec![member_a, member_b],
        vec![(shared_key, ob_a, member_a), (shared_key, ob_b, member_b)],
    );
    res.commit_id = Some(commit);
    consumer.releasing_resources.push(res);

    // Reversed evidence: non-reference CRTC B hardware-completes first
    consumer.commit_members.insert(commit, vec![member_b]);
    consumer
        .consume(OwnerEvent::HardwareComplete { commit }, &mut service)
        .unwrap();
    consumer.on_available(&[shared_key], &mut service).unwrap();
    service.service_ready();

    // Shared source MUST be retained because CRTC A replacement has not yet completed
    assert!(
        service.contains(&shared_key),
        "shared source must survive while member_a replacement is incomplete"
    );

    // Presentation event arrives with clock samples for both CRTC 1 and CRTC 2
    let mut samples = std::collections::BTreeMap::new();
    samples.insert(
        1,
        ClockSample {
            msc: 100,
            ust: 1000,
        },
    );
    samples.insert(
        2,
        ClockSample {
            msc: 200,
            ust: 2000,
        },
    );
    consumer
        .consume(OwnerEvent::Presented { commit, samples }, &mut service)
        .unwrap();

    // Verify reference CRTC (CRTC 2) supplied the presentation sample
    let disp = consumer
        .present_dispositions
        .get(&present_key)
        .expect("present disposition");
    assert_eq!(disp.completion, CompletionDisposition::Emitted);
    assert_eq!(
        disp.sample,
        Some(ClockSample {
            msc: 200,
            ust: 2000,
        })
    );

    // Shared source still retained
    consumer.on_available(&[shared_key], &mut service).unwrap();
    service.service_ready();
    assert!(
        service.contains(&shared_key),
        "shared source still retained before member_a completion"
    );

    // CRTC A (reference CRTC) hardware-completes later
    consumer.commit_members.insert(commit, vec![member_a]);
    consumer
        .consume(OwnerEvent::HardwareComplete { commit }, &mut service)
        .unwrap();
    consumer.on_available(&[shared_key], &mut service).unwrap();
    service.service_ready();

    // All replacements finished: shared source is freed
    assert!(
        !service.contains(&shared_key),
        "shared source freed once all CRTC replacements finish"
    );
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

// ── 10. Unflip with ordinary retirement occupied ────────────────────────────
#[test]
fn c0_2ci_adapter_unflip_ordinary_retirement_occupied() {
    let dev = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let inc = IncarnationId::first();
    let mut service = ResourceService::new(dev, inc);
    let mut consumer = CommitResourceConsumer::new();

    // Allocation A: currently occupying OrdinaryRetirement (previous frame)
    let alloc_a = ScanoutAllocation::new(
        None,
        SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        ),
    );
    let lease_a = service.adopt(AllocationPayload::Scanout(alloc_a)).unwrap();
    let key_a = lease_a.key();
    let gpu_a = service.register(key_a, ObligationKind::Gpu).unwrap();
    let ord_slot = consumer
        .capacity
        .reserve(DirectRole::OrdinaryRetirement)
        .unwrap();
    let res_a = CommitResources::new(vec![lease_a], None, None, None, vec![], vec![]);
    let res_a = consumer.capacity.attach(ord_slot, res_a).unwrap();
    consumer.releasing_resources.push(res_a);

    // Allocation B: currently in Current role (direct scanout frame to be unflipped)
    let alloc_b = ScanoutAllocation::new(
        None,
        SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        ),
    );
    let lease_b = service.adopt(AllocationPayload::Scanout(alloc_b)).unwrap();
    let key_b = lease_b.key();
    let gpu_b = service.register(key_b, ObligationKind::Gpu).unwrap();
    let curr_slot = consumer.capacity.reserve(DirectRole::Current).unwrap();
    let res_b = CommitResources::new(vec![lease_b], None, None, None, vec![], vec![]);
    let res_b = consumer.capacity.attach(curr_slot, res_b).unwrap();
    consumer.current_resources.push(res_b);

    // Allocation C: composed return resource pre-allocated in service (no extra allocation at unflip time)
    let alloc_c = ScanoutAllocation::new(
        None,
        SharedBacking::mock(
            ash::vk::Image::null(),
            ash::vk::DeviceMemory::null(),
            ash::vk::ImageView::null(),
            crate::kms::vk::scanout::TransferResources::empty(),
            None,
        ),
    );
    let lease_c = service.adopt(AllocationPayload::Scanout(alloc_c)).unwrap();
    let key_c = lease_c.key();

    // Verify OrdinaryRetirement is occupied by Frame A
    assert!(!consumer.capacity.is_vacant(DirectRole::OrdinaryRetirement));

    // Unflip occurs: move Current (Frame B) into ExitRetirement via pre-reserved exit slot
    let exit_slot = consumer
        .capacity
        .reserve(DirectRole::ExitRetirement)
        .unwrap();
    let mut b_res = consumer.take_current().into_iter().next().unwrap();
    consumer
        .capacity
        .move_into_reserved(b_res.direct_role.as_mut().unwrap(), exit_slot)
        .unwrap();
    assert_eq!(
        b_res.direct_role.as_ref().unwrap().role(),
        DirectRole::ExitRetirement
    );
    consumer.releasing_resources.push(b_res);

    // Both retirement roles are now occupied
    assert!(!consumer.capacity.is_vacant(DirectRole::OrdinaryRetirement));
    assert!(!consumer.capacity.is_vacant(DirectRole::ExitRetirement));
    assert!(!consumer.capacity.can_enter_direct());

    // Composed return resource C is acquired for composed scanout without extra allocation
    let composed_write = service.reserve(key_c, UseKind::Write).unwrap();
    assert_eq!(composed_write.key(), key_c);

    // Fulfill GPU obligations for A and B
    service.apply_validated_proof(key_a, gpu_a).unwrap();
    service.apply_validated_proof(key_b, gpu_b).unwrap();

    consumer
        .on_available(&[key_a, key_b], &mut service)
        .unwrap();
    service.service_ready();

    // A and B freed; retirement roles are vacant; direct re-entry is permitted again
    assert!(!service.contains(&key_a));
    assert!(!service.contains(&key_b));
    assert!(consumer.capacity.is_vacant(DirectRole::OrdinaryRetirement));
    assert!(consumer.capacity.is_vacant(DirectRole::ExitRetirement));
    assert!(consumer.capacity.can_enter_direct());

    // Composed return resource remains valid in service
    drop(composed_write);
    drop(lease_c);
    service.service_ready();
    assert!(!service.contains(&key_c));
}

// ── 11. Unknown -> detach -> late reply -> helper reap ──────────────────────
#[test]
fn c0_2ci_adapter_unknown_detach_late_reply_reap() {
    let dev = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let inc = IncarnationId::first();
    let mut service = ResourceService::new(dev, inc);

    let storage = Storage::for_tests_null(
        vk::Extent2D {
            width: 64,
            height: 64,
        },
        vk::Format::B8G8R8A8_UNORM,
    );
    let StorageBacking::Legacy(alloc) = storage.backing else {
        panic!("expected legacy storage");
    };
    let lease = service.adopt(AllocationPayload::Storage(alloc)).unwrap();
    let _key = lease.key();

    let owner = DeviceCommitOwner::<CommitResources>::new(inc, LifecycleEpochId::first(), 1);
    let commit_res = CommitResources::new(vec![lease], None, None, None, vec![], vec![]);

    let mut consumer = CommitResourceConsumer::new();
    consumer.current_resources.push(commit_res);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let io = MockCleanupIo::new(calls);
    let drm = DrmCleanupRegistry::new_with_io(dev, inc, Box::new(io));
    let ingress = CompletionIngress::new();
    let mut gate = owner_gate_for_tests(dev, inc);
    let grant = gate.authorize_owner_write(WriterClass::Modeset).unwrap();

    let bundle = IncarnationBundle::new(owner, service, consumer, drm, None, ingress, gate);

    let mut supervisor = RetainingSupervisor::new();
    let slot = supervisor.reserve_slot(dev, inc);
    // Handoff revokes owner writes and transfers bundle
    assert!(supervisor.router.transfer(slot, bundle).is_ok());
    drop(grant);

    // Deliver late descriptor
    let (r, w) = nix::unistd::pipe().unwrap();
    supervisor.router.deliver_descriptor(inc, r).unwrap();
    drop(w);

    // Attempting to mint barrier while descriptor is open fails
    {
        let bundle_ref = supervisor.router.get_bundle_mut(&inc).unwrap();
        bundle_ref.drm.detach_fake_submitters();
        bundle_ref.drm.close_fake_control();
        bundle_ref.drm.reap_fake_helper();
        assert_eq!(
            bundle_ref
                .drm
                .try_mint_file_family_closed(|_, _| Ok(()))
                .err()
                .unwrap()
                .to_string(),
            "non-payload aliases still active"
        );
    }

    // Close returned descriptors and complete reap: barrier minting succeeds
    {
        let bundle_ref = supervisor.router.get_bundle_mut(&inc).unwrap();
        bundle_ref.drm.close_returned_descriptors();
        let closed = bundle_ref
            .drm
            .try_mint_file_family_closed(|_, _| Ok(()))
            .unwrap();
        let barrier = DeviceBarrier::from_file_family_closed(closed);
        bundle_ref.resources.record_device_barrier(barrier);
        bundle_ref.resources.service_ready();
    }

    // Recipient router services late evidence without panics
    assert!(supervisor.router.service(Instant::now()).is_ok());
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
    // Reset validation layer error/warning counters before smoke test
    crate::kms::vk::device::reset_validation_counts();

    // R12: Environmental skip must be reported honestly, never fake a pass.
    let mut platform = match live_platform() {
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
    let mut store = crate::kms::render::store::DrawableStore::new();
    let mut invalidations = 0;

    // 1. Native storage: allocate, adopt into managed lease, free drawable in store, observe cleanup
    let storage = platform
        .allocate_drawable_storage(64, 64, 32)
        .expect("allocate_drawable_storage");
    let lease = storage
        .into_managed(
            &mut service,
            &platform,
            crate::kms::render::target::PaintTarget::new(
                crate::kms::render::store::DrawableId::for_tests(0x2001),
                (0, 0),
                None,
                32,
            ),
            (0, 0),
        )
        .map_err(|(e, _)| e)
        .expect("into_managed");
    let key = lease.allocation.key();
    let gpu_ob = service
        .register(key, ObligationKind::Gpu)
        .expect("register gpu");
    let managed_storage = Storage::from_backing(StorageBacking::Managed(lease));
    let xid = 0x2001;
    let id = store
        .allocate(
            xid,
            crate::kms::render::store::DrawableKind::Pixmap,
            32,
            false,
            managed_storage,
        )
        .expect("allocate drawable");

    // Free drawable in store via decref: triggers cache invalidation callback
    let dec = store.decref(&mut platform, id, |_inv_id| {
        invalidations += 1;
    });
    assert_eq!(dec, crate::kms::render::store::RetireDecision::Destroyed);
    assert_eq!(
        invalidations, 1,
        "decref must trigger invalidation callback once"
    );
    assert!(store.lookup(xid).is_none());

    // Storage is destroyed by decref, but GPU obligation retains allocation in ResourceService
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

    // 2. Repeat for promoted backing
    let storage2 = platform
        .allocate_drawable_storage(64, 64, 32)
        .expect("allocate storage2");
    let lease2 = storage2
        .into_managed(
            &mut service,
            &platform,
            crate::kms::render::target::PaintTarget::new(
                crate::kms::render::store::DrawableId::for_tests(0x2002),
                (0, 0),
                None,
                32,
            ),
            (0, 0),
        )
        .map_err(|(e, _)| e)
        .expect("into_managed");
    let managed_storage2 = Storage::from_backing(StorageBacking::Managed(lease2));
    let xid2 = 0x2002;
    let id2 = store
        .allocate(
            xid2,
            crate::kms::render::store::DrawableKind::Pixmap,
            32,
            false,
            managed_storage2,
        )
        .expect("allocate managed drawable");

    let vk_ctx = platform.vk.clone().unwrap();
    let (exp_img, exp_mem, exp_view, exp_sample) = {
        use ash::vk;
        let img_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::B8G8R8A8_UNORM)
            .extent(vk::Extent3D {
                width: 64,
                height: 64,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED);
        let img = unsafe {
            vk_ctx
                .device
                .create_image(&img_info, None)
                .expect("create image")
        };
        let mem_req = unsafe { vk_ctx.device.get_image_memory_requirements(img) };
        let mem_props = unsafe {
            vk_ctx
                .instance
                .get_physical_device_memory_properties(vk_ctx.physical_device)
        };
        let type_idx = (0..mem_props.memory_type_count as usize)
            .find(|&i| (mem_req.memory_type_bits & (1 << i)) != 0)
            .expect("valid memory type");
        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(mem_req.size)
            .memory_type_index(type_idx as u32);
        let mem = unsafe {
            vk_ctx
                .device
                .allocate_memory(&alloc_info, None)
                .expect("alloc memory")
        };
        unsafe { vk_ctx.device.bind_image_memory(img, mem, 0).expect("bind") };
        let view_info = vk::ImageViewCreateInfo::default()
            .image(img)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::B8G8R8A8_UNORM)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let v1 = unsafe {
            vk_ctx
                .device
                .create_image_view(&view_info, None)
                .expect("view")
        };
        let v2 = unsafe {
            vk_ctx
                .device
                .create_image_view(&view_info, None)
                .expect("view2")
        };
        (img, mem, v1, v2)
    };

    let old_lease = store
        .get_mut(id2)
        .unwrap()
        .storage
        .adopt_exportable_managed(
            &mut service,
            exp_img,
            exp_mem,
            exp_sample,
            exp_view,
            ash::vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            256,
            16384,
            0,
            Some(Arc::clone(&vk_ctx)),
        )
        .expect("adopt_exportable_managed");
    let old_key = old_lease.allocation.key();
    let new_key = store
        .get(id2)
        .unwrap()
        .storage
        .managed_lease()
        .unwrap()
        .allocation
        .key();
    assert_ne!(old_key, new_key);

    let old_gpu = service.register(old_key, ObligationKind::Gpu).unwrap();
    let new_gpu = service.register(new_key, ObligationKind::Gpu).unwrap();

    drop(old_lease);
    let dec2 = store.decref(&mut platform, id2, |_| {
        invalidations += 1;
    });
    assert_eq!(dec2, crate::kms::render::store::RetireDecision::Destroyed);
    assert_eq!(invalidations, 2);

    service.service_ready();
    assert!(service.contains(&old_key));
    assert!(service.contains(&new_key));

    service.apply_validated_proof(old_key, old_gpu).unwrap();
    service.service_ready();
    assert!(!service.contains(&old_key));
    assert!(service.contains(&new_key));

    service.apply_validated_proof(new_key, new_gpu).unwrap();
    service.service_ready();
    assert!(!service.contains(&new_key));

    // 3. Repeat for snapshot scratch
    let scratch_storage = platform
        .allocate_drawable_storage(64, 64, 32)
        .expect("allocate scratch");
    let StorageBacking::Legacy(scratch_alloc) = scratch_storage.backing else {
        panic!("expected legacy storage");
    };
    let scratch_lease = service
        .adopt(AllocationPayload::Storage(scratch_alloc))
        .expect("adopt scratch");
    let scratch_key = scratch_lease.key();
    let scratch_read = service.reserve(scratch_key, UseKind::Read).unwrap();
    let scratch_gpu = service.register(scratch_key, ObligationKind::Gpu).unwrap();

    // Source read ends at CPU copy
    drop(scratch_read);
    drop(scratch_lease);
    service.service_ready();
    assert!(
        service.contains(&scratch_key),
        "scratch retained through GPU use"
    );

    // Scratch GPU use finishes and frees once
    service
        .apply_validated_proof(scratch_key, scratch_gpu)
        .unwrap();
    service.service_ready();
    assert!(!service.contains(&scratch_key), "scratch freed");

    // 4. If render node is available, include gbm_bo in the _vulkan case
    if let Some(drm_dev) = open_test_render_node() {
        let drm_dev = Rc::new(drm_dev);
        let gbm_dev = gbm::Device::new(Rc::clone(&drm_dev)).expect("gbm device");
        let gbm_bo = gbm_dev
            .create_buffer_object::<()>(
                64,
                64,
                gbm::Format::Xrgb8888,
                gbm::BufferObjectFlags::RENDERING | gbm::BufferObjectFlags::SCANOUT,
            )
            .expect("gbm bo");

        let calls = Rc::new(RefCell::new(Vec::new()));
        let io = MockCleanupIo::new(Rc::clone(&calls));
        let mut registry =
            DrmCleanupRegistry::new_with_device_and_io(Rc::clone(&drm_dev), dev, inc, Box::new(io));
        registry.detach_fake_submitters();
        registry.reap_fake_helper();
        registry.close_fake_control();

        let right = registry.register_right(2001, 2002, GemOwner::Gbm);
        let fo = FileOwnedBacking::new(right, Some(gbm_bo), Rc::clone(&drm_dev))
            .expect("file owned backing");

        // Allocate real Vulkan image for SharedBacking
        let shared_img = {
            use ash::vk;
            let img_info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::B8G8R8A8_UNORM)
                .extent(vk::Extent3D {
                    width: 64,
                    height: 64,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED);
            unsafe {
                vk_ctx
                    .device
                    .create_image(&img_info, None)
                    .expect("create image")
            }
        };
        let shared_mem_req = unsafe { vk_ctx.device.get_image_memory_requirements(shared_img) };
        let mem_props = unsafe {
            vk_ctx
                .instance
                .get_physical_device_memory_properties(vk_ctx.physical_device)
        };
        let type_idx = (0..mem_props.memory_type_count as usize)
            .find(|&i| (shared_mem_req.memory_type_bits & (1 << i)) != 0)
            .expect("valid memory type");
        let alloc_info = ash::vk::MemoryAllocateInfo::default()
            .allocation_size(shared_mem_req.size)
            .memory_type_index(type_idx as u32);
        let shared_mem = unsafe {
            vk_ctx
                .device
                .allocate_memory(&alloc_info, None)
                .expect("alloc memory")
        };
        unsafe {
            vk_ctx
                .device
                .bind_image_memory(shared_img, shared_mem, 0)
                .expect("bind")
        };
        let view_info = ash::vk::ImageViewCreateInfo::default()
            .image(shared_img)
            .view_type(ash::vk::ImageViewType::TYPE_2D)
            .format(ash::vk::Format::B8G8R8A8_UNORM)
            .subresource_range(ash::vk::ImageSubresourceRange {
                aspect_mask: ash::vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let shared_view = unsafe {
            vk_ctx
                .device
                .create_image_view(&view_info, None)
                .expect("view")
        };

        let shared = SharedBacking::new(
            shared_img,
            shared_mem,
            shared_view,
            crate::kms::vk::scanout::TransferResources::empty(),
            Arc::clone(&vk_ctx),
            None,
        );
        let scanout_alloc = ScanoutAllocation::new(Some(fo), shared);
        let scanout_lease = service
            .adopt_with_registry(AllocationPayload::Scanout(scanout_alloc), &mut registry)
            .expect("adopt scanout with gbm_bo");
        let scanout_key = scanout_lease.key();
        let scanout_gpu = service.register(scanout_key, ObligationKind::Gpu).unwrap();

        drop(scanout_lease);
        drop(gbm_dev);
        drop(drm_dev);
        service.service_ready();
        assert!(service.contains(&scanout_key));

        // Discharge file-owned half through registry (step 2): drops gbm_bo
        let proof = registry
            .try_mint_file_family_closed(|reg, discharge_key| {
                assert_eq!(discharge_key, scanout_key);
                let entry = service.entries.get(&discharge_key).expect("entry present");
                let mut payload = entry.payload.borrow_mut();
                match payload.as_mut() {
                    Some(AllocationPayload::Scanout(alloc)) => alloc.discharge_file_owned(reg),
                    _ => Ok(()),
                }
            })
            .expect("mint file family closed");

        // Table order: GEM closer is gbm_bo drop, registry sees 0 CloseGem calls
        assert_eq!(
            calls
                .borrow()
                .iter()
                .filter(|c| matches!(c, CleanupCall::CloseGem(_)))
                .count(),
            0,
            "gbm_bo must be the sole GEM closer; registry sees zero CloseGem calls"
        );

        service.record_device_barrier(DeviceBarrier::from_file_family_closed(proof));
        service.service_ready();
        // Still alive because GPU obligation remains
        assert!(service.contains(&scanout_key));

        // Discharge shared GPU obligation to observe VkImage cleanup (step 3+)
        service
            .apply_validated_proof(scanout_key, scanout_gpu)
            .unwrap();
        service.service_ready();
        assert!(!service.contains(&scanout_key));
    }

    // 5. Assert zero validation layer messages
    assert_eq!(
        crate::kms::vk::device::validation_error_count(),
        0,
        "must have zero Vulkan validation layer errors"
    );
    assert_eq!(
        crate::kms::vk::device::validation_warning_count(),
        0,
        "must have zero Vulkan validation layer warnings"
    );
}

// ── 14. Managed scanout: consuming conversion + BoPhase ownership (F-2, B-13) ──
//
// `ScanoutBo.vk` is `Arc<VkContext>`, not `Option`, so even a bo whose
// image/memory/view are all `vk::Image::null()` etc. (this fixture never
// allocates a real image) needs a real Vulkan context to exist at all --
// hence `_vulkan`, though no Vulkan call is ever actually issued: every
// destroy in `SharedBacking::drop` is guarded on the handle being non-null.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan() {
    use crate::kms::vk::scanout::{BoPhase, OutputScanout, ScanoutBo, ScanoutBoPool};

    let mut platform = match live_platform() {
        Some(p) => p,
        None => panic!("environmental skip: no live Vulkan ICD available; not claiming pass"),
    };
    let vk = platform.vk.clone().expect("live_platform installs vk");

    let dev = DrmDeviceKey {
        major: 226,
        minor: 0,
    };
    let inc = IncarnationId::first();
    let mut service = ResourceService::new(dev, inc);
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut registry =
        DrmCleanupRegistry::new_with_io(dev, inc, Box::new(MockCleanupIo::new(Rc::clone(&calls))));

    let bo_drm = Rc::new(crate::drm::Device::for_tests().expect("test drm device"));
    let mut bo = ScanoutBo::for_tests(bo_drm, vk);
    bo.fb_handle = Some(::drm::control::framebuffer::Handle::from(
        std::num::NonZeroU32::new(9001).unwrap(),
    ));
    bo.gem_handle = Some(::drm::buffer::Handle::from(
        std::num::NonZeroU32::new(9002).unwrap(),
    ));

    let mut pool = ScanoutBoPool::for_tests();
    // Match `PlatformBackend::for_tests()`'s own output route so
    // `cancel_scanout_bo_recording`'s `debug_assert_scanout_pool_route`
    // holds; `pool.route` is `pub(crate)`, unlike `ActiveOutput`'s private
    // field, so the pool is what moves to match the output here.
    pool.route = crate::kms::scanout_route::ScanoutRoute::new(
        crate::kms::scanout_route::RenderDeviceId::UnverifiedFallback,
        DrmDeviceKey { major: 0, minor: 0 },
        crate::kms::scanout_route::RenderKmsRelationship::Unknown,
    );
    pool.bos.push(bo);
    platform.scanout_pools[0] = Some(OutputScanout::Shared(pool));
    platform.bo_generations[0] = vec![Default::default()];

    // Legacy acquire sees the Free bo before conversion.
    assert!(platform.acquire_scanout_bo(0).is_some());

    // B-13: register_managed_scanout_bo performs the actual consuming
    // extraction (not just a `managed_key` tag on a bo that still owns its
    // resources) and adopts the result as a real `ScanoutAllocation`.
    let display_key = platform
        .register_managed_scanout_bo(&mut service, &mut registry, 0, 0)
        .expect("register managed scanout bo");

    // F2-B1: the pool slot must root the Retain lease, not a bare key --
    // otherwise the entry has zero live uses right after registration, is
    // already dirty (the dropped lease marked it), and the very next tick
    // discharges and destroys it while the pool slot still lists the husk
    // as managed and possibly scanning out.
    service.service_ready_with_registry(&mut registry);
    assert!(
        service.contains(&display_key),
        "F2-B1: a freshly registered managed bo must not be swept by the next tick"
    );
    assert!(calls.borrow().is_empty());

    let output = CrtcKey::for_output(&platform.outputs[0]);
    let token = platform
        .acquire_managed_scanout_bo(&mut service, output)
        .expect("acquire managed scanout bo");
    assert_eq!(token.display.key(), display_key);

    // B-13: acquire_managed_scanout_bo must own the BoPhase transition, so
    // legacy acquire_scanout_bo can no longer hand out the same slot while
    // this managed token is live.
    assert!(platform.acquire_scanout_bo(0).is_none());

    // Done proving the token itself; drop its Write-kind lease so it is not
    // an extra live use blocking the destroy proof below.
    drop(token);

    // 4.5: cancel_scanout_bo_recording ends recording but not in-flight
    // work -- a registered GPU obligation on the managed key survives it.
    let gpu_ob = service
        .register(display_key, ObligationKind::Gpu)
        .expect("register gpu");
    platform.cancel_scanout_bo_recording(0, 0);
    assert!(service.has_pending_obligation(&display_key, gpu_ob));
    match platform.scanout_pools[0].as_ref().unwrap() {
        OutputScanout::Shared(p) => assert_eq!(p.bos[0].state.phase, BoPhase::Free),
        OutputScanout::Copied(_) => panic!("expected Shared pool"),
    }

    // No ioctl was ever issued against the mock transport: the husk left by
    // the consuming extraction is inert, and cancel is pool-bookkeeping
    // only.
    assert!(calls.borrow().is_empty());

    // Topology reuse: detach clears the real `managed_key` this test
    // registered, not just an already-empty pool's (nonexistent) entries.
    platform.scanout_pools[0]
        .as_mut()
        .unwrap()
        .detach_managed_entries();
    match platform.scanout_pools[0].as_ref().unwrap() {
        OutputScanout::Shared(p) => assert_eq!(p.bos[0].managed_key(), None),
        OutputScanout::Copied(_) => panic!("expected Shared pool"),
    }

    // F2-B1: detach is what actually drops the lease -- only *now* does the
    // entry become destroyable, discharging the still-outstanding GPU
    // obligation notwithstanding (F2-B1's test asks for the right's GEM
    // disposition; the still-pending GPU obligation from the cancel step
    // above additionally proves detach alone does not bypass ordinary
    // availability gating).
    service.apply_validated_proof(display_key, gpu_ob).unwrap();
    service.service_ready_with_registry(&mut registry);
    assert_eq!(
        calls.borrow().as_slice(),
        &[CleanupCall::RemoveFb(9001), CleanupCall::CloseGem(9002)]
    );
    assert!(!service.contains(&display_key));

    // F2b-m1: register_managed_scanout_bo's pre-extraction Exhausted guard
    // (F2-M2) had no end-to-end test -- only the mid-function
    // release_fresh_adoption rollback (the renderer-adoption-fails-after-
    // display-succeeds case) was covered. This drives a SECOND bo into
    // the Exhausted branch and asserts it comes back exactly as it went
    // in: not a partially emptied husk with nowhere for its resources to
    // go.
    let mut second_bo = ScanoutBo::for_tests(
        Rc::new(crate::drm::Device::for_tests().expect("test drm device")),
        platform.vk.clone().expect("live_platform installs vk"),
    );
    let second_fb =
        ::drm::control::framebuffer::Handle::from(std::num::NonZeroU32::new(9101).unwrap());
    let second_gem = ::drm::buffer::Handle::from(std::num::NonZeroU32::new(9102).unwrap());
    second_bo.fb_handle = Some(second_fb);
    second_bo.gem_handle = Some(second_gem);
    match platform.scanout_pools[0].as_mut().unwrap() {
        OutputScanout::Shared(p) => p.bos.push(second_bo),
        OutputScanout::Copied(_) => panic!("expected Shared pool"),
    }
    platform.bo_generations[0].push(Default::default());

    let aliases_before = registry.payload_aliases();
    let calls_before = calls.borrow().len();
    service.force_exhausted_for_tests();
    let err = platform
        .register_managed_scanout_bo(&mut service, &mut registry, 0, 1)
        .expect_err("exhausted service must refuse registration");
    assert_eq!(err, ResourceError::Exhausted);
    match platform.scanout_pools[0].as_ref().unwrap() {
        OutputScanout::Shared(p) => {
            assert_eq!(
                p.bos[1].fb_handle,
                Some(second_fb),
                "fb_handle must survive an exhausted registration attempt untouched"
            );
            assert_eq!(
                p.bos[1].gem_handle,
                Some(second_gem),
                "gem_handle must survive an exhausted registration attempt untouched"
            );
            assert_eq!(p.bos[1].managed_key(), None);
        }
        OutputScanout::Copied(_) => panic!("expected Shared pool"),
    }
    assert_eq!(
        registry.payload_aliases(),
        aliases_before,
        "an exhausted registration attempt must not register a new alias"
    );
    assert_eq!(
        calls.borrow().len(),
        calls_before,
        "an exhausted registration attempt must issue no ioctl"
    );
}
