use std::collections::BTreeSet;

use super::{
    Admission, AdmissionDecision, AdmissionError, Admitted, CrtcId, DirectSuccessor, IntentKey,
    MaintenanceClass, MaintenanceKey, Readiness, ReadinessSnapshot, Reentry, ReentryKind, Tier,
    WaitReason,
};

fn successor(
    source_generation: u64,
    layout_generation: u64,
    topology_generation: u64,
    ids: &[CrtcId],
) -> DirectSuccessor {
    DirectSuccessor {
        source_generation,
        layout_generation,
        topology_generation,
        crtcs: super::crtcs(ids),
    }
}

fn cursor_key(crtc: CrtcId) -> MaintenanceKey {
    MaintenanceKey {
        crtc,
        class: MaintenanceClass::Cursor,
    }
}

fn gamma_key(crtc: CrtcId) -> MaintenanceKey {
    MaintenanceKey {
        crtc,
        class: MaintenanceClass::Gamma,
    }
}

fn ticket_from_completed_generation(
    admission: &mut Admission,
    key: MaintenanceKey,
    generation: u64,
) -> super::AdmissionTicket {
    admission.set_maintenance(key, generation, false).unwrap();
    let ticket = admission.maintenance(key).unwrap().ticket;
    admission.note_completed(key, generation);
    ticket
}

#[test]
fn c0_adm_maint_ticket_survives_replacement() {
    let mut admission = Admission::new();
    let key = cursor_key(1);

    admission.set_maintenance(key, 10, false).unwrap();
    let first = admission.maintenance(key).unwrap();

    admission.set_maintenance(key, 11, false).unwrap();
    let replacement = admission.maintenance(key).unwrap();

    assert_eq!(replacement.generation, 11);
    assert_eq!(replacement.ticket, first.ticket);
    assert!(!replacement.aged);
}

#[test]
fn c0_adm_maint_update_while_submitted_gets_a_new_ticket() {
    let mut admission = Admission::new();
    let key = cursor_key(1);

    admission.set_maintenance(key, 10, false).unwrap();
    let submitted_ticket = admission.maintenance(key).unwrap().ticket;

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Maintenance {
            key,
            generation: 10,
        },
        Readiness::Ready,
    );
    let decision = admission.decide(&snapshot).unwrap();
    assert_eq!(decision.tier, Tier::Maintenance);
    let token = admission.lock(decision, &snapshot).unwrap();
    admission.confirm(token).unwrap();

    admission.set_maintenance(key, 11, false).unwrap();
    let first_update = admission.maintenance(key).unwrap();
    admission.set_maintenance(key, 12, false).unwrap();
    let second_update = admission.maintenance(key).unwrap();

    assert!(submitted_ticket < first_update.ticket);
    assert_eq!(second_update.ticket, first_update.ticket);
    assert!(first_update.aged);
}

#[test]
fn c0_adm_maint_refuses_a_stale_generation() {
    let mut admission = Admission::new();
    let key = cursor_key(1);

    admission.set_maintenance(key, 10, false).unwrap();
    assert_eq!(
        admission.set_maintenance(key, 10, false),
        Err(AdmissionError::StaleGeneration {
            queued: 10,
            offered: 10,
        })
    );
    assert_eq!(admission.maintenance(key).unwrap().generation, 10);
}

#[test]
fn c0_adm_maint_unchanged_generation_is_never_carried() {
    let mut admission = Admission::new();
    let key = cursor_key(1);

    admission.note_completed(key, 5);
    admission.set_maintenance(key, 5, false).unwrap();

    assert!(admission.maintenance(key).is_none());
}

#[test]
fn c0_adm_maint_ages_on_arrival_behind_a_commit() {
    let mut admission = Admission::new();
    let key = cursor_key(1);

    admission.set_maintenance(key, 7, true).unwrap();

    assert!(admission.maintenance(key).unwrap().aged);
}

#[test]
fn c0_adm_maint_rejected_generation_reenters_aged_with_its_ticket() {
    let mut admission = Admission::new();
    let key = cursor_key(1);
    let ticket = ticket_from_completed_generation(&mut admission, key, 5);

    assert_eq!(
        admission.reenter(key, 5, ticket, ReentryKind::Rejected),
        Reentry::Reentered
    );
    assert_eq!(
        admission.maintenance(key),
        Some(super::MaintenanceIntent {
            generation: 5,
            ticket,
            aged: true,
        })
    );
    assert_eq!(admission.rejection_count(key), 1);
}

#[test]
fn c0_adm_maint_second_consecutive_rejection_drops() {
    let mut admission = Admission::new();
    let key = cursor_key(1);
    let ticket = ticket_from_completed_generation(&mut admission, key, 5);

    assert_eq!(
        admission.reenter(key, 5, ticket, ReentryKind::Rejected),
        Reentry::Reentered
    );
    assert_eq!(
        admission.reenter(key, 5, ticket, ReentryKind::Rejected),
        Reentry::Dropped
    );
    assert!(admission.maintenance(key).is_none());
    assert_eq!(admission.rejection_count(key), 2);
}

#[test]
fn c0_adm_maint_collision_keeps_older_ticket_and_inherits_the_count() {
    let mut admission = Admission::new();
    let key = cursor_key(1);
    let older_ticket = ticket_from_completed_generation(&mut admission, key, 5);

    admission.set_maintenance(key, 6, false).unwrap();
    let newer_ticket = admission.maintenance(key).unwrap().ticket;
    assert!(older_ticket < newer_ticket);

    assert_eq!(
        admission.reenter(key, 5, older_ticket, ReentryKind::Rejected),
        Reentry::Reentered
    );
    assert_eq!(
        admission.maintenance(key),
        Some(super::MaintenanceIntent {
            generation: 6,
            ticket: older_ticket,
            aged: true,
        })
    );
    assert_eq!(admission.rejection_count(key), 1);

    assert_eq!(
        admission.reenter(key, 4, older_ticket, ReentryKind::Rejected),
        Reentry::Dropped
    );
    assert!(admission.maintenance(key).is_none());
}

