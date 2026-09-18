use std::collections::BTreeSet;

use super::{
    Admission, AdmissionDecision, AdmissionError, Admitted, CrtcId, DirectSuccessor, IntentKey,
    Readiness, ReadinessSnapshot, Tier, WaitReason,
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
