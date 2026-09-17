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

fn closed_cell() -> Rc<Cell<bool>> {
    Rc::new(Cell::new(false))
}

/// census: C-retire-move-into-reserved commit.rs consume `let Err((err, recovered)) = self.capacity.move_into_reserved(role, reserved)`
#[test]
fn c0_2ci_guard_completion_retired_returns_failed_move_into_reserved() {
    let (mut service, old_alloc, _drops) = spy_service();
    let mut consumer = CommitResourceConsumer::new();
    let commit = CommitId::for_tests(920);
    // Never reserved in the consumer's capacity: move_into_reserved rejects it.
    consumer.prereserve_retirement(
        commit,
        RoleReservation::new_for_test(DirectRole::OrdinaryRetirement, 998, closed_cell()),
    );
    let old =
        CommitResources::new(vec![old_alloc], None, None, None, vec![], vec![]).with_direct_role(
            RoleReservation::new_for_test(DirectRole::Current, 999, closed_cell()),
        );
    let accepted = crate::kms::owner::ledger::Submitted::new(vec![old], vec![]).accepted();
    assert_eq!(
        consumer.consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit,
                resources: accepted,
            },
            &mut service,
        ),
        Err(ResourceError::InvalidState),
        "a failed move of the old Current into its reserved slot must be returned \
         [census:C-retire-move-into-reserved]"
    );
    assert!(consumer.capacity.is_admission_closed());
}

/// census: C-retire-move-into-ordinary-retirement commit.rs consume `self.capacity.is_vacant(DirectRole::OrdinaryRetirement) && let Err(err) = self .capacity .move_role(role, DirectRole::OrdinaryRetirement)`
#[test]
fn c0_2ci_guard_completion_retired_returns_failed_move_into_ordinary_retirement() {
    let (mut service, old_alloc, _drops) = spy_service();
    let mut consumer = CommitResourceConsumer::new();
    // No retirement slot pre-reserved for this commit and OrdinaryRetirement
    // vacant: consume takes the `else if` branch. The Current token was never
    // reserved in the consumer's capacity, so move_role rejects it.
    let old =
        CommitResources::new(vec![old_alloc], None, None, None, vec![], vec![]).with_direct_role(
            RoleReservation::new_for_test(DirectRole::Current, 999, closed_cell()),
        );
    let accepted = crate::kms::owner::ledger::Submitted::new(vec![old], vec![]).accepted();
    assert_eq!(
        consumer.consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: CommitId::for_tests(922),
                resources: accepted,
            },
            &mut service,
        ),
        Err(ResourceError::InvalidState),
        "a failed move of the old Current into a vacant OrdinaryRetirement must be returned \
         [census:C-retire-move-into-ordinary-retirement]"
    );
    assert!(consumer.capacity.is_admission_closed());
}

/// census: C-retire-submitted-to-current commit.rs consume `let Some(ref mut role) = res.direct_role && role.role == DirectRole::Submitted && let Err(err) = self.capacity.move_role(role, DirectRole::Current)`
#[test]
fn c0_2ci_guard_completion_retired_returns_failed_submitted_to_current() {
    let (mut service, new_alloc, _drops) = spy_service();
    let mut consumer = CommitResourceConsumer::new();
    let new =
        CommitResources::new(vec![new_alloc], None, None, None, vec![], vec![]).with_direct_role(
            RoleReservation::new_for_test(DirectRole::Submitted, 997, closed_cell()),
        );
    let accepted = crate::kms::owner::ledger::Submitted::new(vec![], vec![new]).accepted();
    assert_eq!(
        consumer.consume(
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: CommitId::for_tests(921),
                resources: accepted,
            },
            &mut service,
        ),
        Err(ResourceError::InvalidState),
        "a failed move of the new Submitted into Current must be returned \
         [census:C-retire-submitted-to-current]"
    );
    assert!(consumer.capacity.is_admission_closed());
}