#[test]
fn c0_adm_maint_unknown_is_not_a_rejection() {
    let mut admission = Admission::new();
    let key = gamma_key(2);
    let ticket = ticket_from_completed_generation(&mut admission, key, 8);

    assert_eq!(
        admission.reenter(key, 8, ticket, ReentryKind::Unknown),
        Reentry::Reentered
    );
    assert_eq!(
        admission.reenter(key, 8, ticket, ReentryKind::Unknown),
        Reentry::Reentered
    );
    assert_eq!(admission.rejection_count(key), 0);
    assert_eq!(admission.maintenance(key).unwrap().ticket, ticket);
}

#[test]
fn c0_adm_maint_completed_resets_the_count() {
    let mut admission = Admission::new();
    let key = cursor_key(1);
    let ticket = ticket_from_completed_generation(&mut admission, key, 5);

    assert_eq!(
        admission.reenter(key, 5, ticket, ReentryKind::Rejected),
        Reentry::Reentered
    );
    admission.note_completed(key, 5);
    assert_eq!(admission.rejection_count(key), 0);

    assert_eq!(
        admission.reenter(key, 5, ticket, ReentryKind::Rejected),
        Reentry::Reentered
    );
}

#[test]
fn c0_adm_maint_cursor_recovery_is_stored_per_crtc() {
    let mut admission = Admission::new();

    admission.request_cursor_recovery(1);
    admission.request_cursor_recovery(1);
    admission.request_cursor_recovery(2);

    assert_eq!(admission.cursor_recovery(), &BTreeSet::from([1, 2]));
}

#[test]
fn c0_adm_maint_a_generation_after_a_drop_drops_on_its_first_rejection() {
    let mut admission = Admission::new();
    let key = cursor_key(1);
    let first_ticket = ticket_from_completed_generation(&mut admission, key, 5);

    assert_eq!(
        admission.reenter(key, 5, first_ticket, ReentryKind::Rejected),
        Reentry::Reentered
    );
    assert_eq!(
        admission.reenter(key, 5, first_ticket, ReentryKind::Rejected),
        Reentry::Dropped
    );

    admission.set_maintenance(key, 6, false).unwrap();
    let second_ticket = admission.maintenance(key).unwrap().ticket;
    assert!(first_ticket < second_ticket);
    assert_eq!(
        admission.reenter(key, 6, second_ticket, ReentryKind::Rejected),
        Reentry::Dropped
    );
    assert!(admission.maintenance(key).is_none());
}

#[test]
fn c0_adm_composed_newest_wins_and_keeps_its_ordinal() {
    let mut admission = Admission::new();

    admission.set_composed(1, 10).unwrap();
    let first = admission.composed(1).unwrap();

    admission.set_composed(1, 11).unwrap();
    let current = admission.composed(1).unwrap();

    assert_eq!(current.generation, 11);
    assert_eq!(current.ordinal, first.ordinal);
}

#[test]
fn c0_adm_composed_refuses_a_stale_generation() {
    let mut admission = Admission::new();

    admission.set_composed(1, 10).unwrap();
    let result = admission.set_composed(1, 10);

    assert_eq!(
        result,
        Err(AdmissionError::StaleGeneration {
            queued: 10,
            offered: 10,
        })
    );
    assert_eq!(admission.composed(1).unwrap().generation, 10);
}

#[test]
fn c0_adm_second_direct_successor_displaces_the_first_and_keeps_the_ordinal() {
    let mut admission = Admission::new();
    let first = successor(10, 20, 30, &[1, 2]);
    let second = successor(11, 21, 31, &[1, 2]);

    assert_eq!(admission.set_direct_successor(first.clone()), Ok(None));
    let first_ordinal = admission.direct().unwrap().ordinal;

    assert_eq!(
        admission.set_direct_successor(second.clone()),
        Ok(Some(first))
    );
    let queued = admission.direct().unwrap();
    assert_eq!(queued.successor, second);
    assert_eq!(queued.ordinal, first_ordinal);
}

#[test]
fn c0_adm_unflip_displaces_the_successor_and_refuses_later_direct_work() {
    let mut admission = Admission::new();
    let queued = successor(10, 20, 30, &[1]);
    let later = successor(11, 21, 31, &[1]);

    admission.set_direct_successor(queued.clone()).unwrap();
    assert_eq!(
        admission.request_unflip(super::crtcs(&[1, 2])),
        Ok(Some(queued))
    );
    assert!(admission.direct().is_none());
    assert_eq!(
        admission.set_direct_successor(later),
        Err(AdmissionError::UnflipPending)
    );
    assert_eq!(admission.unflip().unwrap().crtcs, super::crtcs(&[1, 2]));
}

#[test]
fn c0_adm_withdraw_only_matches_the_queued_generation() {
    let mut admission = Admission::new();
    let queued = successor(10, 20, 30, &[1]);

    admission.set_direct_successor(queued.clone()).unwrap();
    assert!(admission.withdraw_direct(11).is_none());
    assert_eq!(admission.direct().unwrap().successor, queued);
    assert_eq!(admission.withdraw_direct(10), Some(queued));
    assert!(admission.direct().is_none());
}

#[test]
fn c0_adm_empty_crtc_sets_are_refused() {
    let mut admission = Admission::new();
    let empty = DirectSuccessor {
        source_generation: 10,
        layout_generation: 20,
        topology_generation: 30,
        crtcs: BTreeSet::new(),
    };

    assert_eq!(
        admission.set_direct_successor(empty),
        Err(AdmissionError::EmptyCrtcSet)
    );
    assert_eq!(
        admission.request_unflip(BTreeSet::new()),
        Err(AdmissionError::EmptyCrtcSet)
    );
    assert!(admission.direct().is_none());
    assert!(admission.unflip().is_none());
}

#[test]
fn c0_adm_ordinals_are_device_monotonic_across_shapes() {
    let mut admission = Admission::new();

    admission.set_composed(2, 10).unwrap();
    let composed_two = admission.composed(2).unwrap().ordinal;

    admission
        .set_direct_successor(successor(20, 30, 40, &[1, 2]))
        .unwrap();
    let direct = admission.direct().unwrap().ordinal;

    admission.set_composed(1, 30).unwrap();
    let composed_one = admission.composed(1).unwrap().ordinal;

    assert!(composed_two < direct);
    assert!(direct < composed_one);
}

#[test]
fn c0_adm_topology_requests_are_monotonic() {
    let mut admission = Admission::new();

    admission.request_topology(10).unwrap();
    assert_eq!(
        admission.request_topology(10),
        Err(AdmissionError::StaleGeneration {
            queued: 10,
            offered: 10,
        })
    );
    admission.request_topology(11).unwrap();
    assert_eq!(admission.topology(), Some(11));
}

