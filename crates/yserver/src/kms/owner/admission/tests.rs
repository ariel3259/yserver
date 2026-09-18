use std::collections::BTreeSet;

use super::{Admission, AdmissionError, CrtcId, DirectSuccessor};

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