/// census: C-on-available-releasing-early-return commit.rs on_available `let Some(err) = transition_error` #1
#[test]
fn c0_2ci_guard_on_available_leaves_rejected_untouched_after_releasing_error() {
    let (mut service, releasing_alloc, _drops) = spy_service();
    let rejected_alloc = spy(&mut service);
    let rejected_key = rejected_alloc.key();
    let mut consumer = CommitResourceConsumer::new();
    consumer.releasing_resources = vec![
        CommitResources::new(vec![releasing_alloc], None, None, None, vec![], vec![])
            .with_direct_role(RoleReservation::new_for_test(
                DirectRole::Preparing,
                999,
                closed_cell(),
            )),
    ];
    // Releasable and roleless: processing it would drop it.
    consumer.rejected_resources = vec![CommitResources::new(
        vec![rejected_alloc],
        None,
        None,
        None,
        vec![],
        vec![],
    )];
    // Both the first and the second transition_error check return the same
    // error, so the return value cannot tell them apart. What the first one
    // protects is the rejected half: it must not be processed after an error.
    assert_eq!(
        consumer.on_available(&[], &mut service),
        Err(ResourceError::InvalidState)
    );
    // "Untouched", not just "still one": the same resource, still holding the
    // same allocation, and nothing released on its behalf.
    let rejected: Vec<Vec<AllocationKey>> = consumer
        .rejected_resources
        .iter()
        .map(|r| r.allocations.iter().map(|a| a.key()).collect())
        .collect();
    assert_eq!(
        (rejected, consumer.released_presents.len()),
        (vec![vec![rejected_key]], 0),
        "rejected resources must be left untouched after a releasing-half error \
         [census:C-on-available-releasing-early-return]"
    );
}

/// census: C-on-available-rejected-error commit.rs on_available `let Some(err) = transition_error` #2
#[test]
fn c0_2ci_guard_on_available_returns_rejected_half_error() {
    let (mut service, rejected_alloc, _drops) = spy_service();
    let mut consumer = CommitResourceConsumer::new();
    consumer.rejected_resources = vec![
        CommitResources::new(vec![rejected_alloc], None, None, None, vec![], vec![])
            .with_direct_role(RoleReservation::new_for_test(
                DirectRole::Preparing,
                999,
                closed_cell(),
            )),
    ];
    assert_eq!(
        consumer.on_available(&[], &mut service),
        Err(ResourceError::InvalidState),
        "a failed finish_role on a rejected resource must be returned \
         [census:C-on-available-rejected-error]"
    );
}

fn storage_lease(allocation: AllocationLease) -> super::storage::StorageLease {
    let key = allocation.key();
    super::storage::StorageLease {
        allocation,
        pixels: super::storage::PixelIdentity {
            target: crate::kms::render::target::PaintTarget::new(
                crate::kms::render::store::DrawableId::for_tests(1),
                (0, 0),
                None,
                24,
            ),
            allocation: key,
            content_offset: (0, 0),
            extent: ash::vk::Extent2D {
                width: 1,
                height: 1,
            },
            format: ash::vk::Format::B8G8R8A8_UNORM,
            image_view: ash::vk::ImageView::null(),
            sample_view: ash::vk::ImageView::null(),
            image: ash::vk::Image::null(),
        },
    }
}

/// census: D-releasable-source commit.rs is_resource_releasable `let Some(source) = &res.source && !service.is_releasable(&source.allocation.key())`
#[test]
fn c0_2ci_guard_on_available_retains_resource_with_busy_source() {
    let (mut service, held, _drops) = spy_service();
    let _pending = service.register(held.key(), ObligationKind::Gpu).unwrap();
    let mut consumer = CommitResourceConsumer::new();
    consumer.releasing_resources = vec![CommitResources::new(
        vec![],
        Some(storage_lease(held)),
        None,
        None,
        vec![],
        vec![],
    )];
    consumer.on_available(&[], &mut service).unwrap();
    assert_eq!(
        consumer.releasing_resources.len(),
        1,
        "a resource whose source allocation is not releasable must stay releasing \
         [census:D-releasable-source]"
    );
}

/// census: D-releasable-fallback commit.rs is_resource_releasable `let Some(fallback) = &res.fallback && !service.is_releasable(&fallback.allocation.key())`
#[test]
fn c0_2ci_guard_on_available_retains_resource_with_busy_fallback() {
    let (mut service, held, _drops) = spy_service();
    let _pending = service.register(held.key(), ObligationKind::Gpu).unwrap();
    let mut consumer = CommitResourceConsumer::new();
    consumer.releasing_resources = vec![CommitResources::new(
        vec![],
        None,
        Some(storage_lease(held)),
        None,
        vec![],
        vec![],
    )];
    consumer.on_available(&[], &mut service).unwrap();
    assert_eq!(
        consumer.releasing_resources.len(),
        1,
        "a resource whose fallback allocation is not releasable must stay releasing \
         [census:D-releasable-fallback]"
    );
}