#[test]
fn c0_adm_a_second_unflip_request_widens_the_barrier() {
    let mut admission = Admission::new();

    admission.request_unflip(super::crtcs(&[1])).unwrap();
    admission.request_unflip(super::crtcs(&[2])).unwrap();

    assert_eq!(admission.unflip().unwrap().crtcs, super::crtcs(&[1, 2]));
}

#[test]
fn c0_adm_tiers_topology_then_unflip_then_primary() {
    let mut with_topology = Admission::new();
    with_topology.set_composed(1, 10).unwrap();
    with_topology.request_unflip(super::crtcs(&[2])).unwrap();
    with_topology.request_topology(30).unwrap();

    let mut snapshot = ReadinessSnapshot::new(20, 30);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    snapshot.report(IntentKey::Unflip, Readiness::Ready);

    assert_eq!(
        with_topology.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Topology,
            admitted: Admitted::Topology { generation: 30 },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );

    let mut without_topology = Admission::new();
    without_topology.set_composed(1, 10).unwrap();
    without_topology.request_unflip(super::crtcs(&[2])).unwrap();

    assert_eq!(
        without_topology.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Unflip,
            admitted: Admitted::Unflip {
                crtcs: super::crtcs(&[2]),
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_waiting_or_unreported_intent_is_never_admitted() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();
    admission
        .set_direct_successor(successor(20, 30, 40, &[1]))
        .unwrap();
    admission.request_unflip(super::crtcs(&[2])).unwrap();

    let mut snapshot = ReadinessSnapshot::new(30, 40);
    assert!(admission.decide(&snapshot).is_none());

    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Waiting(WaitReason::NoReusableBuffer),
    );
    snapshot.report(
        IntentKey::Direct {
            source_generation: 20,
        },
        Readiness::Waiting(WaitReason::SourceWaits),
    );
    snapshot.report(
        IntentKey::Unflip,
        Readiness::Waiting(WaitReason::ExitRetirementOccupied),
    );
    assert!(admission.decide(&snapshot).is_none());

    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 11,
        },
        Readiness::Ready,
    );
    assert!(admission.decide(&snapshot).is_none());
}

#[test]
fn c0_adm_oldest_ready_primary_wins_across_shapes() {
    let mut admission = Admission::new();
    admission
        .set_direct_successor(successor(10, 20, 30, &[1]))
        .unwrap();
    admission.set_composed(2, 40).unwrap();

    let mut snapshot = ReadinessSnapshot::new(20, 30);
    snapshot.report(
        IntentKey::Direct {
            source_generation: 10,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 40,
        },
        Readiness::Ready,
    );

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Direct {
                successor: successor(10, 20, 30, &[1]),
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_ordinal_survives_replacement_and_waiting() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();
    admission.set_composed(2, 20).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Waiting(WaitReason::NoReusableBuffer),
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 2,
                generation: 20,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );

    admission.set_composed(1, 11).unwrap();
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 11,
        },
        Readiness::Ready,
    );
    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 1,
                generation: 11,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_direct_with_a_stale_layout_or_topology_generation_is_not_admitted() {
    let mut admission = Admission::new();
    admission
        .set_direct_successor(successor(10, 20, 30, &[1]))
        .unwrap();

    let mut snapshot = ReadinessSnapshot::new(21, 30);
    snapshot.report(
        IntentKey::Direct {
            source_generation: 10,
        },
        Readiness::Ready,
    );
    assert!(admission.decide(&snapshot).is_none());

    snapshot.layout_generation = 20;
    snapshot.topology_generation = 31;
    assert!(admission.decide(&snapshot).is_none());

    snapshot.topology_generation = 30;
    assert!(admission.decide(&snapshot).is_some());
}

#[test]
fn c0_adm_composed_on_an_unflip_crtc_does_not_overtake_the_barrier() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();
    admission.set_composed(2, 20).unwrap();
    admission.request_unflip(super::crtcs(&[1])).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Unflip,
        Readiness::Waiting(WaitReason::SourceWaits),
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 2,
                generation: 20,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_decide_is_pure() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );

    let first = admission.decide(&snapshot);
    let second = admission.decide(&snapshot);

    assert_eq!(first, second);
    assert_eq!(admission.composed(1).unwrap().generation, 10);
}

#[test]
fn c0_adm_lock_refuses_a_second_lock_while_a_token_exists() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let decision = admission.decide(&snapshot).unwrap();
    let token = admission.lock(decision.clone(), &snapshot).unwrap();

    assert!(matches!(
        admission.lock(decision, &snapshot),
        Err(AdmissionError::AlreadyLocked)
    ));
    assert!(admission.is_locked());
    admission.abort(token).unwrap();
}

#[test]
fn c0_adm_abort_leaves_the_decider_exactly_as_before() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let before = admission.decide(&snapshot).unwrap();
    let ordinal = admission.composed(1).unwrap().ordinal;
    let token = admission.lock(before.clone(), &snapshot).unwrap();

    assert_eq!(admission.sequence(), 0);
    admission.abort(token).unwrap();

    assert!(!admission.is_locked());
    assert_eq!(admission.sequence(), 0);
    assert_eq!(admission.decide(&snapshot), Some(before));
    assert_eq!(admission.composed(1).unwrap().ordinal, ordinal);
}

#[test]
fn c0_adm_confirm_consumes_the_admitted_intent_only() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();
    admission.set_composed(2, 20).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    let decision = admission.decide(&snapshot).unwrap();
    let token = admission.lock(decision.clone(), &snapshot).unwrap();

    let confirmed = admission.confirm(token).unwrap();

    assert_eq!(confirmed.sequence, 1);
    assert_eq!(confirmed.decision, decision);
    assert!(admission.composed(1).is_none());
    assert_eq!(admission.composed(2).unwrap().generation, 20);
    assert!(!admission.is_locked());
}

#[test]
fn c0_adm_lock_detects_a_generation_that_changed_since_decide() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();

    let mut old_snapshot = ReadinessSnapshot::new(0, 0);
    old_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let old_decision = admission.decide(&old_snapshot).unwrap();

    admission.set_composed(1, 11).unwrap();
    let mut new_snapshot = ReadinessSnapshot::new(0, 0);
    new_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 11,
        },
        Readiness::Ready,
    );

    assert!(matches!(
        admission.lock(old_decision, &new_snapshot),
        Err(AdmissionError::DecisionMismatch)
    ));
    assert!(!admission.is_locked());
    assert_eq!(admission.sequence(), 0);
}

#[test]
fn c0_adm_lock_rejects_a_direct_successor_whose_layout_changed() {
    let mut admission = Admission::new();
    admission
        .set_direct_successor(successor(10, 20, 30, &[1]))
        .unwrap();

    let mut snapshot = ReadinessSnapshot::new(20, 30);
    snapshot.report(
        IntentKey::Direct {
            source_generation: 10,
        },
        Readiness::Ready,
    );
    let decision = admission.decide(&snapshot).unwrap();

    snapshot.layout_generation = 21;
    assert!(matches!(
        admission.lock(decision, &snapshot),
        Err(AdmissionError::DecisionMismatch)
    ));
    assert!(!admission.is_locked());
    assert_eq!(admission.direct().unwrap().successor.layout_generation, 20);
}

#[test]
fn c0_adm_a_dropped_token_keeps_the_decider_locked() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let decision = admission.decide(&snapshot).unwrap();
    let _token = admission.lock(decision, &snapshot).unwrap();

    assert!(admission.is_locked());
    assert_eq!(admission.sequence(), 0);
}

#[test]
fn c0_adm_a_foreign_token_is_refused() {
    let mut first = Admission::new();
    let mut second = Admission::new();
    first.set_composed(1, 10).unwrap();
    second.set_composed(1, 20).unwrap();

    let mut first_snapshot = ReadinessSnapshot::new(0, 0);
    first_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let mut second_snapshot = ReadinessSnapshot::new(0, 0);
    second_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 20,
        },
        Readiness::Ready,
    );
    let first_decision = first.decide(&first_snapshot).unwrap();
    let second_decision = second.decide(&second_snapshot).unwrap();
    let first_token = first.lock(first_decision, &first_snapshot).unwrap();
    let second_token = second.lock(second_decision, &second_snapshot).unwrap();

    assert_eq!(
        first.confirm(second_token),
        Err(AdmissionError::TokenMismatch)
    );
    assert!(first.is_locked());
    assert_eq!(first.sequence(), 0);

    let confirmed = first.confirm(first_token).unwrap();
    assert_eq!(confirmed.sequence, 1);
    assert!(!first.is_locked());
}

#[test]
fn c0_adm_a_foreign_token_cannot_abort() {
    let mut first = Admission::new();
    let mut second = Admission::new();
    first.set_composed(1, 10).unwrap();
    second.set_composed(1, 20).unwrap();

    let mut first_snapshot = ReadinessSnapshot::new(0, 0);
    first_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let mut second_snapshot = ReadinessSnapshot::new(0, 0);
    second_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 20,
        },
        Readiness::Ready,
    );
    let first_decision = first.decide(&first_snapshot).unwrap();
    let second_decision = second.decide(&second_snapshot).unwrap();
    let first_token = first.lock(first_decision, &first_snapshot).unwrap();
    let second_token = second.lock(second_decision, &second_snapshot).unwrap();

    assert_eq!(
        first.abort(second_token),
        Err(AdmissionError::TokenMismatch)
    );
    assert!(first.is_locked());
    assert_eq!(first.sequence(), 0);

    let confirmed = first.confirm(first_token).unwrap();
    assert_eq!(confirmed.sequence, 1);
    assert!(!first.is_locked());
}

#[test]
fn c0_adm_confirm_consumes_a_direct_unflip_or_topology_admission_exactly() {
    {
        let mut admission = Admission::new();
        admission
            .set_direct_successor(successor(10, 20, 30, &[1]))
            .unwrap();
        admission.set_composed(2, 40).unwrap();

        let mut snapshot = ReadinessSnapshot::new(20, 30);
        snapshot.report(
            IntentKey::Direct {
                source_generation: 10,
            },
            Readiness::Ready,
        );
        snapshot.report(
            IntentKey::Composed {
                crtc: 2,
                generation: 40,
            },
            Readiness::Ready,
        );
        let decision = admission.decide(&snapshot).unwrap();
        assert!(matches!(decision.admitted, Admitted::Direct { .. }));
        let token = admission.lock(decision, &snapshot).unwrap();
        admission.confirm(token).unwrap();

        assert!(admission.direct().is_none());
        assert_eq!(admission.composed(2).unwrap().generation, 40);
    }

    {
        let mut admission = Admission::new();
        admission.set_composed(2, 40).unwrap();
        admission.request_unflip(super::crtcs(&[1])).unwrap();

        let mut snapshot = ReadinessSnapshot::new(0, 0);
        snapshot.report(IntentKey::Unflip, Readiness::Ready);
        snapshot.report(
            IntentKey::Composed {
                crtc: 2,
                generation: 40,
            },
            Readiness::Ready,
        );
        let decision = admission.decide(&snapshot).unwrap();
        assert!(matches!(decision.admitted, Admitted::Unflip { .. }));
        let token = admission.lock(decision, &snapshot).unwrap();
        admission.confirm(token).unwrap();

        assert!(admission.unflip().is_none());
        assert_eq!(admission.composed(2).unwrap().generation, 40);
    }

    {
        let mut admission = Admission::new();
        admission.set_composed(2, 40).unwrap();
        admission.request_topology(50).unwrap();

        let mut snapshot = ReadinessSnapshot::new(0, 0);
        snapshot.report(
            IntentKey::Composed {
                crtc: 2,
                generation: 40,
            },
            Readiness::Ready,
        );
        let decision = admission.decide(&snapshot).unwrap();
        assert_eq!(decision.admitted, Admitted::Topology { generation: 50 });
        let token = admission.lock(decision, &snapshot).unwrap();
        admission.confirm(token).unwrap();

        assert!(admission.topology().is_none());
        assert_eq!(admission.composed(2).unwrap().generation, 40);
    }
}