/// census: D-unique-new-members commit.rs register_commit_dependencies `!GroupMember::validate_unique(&new_members)`
#[test]
fn c0_2ci_guard_commit_dependencies_refuse_duplicate_new_members() {
    let (mut service, old_alloc, _drops) = spy_service();
    let new_alloc = spy(&mut service);
    let old = vec![CommitResources::new(
        vec![old_alloc],
        None,
        None,
        None,
        vec![member()],
        vec![],
    )];
    let new = vec![CommitResources::new(
        vec![new_alloc],
        None,
        None,
        None,
        vec![member(), member()],
        vec![],
    )];
    assert!(
        matches!(
            register_commit_dependencies(CommitId::for_tests(930), old, new, &mut service),
            Err((ResourceError::InvalidProof, _, _))
        ),
        "a commit whose new set repeats a member must be refused [census:D-unique-new-members]"
    );
}

/// census: B-authorize-write-closed transport.rs authorize_write `TransportState::Closed =>`
#[test]
fn c0_2ci_guard_authorize_write_refuses_every_class_when_closed() {
    let ownership = FakeDirectOwnershipState::new();
    let mut gate = TransportGate::new_legacy(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        IncarnationId::first(),
        Box::new(ownership.clone()),
    );
    gate.force_close();
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
        assert_eq!(
            gate.authorize_write(class, None),
            Err(ResourceError::Detached),
            "a closed transport must refuse every writer class [census:B-authorize-write-closed]"
        );
    }
}

// ---------------------------------------------------------------------------
// Session 2, spec 4.1: pool-husk accounting bound to identity.
// ---------------------------------------------------------------------------

/// A registry on `incarnation` whose every other `FileFamilyClosed`
/// precondition already holds, so a mint refusal can only come from husk
/// accounting.
fn husk_registry(incarnation: IncarnationId) -> DrmCleanupRegistry {
    let mut registry = DrmCleanupRegistry::new_with_io(
        DrmDeviceKey {
            major: 226,
            minor: 0,
        },
        incarnation,
        Box::new(super::tests::MockCleanupIo::new(Rc::new(
            std::cell::RefCell::new(Vec::new()),
        ))),
    );
    registry.detach_fake_submitters();
    registry.reap_fake_helper();
    registry.close_fake_control();
    registry
}

/// A stub device alias for a husk registration: `Device::for_tests` opens
/// no DRM node, and what the test needs is only the `Rc` whose lifetime the
/// registration now owns.
fn husk_alias() -> Rc<crate::drm::Device> {
    Rc::new(crate::drm::Device::for_tests().expect("stub drm device"))
}

fn mint_refusal(registry: &mut DrmCleanupRegistry) -> Option<String> {
    registry
        .try_mint_file_family_closed(|_, _| Ok(()))
        .err()
        .map(|err| err.to_string())
}

/// Round-1 B-1: the alias and the count that names it end together, so the
/// inventory cannot reach zero while the husk's `Rc<drm::Device>` is alive.
#[test]
fn c0_2ci_husk_registration_owns_the_alias_it_counts() {
    let mut registry = husk_registry(IncarnationId::first());
    let alias = husk_alias();
    let watch = Rc::clone(&alias);
    let registration = registry.register_pool_husk(alias);
    assert_eq!(
        Rc::strong_count(&watch),
        2,
        "the registration must own the husk's alias"
    );
    assert_eq!(
        mint_refusal(&mut registry).as_deref(),
        Some("non-payload aliases still active"),
        "the family cannot close while the husk alias is counted"
    );
    registry.unregister_pool_husk(registration).unwrap();
    assert_eq!(
        Rc::strong_count(&watch),
        1,
        "consuming the registration must drop the alias it counted"
    );
    assert_eq!(mint_refusal(&mut registry), None);
}

/// census: S2-husk-mint-poisoned drm_cleanup.rs try_mint_file_family_closed `self.husk_accounting_failed.get()`
#[test]
fn c0_2ci_guard_dropped_husk_registration_closes_the_family_barrier() {
    let mut registry = husk_registry(IncarnationId::first());
    drop(registry.register_pool_husk(husk_alias()));
    assert_eq!(
        mint_refusal(&mut registry).as_deref(),
        Some("pool husk accounting failed"),
        "a husk registration dropped undischarged must fail closed [census:S2-husk-mint-poisoned]"
    );
}