#[test]
fn c0_adm_grouped_then_composed_on_its_crtcs_yields_to_the_owed_crtc() {
    let mut admission = Admission::new();
    admission
        .set_direct_successor(successor(10, 20, 30, &[1, 2]))
        .unwrap();

    let mut direct_snapshot = ReadinessSnapshot::new(20, 30);
    direct_snapshot.report(
        IntentKey::Direct {
            source_generation: 10,
        },
        Readiness::Ready,
    );
    let direct_decision = admission.decide(&direct_snapshot).unwrap();
    let direct_token = admission.lock(direct_decision, &direct_snapshot).unwrap();
    admission.confirm(direct_token).unwrap();

    admission.set_composed(1, 11).unwrap();
    admission.set_composed(2, 12).unwrap();
    admission.set_composed(3, 13).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    for (crtc, generation) in [(1, 11), (2, 12), (3, 13)] {
        snapshot.report(IntentKey::Composed { crtc, generation }, Readiness::Ready);
    }

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 3,
                generation: 13,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_composed_then_grouped_yields_to_the_owed_crtc() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();

    let mut first_snapshot = ReadinessSnapshot::new(0, 0);
    first_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let first_decision = admission.decide(&first_snapshot).unwrap();
    let first_token = admission.lock(first_decision, &first_snapshot).unwrap();
    admission.confirm(first_token).unwrap();

    admission
        .set_direct_successor(successor(20, 30, 40, &[1, 2]))
        .unwrap();
    admission.set_composed(3, 30).unwrap();

    let mut snapshot = ReadinessSnapshot::new(30, 40);
    snapshot.report(
        IntentKey::Direct {
            source_generation: 20,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 3,
            generation: 30,
        },
        Readiness::Ready,
    );

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 3,
                generation: 30,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );

    let decision = admission.decide(&snapshot).unwrap();
    let token = admission.lock(decision, &snapshot).unwrap();
    admission.confirm(token).unwrap();

    assert!(matches!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            admitted: Admitted::Direct { .. },
            ..
        })
    ));
}

#[test]
fn c0_adm_when_every_ready_crtc_was_just_served_the_oldest_wins() {
    let mut admission = Admission::new();
    admission
        .set_direct_successor(successor(10, 20, 30, &[1, 2]))
        .unwrap();

    let mut direct_snapshot = ReadinessSnapshot::new(20, 30);
    direct_snapshot.report(
        IntentKey::Direct {
            source_generation: 10,
        },
        Readiness::Ready,
    );
    let direct_decision = admission.decide(&direct_snapshot).unwrap();
    let direct_token = admission.lock(direct_decision, &direct_snapshot).unwrap();
    admission.confirm(direct_token).unwrap();

    admission.set_composed(2, 20).unwrap();
    admission.set_composed(1, 10).unwrap();
    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 2,
                generation: 20,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_an_intervening_admission_ends_the_successive_run() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();

    let mut composed_snapshot = ReadinessSnapshot::new(0, 0);
    composed_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let composed_decision = admission.decide(&composed_snapshot).unwrap();
    let composed_token = admission
        .lock(composed_decision, &composed_snapshot)
        .unwrap();
    admission.confirm(composed_token).unwrap();

    admission.request_topology(20).unwrap();
    let topology_decision = admission.decide(&ReadinessSnapshot::new(0, 0)).unwrap();
    let topology_token = admission
        .lock(topology_decision, &ReadinessSnapshot::new(0, 0))
        .unwrap();
    admission.confirm(topology_token).unwrap();

    admission.set_composed(1, 11).unwrap();
    admission.set_composed(2, 20).unwrap();
    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 11,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 1,
                generation: 11,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_retirement_successor_is_preferred_when_no_other_crtc_is_owed() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();
    admission
        .set_direct_successor(successor(20, 30, 40, &[1, 2]))
        .unwrap();

    let mut snapshot = ReadinessSnapshot::new(30, 40);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Direct {
            source_generation: 20,
        },
        Readiness::Ready,
    );

    assert!(matches!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            admitted: Admitted::Composed { crtc: 1, .. },
            ..
        })
    ));

    snapshot.retirement_wake = true;
    assert!(matches!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            admitted: Admitted::Direct { .. },
            ..
        })
    ));
}

#[test]
fn c0_adm_retirement_successor_yields_to_an_owed_crtc() {
    let mut admission = Admission::new();
    admission
        .set_direct_successor(successor(10, 20, 30, &[1, 2]))
        .unwrap();

    let mut first_snapshot = ReadinessSnapshot::new(20, 30);
    first_snapshot.report(
        IntentKey::Direct {
            source_generation: 10,
        },
        Readiness::Ready,
    );
    let first_decision = admission.decide(&first_snapshot).unwrap();
    let first_token = admission.lock(first_decision, &first_snapshot).unwrap();
    admission.confirm(first_token).unwrap();

    admission
        .set_direct_successor(successor(11, 20, 30, &[1, 2]))
        .unwrap();
    admission.set_composed(3, 30).unwrap();

    let mut snapshot = ReadinessSnapshot::new(20, 30);
    snapshot.retirement_wake = true;
    snapshot.report(
        IntentKey::Direct {
            source_generation: 11,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 3,
            generation: 30,
        },
        Readiness::Ready,
    );

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 3,
                generation: 30,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_a_retirement_successor_stream_cannot_starve_another_crtc() {
    let mut admission = Admission::new();
    admission
        .set_direct_successor(successor(10, 20, 30, &[1]))
        .unwrap();
    admission.set_composed(2, 20).unwrap();

    let mut first_snapshot = ReadinessSnapshot::new(20, 30);
    first_snapshot.report(
        IntentKey::Direct {
            source_generation: 10,
        },
        Readiness::Ready,
    );
    first_snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    let first_decision = admission.decide(&first_snapshot).unwrap();
    let first_token = admission.lock(first_decision, &first_snapshot).unwrap();
    admission.confirm(first_token).unwrap();

    admission
        .set_direct_successor(successor(11, 20, 30, &[1]))
        .unwrap();
    let mut retirement_snapshot = ReadinessSnapshot::new(20, 30);
    retirement_snapshot.retirement_wake = true;
    retirement_snapshot.report(
        IntentKey::Direct {
            source_generation: 11,
        },
        Readiness::Ready,
    );
    retirement_snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );

    assert_eq!(
        admission.decide(&retirement_snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 2,
                generation: 20,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_a_grouped_candidate_yields_to_an_owed_crtc_it_contains() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();

    let mut first_snapshot = ReadinessSnapshot::new(0, 0);
    first_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let first_decision = admission.decide(&first_snapshot).unwrap();
    let first_token = admission.lock(first_decision, &first_snapshot).unwrap();
    admission.confirm(first_token).unwrap();

    admission
        .set_direct_successor(successor(20, 30, 40, &[1, 2]))
        .unwrap();
    admission.set_composed(2, 20).unwrap();

    let mut snapshot = ReadinessSnapshot::new(30, 40);
    snapshot.report(
        IntentKey::Direct {
            source_generation: 20,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 2,
                generation: 20,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_a_lone_grouped_candidate_is_not_held_back_by_itself() {
    let mut admission = Admission::new();
    admission.set_composed(1, 10).unwrap();

    let mut first_snapshot = ReadinessSnapshot::new(0, 0);
    first_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    let first_decision = admission.decide(&first_snapshot).unwrap();
    let first_token = admission.lock(first_decision, &first_snapshot).unwrap();
    admission.confirm(first_token).unwrap();

    admission
        .set_direct_successor(successor(20, 30, 40, &[1, 2]))
        .unwrap();
    let mut snapshot = ReadinessSnapshot::new(30, 40);
    snapshot.report(
        IntentKey::Direct {
            source_generation: 20,
        },
        Readiness::Ready,
    );

    assert!(matches!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            admitted: Admitted::Direct { .. },
            ..
        })
    ));
}

#[test]
fn c0_adm_a_multi_crtc_unflip_serves_every_crtc_it_covers() {
    let mut admission = Admission::new();
    admission.request_unflip(super::crtcs(&[1, 2])).unwrap();

    let mut unflip_snapshot = ReadinessSnapshot::new(0, 0);
    unflip_snapshot.report(IntentKey::Unflip, Readiness::Ready);
    let unflip_decision = admission.decide(&unflip_snapshot).unwrap();
    let unflip_token = admission.lock(unflip_decision, &unflip_snapshot).unwrap();
    admission.confirm(unflip_token).unwrap();

    admission.set_composed(1, 10).unwrap();
    admission.set_composed(2, 20).unwrap();
    admission.set_composed(3, 30).unwrap();
    let mut snapshot = ReadinessSnapshot::new(0, 0);
    for (crtc, generation) in [(1, 10), (2, 20), (3, 30)] {
        snapshot.report(IntentKey::Composed { crtc, generation }, Readiness::Ready);
    }

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 3,
                generation: 30,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_retirement_preference_needs_no_other_crtc_owed() {
    let mut admission = Admission::new();
    admission.request_topology(10).unwrap();
    let topology_snapshot = ReadinessSnapshot::new(0, 0);
    let topology_decision = admission.decide(&topology_snapshot).unwrap();
    let topology_token = admission
        .lock(topology_decision, &topology_snapshot)
        .unwrap();
    admission.confirm(topology_token).unwrap();

    admission.set_composed(3, 30).unwrap();
    admission
        .set_direct_successor(successor(20, 40, 50, &[1]))
        .unwrap();
    let mut snapshot = ReadinessSnapshot::new(40, 50);
    snapshot.retirement_wake = true;
    snapshot.report(
        IntentKey::Composed {
            crtc: 3,
            generation: 30,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Direct {
            source_generation: 20,
        },
        Readiness::Ready,
    );

    assert_eq!(
        admission.decide(&snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 3,
                generation: 30,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_maint_ages_after_losing_an_admission() {
    let mut admission = Admission::new();
    let key = cursor_key(1);
    admission.set_maintenance(key, 7, false).unwrap();
    admission.set_composed(2, 10).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Maintenance { key, generation: 7 },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 10,
        },
        Readiness::Ready,
    );

    let decision = admission.decide(&snapshot).unwrap();
    assert_eq!(decision.tier, Tier::Primary);
    assert!(decision.carried.is_empty());
    let token = admission.lock(decision, &snapshot).unwrap();
    admission.confirm(token).unwrap();

    let intent = admission.maintenance(key).unwrap();
    assert_eq!(intent.generation, 7);
    assert!(intent.aged);
}

#[test]
fn c0_adm_maint_barrier_ages_overtaken_maintenance_without_resetting_tickets() {
    let mut admission = Admission::new();
    let key = cursor_key(1);
    admission.set_maintenance(key, 7, false).unwrap();
    let ticket = admission.maintenance(key).unwrap().ticket;
    admission.request_topology(10).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Maintenance { key, generation: 7 },
        Readiness::Ready,
    );
    let decision = admission.decide(&snapshot).unwrap();
    assert_eq!(decision.tier, Tier::Topology);
    let token = admission.lock(decision, &snapshot).unwrap();
    admission.confirm(token).unwrap();

    let intent = admission.maintenance(key).unwrap();
    assert!(intent.aged);
    assert_eq!(intent.ticket, ticket);
}

#[test]
fn c0_adm_maint_cursor_recovery_is_a_tier2_barrier() {
    let mut admission = Admission::new();
    admission.request_cursor_recovery(1);
    admission.set_composed(2, 20).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(IntentKey::CursorRecovery { crtc: 1 }, Readiness::Ready);
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    let decision = admission.decide(&snapshot).unwrap();
    assert_eq!(decision.tier, Tier::Unflip);
    assert_eq!(decision.admitted, Admitted::CursorRecovery { crtc: 1 });

    let mut blocked = Admission::new();
    blocked.request_cursor_recovery(1);
    blocked.set_composed(1, 10).unwrap();
    blocked.set_composed(2, 20).unwrap();
    let mut blocked_snapshot = ReadinessSnapshot::new(0, 0);
    blocked_snapshot.report(
        IntentKey::CursorRecovery { crtc: 1 },
        Readiness::Waiting(WaitReason::SourceWaits),
    );
    blocked_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    blocked_snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    assert_eq!(
        blocked.decide(&blocked_snapshot),
        Some(AdmissionDecision {
            tier: Tier::Primary,
            admitted: Admitted::Composed {
                crtc: 2,
                generation: 20,
            },
            carried: Vec::new(),
            combined_primary: None,
            ages: BTreeSet::new(),
        })
    );
}

#[test]
fn c0_adm_maint_aged_wins_over_non_aged() {
    let mut admission = Admission::new();
    let aged = cursor_key(1);
    let fresh = gamma_key(2);
    admission.set_maintenance(aged, 7, true).unwrap();
    admission.set_maintenance(fresh, 8, false).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Maintenance {
            key: aged,
            generation: 7,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Maintenance {
            key: fresh,
            generation: 8,
        },
        Readiness::Ready,
    );
    let decision = admission.decide(&snapshot).unwrap();

    assert_eq!(decision.tier, Tier::AgedMaintenance);
    assert_eq!(
        decision.admitted,
        Admitted::Maintenance {
            key: aged,
            generation: 7,
        }
    );
}

#[test]
fn c0_adm_maint_tier6_precedes_tier7_for_an_older_primary() {
    let mut admission = Admission::new();
    let cursor = cursor_key(2);
    admission.set_composed(1, 10).unwrap();
    admission.set_maintenance(cursor, 20, false).unwrap();
    admission.set_composed(2, 30).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 30,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Maintenance {
            key: cursor,
            generation: 20,
        },
        Readiness::Ready,
    );
    snapshot.report_compatible(
        IntentKey::Maintenance {
            key: cursor,
            generation: 20,
        },
        IntentKey::Composed {
            crtc: 2,
            generation: 30,
        },
    );

    let decision = admission.decide(&snapshot).unwrap();
    assert_eq!(decision.tier, Tier::Primary);
    assert_eq!(
        decision.admitted,
        Admitted::Composed {
            crtc: 1,
            generation: 10,
        }
    );
    assert!(decision.combined_primary.is_none());
    assert!(decision.carried.is_empty());
}

#[test]
fn c0_adm_maint_symmetric_absorption_combines_the_oldest_compatible_primary() {
    let mut composed_older = Admission::new();
    let cursor = cursor_key(1);
    let gamma = gamma_key(1);
    composed_older.set_composed(1, 10).unwrap();
    composed_older
        .set_direct_successor(successor(30, 0, 0, &[1, 2]))
        .unwrap();
    composed_older.set_maintenance(cursor, 20, true).unwrap();
    composed_older.set_maintenance(gamma, 21, false).unwrap();
    let mut composed_snapshot = ReadinessSnapshot::new(0, 0);
    composed_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    composed_snapshot.report(
        IntentKey::Direct {
            source_generation: 30,
        },
        Readiness::Ready,
    );
    for (key, generation) in [(cursor, 20), (gamma, 21)] {
        composed_snapshot.report(IntentKey::Maintenance { key, generation }, Readiness::Ready);
        composed_snapshot.report_compatible(
            IntentKey::Maintenance { key, generation },
            IntentKey::Composed {
                crtc: 1,
                generation: 10,
            },
        );
        composed_snapshot.report_compatible(
            IntentKey::Maintenance { key, generation },
            IntentKey::Direct {
                source_generation: 30,
            },
        );
    }
    let composed_decision = composed_older.decide(&composed_snapshot).unwrap();
    assert_eq!(composed_decision.tier, Tier::AgedMaintenance);
    assert_eq!(
        composed_decision.combined_primary,
        Some(Admitted::Composed {
            crtc: 1,
            generation: 10,
        })
    );
    assert_eq!(
        composed_decision
            .carried
            .iter()
            .map(|carried| carried.key)
            .collect::<Vec<_>>(),
        vec![cursor, gamma]
    );

    let mut direct_older = Admission::new();
    direct_older
        .set_direct_successor(successor(10, 0, 0, &[1, 2]))
        .unwrap();
    direct_older.set_composed(1, 11).unwrap();
    direct_older.set_maintenance(cursor, 20, true).unwrap();
    let mut direct_snapshot = ReadinessSnapshot::new(0, 0);
    direct_snapshot.report(
        IntentKey::Direct {
            source_generation: 10,
        },
        Readiness::Ready,
    );
    direct_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 11,
        },
        Readiness::Ready,
    );
    direct_snapshot.report(
        IntentKey::Maintenance {
            key: cursor,
            generation: 20,
        },
        Readiness::Ready,
    );
    direct_snapshot.report_compatible(
        IntentKey::Maintenance {
            key: cursor,
            generation: 20,
        },
        IntentKey::Direct {
            source_generation: 10,
        },
    );
    direct_snapshot.report_compatible(
        IntentKey::Maintenance {
            key: cursor,
            generation: 20,
        },
        IntentKey::Composed {
            crtc: 1,
            generation: 11,
        },
    );
    let direct_decision = direct_older.decide(&direct_snapshot).unwrap();
    assert_eq!(direct_decision.tier, Tier::AgedMaintenance);
    assert_eq!(
        direct_decision.combined_primary,
        Some(Admitted::Direct {
            successor: successor(10, 0, 0, &[1, 2]),
        })
    );
    assert_eq!(direct_decision.carried.len(), 1);
    assert_eq!(direct_decision.carried[0].key, cursor);
}

#[test]
fn c0_adm_maint_symmetric_absorption_respects_barriers_and_round_robin() {
    let mut barrier = Admission::new();
    let key = cursor_key(1);
    barrier.set_maintenance(key, 7, true).unwrap();
    barrier.set_composed(1, 10).unwrap();
    barrier.request_unflip(super::crtcs(&[1])).unwrap();
    let mut barrier_snapshot = ReadinessSnapshot::new(0, 0);
    barrier_snapshot.report(
        IntentKey::Unflip,
        Readiness::Waiting(WaitReason::ExitRetirementOccupied),
    );
    barrier_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    barrier_snapshot.report(
        IntentKey::Maintenance { key, generation: 7 },
        Readiness::Ready,
    );
    barrier_snapshot.report_compatible(
        IntentKey::Maintenance { key, generation: 7 },
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
    );
    let barrier_decision = barrier.decide(&barrier_snapshot).unwrap();
    assert_eq!(barrier_decision.tier, Tier::AgedMaintenance);
    assert!(barrier_decision.combined_primary.is_none());

    let mut round_robin = Admission::new();
    round_robin.set_composed(1, 1).unwrap();
    let mut first_snapshot = ReadinessSnapshot::new(0, 0);
    first_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 1,
        },
        Readiness::Ready,
    );
    let first = round_robin.decide(&first_snapshot).unwrap();
    let first_token = round_robin.lock(first, &first_snapshot).unwrap();
    round_robin.confirm(first_token).unwrap();

    round_robin.set_composed(1, 2).unwrap();
    round_robin.set_composed(2, 3).unwrap();
    round_robin.set_maintenance(key, 8, true).unwrap();
    let mut turn_snapshot = ReadinessSnapshot::new(0, 0);
    for (crtc, generation) in [(1, 2), (2, 3)] {
        turn_snapshot.report(IntentKey::Composed { crtc, generation }, Readiness::Ready);
    }
    turn_snapshot.report(
        IntentKey::Maintenance { key, generation: 8 },
        Readiness::Ready,
    );
    turn_snapshot.report_compatible(
        IntentKey::Maintenance { key, generation: 8 },
        IntentKey::Composed {
            crtc: 1,
            generation: 2,
        },
    );
    let turn_decision = round_robin.decide(&turn_snapshot).unwrap();
    assert_eq!(turn_decision.tier, Tier::AgedMaintenance);
    assert!(turn_decision.combined_primary.is_none());
}