/// census: S2-husk-foreign drm_cleanup.rs unregister_pool_husk `registration.device_key != self.device_key || registration.incarnation != self.incarnation`
#[test]
fn c0_2ci_guard_foreign_husk_registration_is_refused_and_fails_closed() {
    let mut minted_by = husk_registry(IncarnationId::first());
    let mut presented_to = husk_registry(IncarnationId::first().next());
    let own = presented_to.register_pool_husk(husk_alias());
    let foreign = minted_by.register_pool_husk(husk_alias());
    assert_eq!(
        presented_to.unregister_pool_husk(foreign),
        Err(ResourceError::WrongIncarnation),
        "a registration from another incarnation must be refused as such [census:S2-husk-foreign]"
    );
    // The refused registration must not have consumed `own`'s count...
    presented_to.unregister_pool_husk(own).unwrap();
    // ...and both registries now refuse to certify the family closed.
    assert_eq!(
        mint_refusal(&mut presented_to).as_deref(),
        Some("pool husk accounting failed")
    );
    assert_eq!(
        mint_refusal(&mut minted_by).as_deref(),
        Some("pool husk accounting failed")
    );
}

/// census: S2-husk-unknown drm_cleanup.rs unregister_pool_husk `!Rc::ptr_eq(&registration.accounting, &self.husk_accounting_failed)`
#[test]
fn c0_2ci_guard_unknown_husk_registration_cannot_consume_another_husks_count() {
    // Same device and incarnation, different registry: identity alone
    // cannot tell them apart, so only the registry's own correlation can.
    let mut minted_by = husk_registry(IncarnationId::first());
    let mut presented_to = husk_registry(IncarnationId::first());
    let own = presented_to.register_pool_husk(husk_alias());
    let unknown = minted_by.register_pool_husk(husk_alias());
    assert_eq!(
        presented_to.unregister_pool_husk(unknown),
        Err(ResourceError::InvalidProof),
        "a registration another registry minted must be refused [census:S2-husk-unknown]"
    );
    presented_to.unregister_pool_husk(own).unwrap();
    assert_eq!(
        mint_refusal(&mut presented_to).as_deref(),
        Some("pool husk accounting failed")
    );
    assert_eq!(
        mint_refusal(&mut minted_by).as_deref(),
        Some("pool husk accounting failed")
    );
}

// ---------------------------------------------------------------------------
// Session 2, spec 4.2: a failed managed submission's unwind is propagated,
// and the transport closes when the submission is uncertain or unwinding
// fails.
// ---------------------------------------------------------------------------

use crate::kms::{render::scene::managed_submit_failure, vk::compositor::PresentError};

/// A service with one spy entry holding a pending GPU obligation, and a
/// legacy transport gate installed on it.
fn submission_fixture() -> (
    ResourceService,
    AllocationLease,
    ObligationId,
    TransportGate,
) {
    let (mut service, held, _drops) = spy_service();
    let obligation = service.register(held.key(), ObligationKind::Gpu).unwrap();
    let gate = TransportGate::for_tests(service.device(), IncarnationId::first());
    service.set_transport_gate(gate.handle()).unwrap();
    (service, held, obligation, gate)
}

/// census: S2-gate-install-identity mod.rs set_transport_gate `gate.device() != self.device || gate.incarnation() != self.incarnation`
#[test]
fn c0_2ci_guard_service_refuses_a_transport_gate_for_another_transport() {
    let (mut service, _held, _obligation, _gate) = submission_fixture();
    let other_device = TransportGate::for_tests(
        DrmDeviceKey {
            major: 226,
            minor: 1,
        },
        IncarnationId::first(),
    );
    let other_incarnation =
        TransportGate::for_tests(service.device(), IncarnationId::first().next());
    for foreign in [&other_device, &other_incarnation] {
        assert_eq!(
            service.set_transport_gate(foreign.handle()),
            Err(ResourceError::WrongIncarnation),
            "a service must refuse a gate for another transport [census:S2-gate-install-identity]"
        );
    }
    service.close_transport_gate();
    assert_eq!(other_device.state(), TransportState::Legacy);
    assert_eq!(other_incarnation.state(), TransportState::Legacy);
}

/// census: S2-gate-install-replacement mod.rs set_transport_gate `let Some(installed) = &self.transport_gate && !installed.same_gate(&gate)`
#[test]
fn c0_2ci_guard_service_refuses_a_second_transport_gate() {
    let (mut service, _held, _obligation, gate) = submission_fixture();
    // Same device and incarnation, a different gate instance.
    let replacement = TransportGate::for_tests(service.device(), IncarnationId::first());
    assert_eq!(
        service.set_transport_gate(replacement.handle()),
        Err(ResourceError::InvalidState),
        "a service must refuse to swap the transport it closes [census:S2-gate-install-replacement]"
    );
    // Re-installing the gate already there is idempotent.
    service.set_transport_gate(gate.handle()).unwrap();
    service.close_transport_gate();
    assert_eq!(gate.state(), TransportState::Closed);
    assert_eq!(replacement.state(), TransportState::Legacy);
}