#[test]
fn c0_adm_maint_symmetric_absorption_skips_an_incompatible_primary() {
    let mut admission = Admission::new();
    let key = cursor_key(1);
    admission.set_composed(1, 10).unwrap();
    admission
        .set_direct_successor(successor(20, 0, 0, &[1, 2]))
        .unwrap();
    admission.set_maintenance(key, 7, true).unwrap();

    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Direct {
            source_generation: 20,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Maintenance { key, generation: 7 },
        Readiness::Ready,
    );
    snapshot.report_compatible(
        IntentKey::Maintenance { key, generation: 7 },
        IntentKey::Direct {
            source_generation: 20,
        },
    );
    let decision = admission.decide(&snapshot).unwrap();
    assert_eq!(
        decision.combined_primary,
        Some(Admitted::Direct {
            successor: successor(20, 0, 0, &[1, 2]),
        })
    );

    let mut no_compatible = Admission::new();
    no_compatible.set_composed(1, 10).unwrap();
    no_compatible.set_maintenance(key, 7, true).unwrap();
    let mut no_compatible_snapshot = ReadinessSnapshot::new(0, 0);
    no_compatible_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    no_compatible_snapshot.report(
        IntentKey::Maintenance { key, generation: 7 },
        Readiness::Ready,
    );
    assert!(
        no_compatible
            .decide(&no_compatible_snapshot)
            .unwrap()
            .combined_primary
            .is_none()
    );
}