/// Round-2 B-2: a failed unwind must not cost the cause its identity. The
/// caller latches the fatal renderer state on a device loss, and it
/// classifies the error it is handed.
#[test]
fn c0_2ci_failed_unwind_keeps_a_device_loss_recognisable() {
    let (mut service, held, obligation, _gate) = submission_fixture();
    service.cancel(held.key(), obligation).unwrap();
    let err = managed_submit_failure(
        &mut service,
        &[(held.key(), obligation)],
        false,
        PresentError::Vk(ash::vk::Result::ERROR_DEVICE_LOST),
    );
    assert!(
        crate::kms::render::scene::present_error_is_device_lost(&err),
        "a device loss must survive a failed unwind, got {err}"
    );
    assert!(
        err.to_string()
            .contains("unwinding the managed batch also failed"),
        "and the unwind failure must still be reported, got {err}"
    );
}

/// census: S2-cancel-pre-submit-error gpu.rs cancel_pre_submit_batch `Some(err) =>`
#[test]
fn c0_2ci_guard_failed_pre_submit_cancel_is_reported_and_closes_transport() {
    let (mut service, held, obligation, gate) = submission_fixture();
    // The obligation is already gone, so the unwind's cancel must fail.
    service.cancel(held.key(), obligation).unwrap();
    let err = managed_submit_failure(
        &mut service,
        &[(held.key(), obligation)],
        false,
        PresentError::NoFb,
    );
    assert!(
        err.to_string()
            .contains("unwinding the managed batch also failed"),
        "a failed pre-submit cancel must reach the caller, got {err} [census:S2-cancel-pre-submit-error]"
    );
    assert_eq!(gate.state(), TransportState::Closed);
}

/// census: S2-freeze-uncertain-error gpu.rs freeze_uncertain_batch `Some(err) =>`
#[test]
fn c0_2ci_guard_failed_uncertain_freeze_is_reported_and_closes_transport() {
    let (mut service, held, obligation, gate) = submission_fixture();
    // A key from another incarnation cannot be frozen here.
    let err = managed_submit_failure(
        &mut service,
        &[(wrong_incarnation(held.key()), obligation)],
        true,
        PresentError::NoFb,
    );
    assert!(
        err.to_string()
            .contains("unwinding the managed batch also failed"),
        "a failed freeze of an uncertain submission must reach the caller, got {err} [census:S2-freeze-uncertain-error]"
    );
    assert_eq!(gate.state(), TransportState::Closed);
}

/// Round-2 B-1: a close that arrives through the service's handle is as
/// terminal as `close()`. The handle can only set the shared flag, so every
/// transition has to read the effective state, not the raw field. This
/// covers `begin_quiescing`; the Owner-side transitions need the handover
/// evidence Task 4 reshapes, so they are proven in Task 5.
#[test]
fn c0_2ci_service_driven_close_stops_the_gate_quiescing() {
    let (service, _held, _obligation, mut gate) = submission_fixture();
    service.close_transport_gate();
    assert_eq!(gate.state(), TransportState::Closed);
    assert_eq!(
        gate.begin_quiescing(),
        Err(ResourceError::Detached),
        "a transport closed through its handle must refuse to quiesce"
    );
}

#[test]
fn c0_2ci_uncertain_submission_freezes_and_closes_transport() {
    let (mut service, held, obligation, gate) = submission_fixture();
    let err = managed_submit_failure(
        &mut service,
        &[(held.key(), obligation)],
        true,
        PresentError::NoFb,
    );
    assert!(matches!(err, PresentError::NoFb), "got {err}");
    assert_eq!(
        gate.state(),
        TransportState::Closed,
        "an uncertain submission must close the transport"
    );
    assert!(
        service.is_frozen(&held.key()),
        "an uncertain submission must freeze its entries"
    );
}

#[test]
fn c0_2ci_pre_submit_failure_cancels_and_leaves_transport_open() {
    let (mut service, held, obligation, gate) = submission_fixture();
    let err = managed_submit_failure(
        &mut service,
        &[(held.key(), obligation)],
        false,
        PresentError::NoFb,
    );
    assert!(matches!(err, PresentError::NoFb), "got {err}");
    assert_eq!(gate.state(), TransportState::Legacy);
    assert!(!service.has_pending_obligation(&held.key(), obligation));
}