#[test]
fn c0_adm_maint_confirm_consumes_the_combined_primary() {
    let key = cursor_key(1);
    let mut confirmed = Admission::new();
    confirmed.set_composed(1, 10).unwrap();
    confirmed.set_maintenance(key, 7, true).unwrap();
    let mut snapshot = ReadinessSnapshot::new(0, 0);
    snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    snapshot.report(
        IntentKey::Maintenance { key, generation: 7 },
        Readiness::Ready,
    );
    snapshot.report_compatible(
        IntentKey::Maintenance { key, generation: 7 },
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
    );
    let decision = confirmed.decide(&snapshot).unwrap();
    let token = confirmed.lock(decision, &snapshot).unwrap();
    confirmed.confirm(token).unwrap();
    assert!(confirmed.composed(1).is_none());

    confirmed.set_composed(1, 11).unwrap();
    confirmed.set_composed(2, 20).unwrap();
    let mut served_snapshot = ReadinessSnapshot::new(0, 0);
    served_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 11,
        },
        Readiness::Ready,
    );
    served_snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    assert_eq!(
        confirmed.decide(&served_snapshot).unwrap().admitted,
        Admitted::Composed {
            crtc: 2,
            generation: 20,
        }
    );

    let mut aborted = Admission::new();
    aborted.set_composed(1, 10).unwrap();
    aborted.set_maintenance(key, 7, true).unwrap();
    let mut abort_snapshot = ReadinessSnapshot::new(0, 0);
    abort_snapshot.report(
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
        Readiness::Ready,
    );
    abort_snapshot.report(
        IntentKey::Maintenance { key, generation: 7 },
        Readiness::Ready,
    );
    abort_snapshot.report_compatible(
        IntentKey::Maintenance { key, generation: 7 },
        IntentKey::Composed {
            crtc: 1,
            generation: 10,
        },
    );
    let abort_decision = aborted.decide(&abort_snapshot).unwrap();
    let token = aborted
        .lock(abort_decision.clone(), &abort_snapshot)
        .unwrap();
    aborted.abort(token).unwrap();
    assert_eq!(aborted.composed(1).unwrap().generation, 10);
    assert_eq!(aborted.decide(&abort_snapshot), Some(abort_decision));
}

#[test]
fn c0_adm_maint_waiting_candidates_never_win_tiers_2_4_7() {
    let mut recovery = Admission::new();
    recovery.request_cursor_recovery(1);
    recovery.set_composed(2, 20).unwrap();
    let mut recovery_snapshot = ReadinessSnapshot::new(0, 0);
    recovery_snapshot.report(
        IntentKey::CursorRecovery { crtc: 1 },
        Readiness::Waiting(WaitReason::SourceWaits),
    );
    recovery_snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    assert_eq!(
        recovery.decide(&recovery_snapshot).unwrap().tier,
        Tier::Primary
    );

    let mut aged = Admission::new();
    let aged_key = cursor_key(1);
    aged.set_maintenance(aged_key, 7, true).unwrap();
    aged.set_composed(2, 20).unwrap();
    let mut aged_snapshot = ReadinessSnapshot::new(0, 0);
    aged_snapshot.report(
        IntentKey::Maintenance {
            key: aged_key,
            generation: 7,
        },
        Readiness::Waiting(WaitReason::SourceWaits),
    );
    aged_snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    assert_eq!(aged.decide(&aged_snapshot).unwrap().tier, Tier::Primary);

    let mut fresh = Admission::new();
    let fresh_key = gamma_key(1);
    fresh.set_maintenance(fresh_key, 8, false).unwrap();
    fresh.set_composed(2, 20).unwrap();
    let mut fresh_snapshot = ReadinessSnapshot::new(0, 0);
    fresh_snapshot.report(
        IntentKey::Maintenance {
            key: fresh_key,
            generation: 8,
        },
        Readiness::Waiting(WaitReason::SourceWaits),
    );
    fresh_snapshot.report(
        IntentKey::Composed {
            crtc: 2,
            generation: 20,
        },
        Readiness::Ready,
    );
    assert_eq!(fresh.decide(&fresh_snapshot).unwrap().tier, Tier::Primary);
}
