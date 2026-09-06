use std::collections::{BTreeMap, BTreeSet};

use crate::kms::executor::protocol::AtomicPropertyList;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ObjectKind {
    Crtc,
    Connector,
    Plane,
}

/// One DRM object's serialized persistent properties, plus the one thing the
/// wire provably cannot show: its binding before this request.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SerializedObject {
    pub object: u32,
    pub kind: ObjectKind,
    /// `CRTC_ID` before this request. `None` for a CRTC (its own id is its
    /// binding); `Some(0)` means it was unbound.
    pub old_crtc_id: Option<u32>,
    /// `(property id, value)` in wire order — **the minimal list**.
    pub props: Vec<(u32, u64)>,
}

/// Powered state of one CRTC across the request. Retained metadata, not a property.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CrtcPower {
    pub crtc_id: u32,
    pub old_active: bool,
    pub new_active: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PropertyIds {
    pub crtc_id: u32,
    pub active: u32,
    pub out_fence_ptr: u32,
}

/// Whether this request's class may carry out-fences at all.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FencePolicy {
    /// Every live class: one `OUT_FENCE_PTR` per `ExpectedCompletionCrtcs`.
    Required,
    /// `ValidationOnly`: none, on any CRTC. `spec:320-323`.
    Forbidden,
}

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ClosureError {
    #[error("closure member CRTC {0} has no powered-state row")]
    UnknownPower(u32),
    #[error("CRTC {0} has more than one powered-state row")]
    DuplicatePower(u32),
    #[error("powered-state row for CRTC {0} is outside the closure")]
    PowerOutsideClosure(u32),
    #[error("object {0} appears more than once")]
    DuplicateObject(u32),
    #[error("object {object} carries property {prop} more than once")]
    DuplicateProperty { object: u32, prop: u32 },
    #[error("CRTC_ID value {0:#x} does not fit a u32")]
    CrtcIdOutOfRange(u64),
    #[error("object {0} supplied its own OUT_FENCE_PTR")]
    UnsolicitedOutFence(u32),
    #[error("inactive-to-inactive CRTC {0} cannot carry the global page-event flag")]
    OffToOffWithPageEvent(u32),
    #[error("present consumer CRTC {0} is outside the kernel event set")]
    PresentConsumerOutsideEventSet(u32),
    #[error("serialized closure {serialized:?} differs from recorded {recorded:?}")]
    SerializedClosureDiffers {
        recorded: Vec<u32>,
        serialized: Vec<u32>,
    },
    #[error("OUT_FENCE_PTR coverage {found:?} differs from expected {expected:?}")]
    OutFenceCoverageDiffers { expected: Vec<u32>, found: Vec<u32> },
    #[error("CRTC {0} carries more than one OUT_FENCE_PTR")]
    DuplicateOutFence(u32),
    #[error("OUT_FENCE_PTR on non-CRTC object {0}")]
    OutFenceOnNonCrtc(u32),
    #[error("CRTC {crtc} serializes ACTIVE={serialized} against recorded {recorded}")]
    ActiveContradictsPower {
        crtc: u32,
        serialized: bool,
        recorded: bool,
    },
    #[error("object {0} in the serialized list has no known kind")]
    UnknownObject(u32),
    #[error("the serialized property list is malformed: {0}")]
    MalformedPropertyList(&'static str),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AtomicCrtcClosure {
    closure: Vec<u32>,
    /// Members contributed only by an `old_crtc_id`. A serialized list cannot
    /// show these, so the re-scan subtracts them before demanding equality.
    old_binding_only: Vec<u32>,
    expected_completion: Vec<u32>,
    kernel_event: Vec<u32>,
    present_event: Vec<u32>,
    new_active: BTreeMap<u32, bool>,
}

impl AtomicCrtcClosure {
    pub fn closure(&self) -> &[u32] {
        &self.closure
    }

    pub fn old_binding_only(&self) -> &[u32] {
        &self.old_binding_only
    }

    pub fn expected_completion(&self) -> &[u32] {
        &self.expected_completion
    }

    pub fn kernel_event(&self) -> &[u32] {
        &self.kernel_event
    }

    pub fn present_event(&self) -> &[u32] {
        &self.present_event
    }

    pub fn compute(
        objects: &[SerializedObject],
        power_rows: &[CrtcPower],
        ids: &PropertyIds,
        page_flip_event: bool,
        present_consumers: &[u32],
    ) -> Result<Self, ClosureError> {
        let mut power: BTreeMap<u32, CrtcPower> = BTreeMap::new();
        for row in power_rows {
            if power.insert(row.crtc_id, *row).is_some() {
                return Err(ClosureError::DuplicatePower(row.crtc_id));
            }
        }

        let mut from_objects: BTreeSet<u32> = BTreeSet::new();
        let mut from_old_binding: BTreeSet<u32> = BTreeSet::new();
        let mut seen: BTreeSet<u32> = BTreeSet::new();

        for object in objects {
            if !seen.insert(object.object) {
                return Err(ClosureError::DuplicateObject(object.object));
            }
            // Each property may appear at most once on an object: two values
            // for one property mean two different requests, and picking one
            // silently decouples the closure from what is actually sent.
            let mut props_seen: BTreeSet<u32> = BTreeSet::new();
            for (prop, _) in &object.props {
                if *prop == ids.out_fence_ptr {
                    return Err(ClosureError::UnsolicitedOutFence(object.object));
                }
                if !props_seen.insert(*prop) {
                    return Err(ClosureError::DuplicateProperty {
                        object: object.object,
                        prop: *prop,
                    });
                }
            }
            match object.kind {
                ObjectKind::Crtc => {
                    from_objects.insert(object.object);
                    // No ACTIVE is required here. Power is retained metadata,
                    // so the persistent list stays minimal (spec:551-556).
                }
                ObjectKind::Connector | ObjectKind::Plane => {
                    if let Some(old) = object.old_crtc_id.filter(|b| *b != 0) {
                        from_old_binding.insert(old);
                    }
                    for (prop, value) in &object.props {
                        if *prop == ids.crtc_id && *value != 0 {
                            let id = u32::try_from(*value)
                                .map_err(|_| ClosureError::CrtcIdOutOfRange(*value))?;
                            from_objects.insert(id);
                        }
                    }
                }
            }
        }

        let mut closure: Vec<u32> = from_objects.union(&from_old_binding).copied().collect();
        closure.sort_unstable();
        let old_binding_only: Vec<u32> = from_old_binding
            .difference(&from_objects)
            .copied()
            .collect();

        for id in power.keys() {
            if !closure.contains(id) {
                return Err(ClosureError::PowerOutsideClosure(*id));
            }
        }

        let mut expected_completion = Vec::new();
        let mut new_active = BTreeMap::new();
        for id in &closure {
            let row = *power.get(id).ok_or(ClosureError::UnknownPower(*id))?;
            new_active.insert(*id, row.new_active);
            if row.old_active || row.new_active {
                expected_completion.push(*id);
            } else if page_flip_event {
                return Err(ClosureError::OffToOffWithPageEvent(*id));
            }
        }

        let kernel_event = if page_flip_event {
            expected_completion.clone()
        } else {
            Vec::new()
        };

        let mut present_event = Vec::new();
        for id in present_consumers {
            if !kernel_event.contains(id) {
                return Err(ClosureError::PresentConsumerOutsideEventSet(*id));
            }
            if !present_event.contains(id) {
                present_event.push(*id);
            }
        }
        present_event.sort_unstable();

        Ok(Self {
            closure,
            old_binding_only,
            expected_completion,
            kernel_event,
            present_event,
            new_active,
        })
    }

    pub fn verify_serialized(
        &self,
        props: &AtomicPropertyList,
        kinds: &BTreeMap<u32, ObjectKind>,
        ids: &PropertyIds,
        fences: FencePolicy,
    ) -> Result<(), ClosureError> {
        if props.count_props.len() != props.objects.len() {
            return Err(ClosureError::MalformedPropertyList("count_props length"));
        }
        if props.props.len() != props.values.len() {
            return Err(ClosureError::MalformedPropertyList("props/values length"));
        }
        let declared: u64 = props.count_props.iter().map(|c| u64::from(*c)).sum();
        if declared != props.props.len() as u64 {
            return Err(ClosureError::MalformedPropertyList(
                "declared count vs payload",
            ));
        }

        let mut serialized: BTreeSet<u32> = BTreeSet::new();
        // A multiset, so a duplicate is visible rather than collapsed.
        let mut fenced: BTreeMap<u32, usize> = BTreeMap::new();
        let mut cursor = 0usize;
        for (index, object) in props.objects.iter().enumerate() {
            let count = props.count_props[index] as usize;
            let kind = *kinds
                .get(object)
                .ok_or(ClosureError::UnknownObject(*object))?;
            if kind == ObjectKind::Crtc {
                serialized.insert(*object);
            }
            for offset in 0..count {
                let prop = props.props[cursor + offset];
                let value = props.values[cursor + offset];
                if prop == ids.crtc_id
                    && matches!(kind, ObjectKind::Connector | ObjectKind::Plane)
                    && value != 0
                {
                    serialized.insert(value as u32);
                }
                if prop == ids.out_fence_ptr {
                    if kind != ObjectKind::Crtc {
                        return Err(ClosureError::OutFenceOnNonCrtc(*object));
                    }
                    *fenced.entry(*object).or_insert(0) += 1;
                }
                if prop == ids.active && kind == ObjectKind::Crtc {
                    let serialized = value != 0;
                    if let Some(&recorded) =
                        self.new_active.get(object).filter(|&&r| r != serialized)
                    {
                        return Err(ClosureError::ActiveContradictsPower {
                            crtc: *object,
                            serialized,
                            recorded,
                        });
                    }
                }
            }
            cursor += count;
        }

        let expected_serialized: Vec<u32> = self
            .closure
            .iter()
            .filter(|c| !self.old_binding_only.contains(c))
            .copied()
            .collect();
        let serialized: Vec<u32> = serialized.into_iter().collect();
        if serialized != expected_serialized {
            return Err(ClosureError::SerializedClosureDiffers {
                recorded: expected_serialized,
                serialized,
            });
        }

        if let Some((crtc, _)) = fenced.iter().find(|(_, n)| **n > 1) {
            return Err(ClosureError::DuplicateOutFence(*crtc));
        }
        let found: Vec<u32> = fenced.keys().copied().collect();
        let expected: &[u32] = match fences {
            FencePolicy::Required => self.expected_completion(),
            FencePolicy::Forbidden => &[],
        };
        if found != expected {
            return Err(ClosureError::OutFenceCoverageDiffers {
                expected: expected.to_vec(),
                found,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: PropertyIds = PropertyIds {
        crtc_id: 20,
        active: 21,
        out_fence_ptr: 22,
    };

    /// A CRTC that serializes its own `ACTIVE` — an enable, disable or modeset.
    fn crtc(id: u32, new_active: bool) -> SerializedObject {
        SerializedObject {
            object: id,
            kind: ObjectKind::Crtc,
            old_crtc_id: None,
            props: vec![(IDS.active, u64::from(new_active))],
        }
    }
    fn plane(id: u32, old: u32, new: u32) -> SerializedObject {
        SerializedObject {
            object: id,
            kind: ObjectKind::Plane,
            old_crtc_id: Some(old),
            props: vec![(IDS.crtc_id, u64::from(new))],
        }
    }
    /// Retained metadata, never serialized.
    fn pw(id: u32, old: bool, new: bool) -> CrtcPower {
        CrtcPower {
            crtc_id: id,
            old_active: old,
            new_active: new,
        }
    }

    #[test]
    fn a_plane_only_request_needs_no_crtc_property_at_all() {
        // spec:551-556 — the persistent list is minimal. A plane move must not
        // have to restate an unchanged ACTIVE for its bound CRTCs just to let the
        // closure be computed. Revision 2 required exactly that; this is the
        // regression guard.
        let c = AtomicCrtcClosure::compute(
            &[plane(31, 1, 2)],
            &[pw(1, true, true), pw(2, true, true)],
            &IDS,
            false,
            &[],
        )
        .expect("closure");
        assert_eq!(c.closure(), &[1, 2]);
        assert_eq!(c.expected_completion(), &[1, 2]);
        assert_eq!(
            c.old_binding_only(),
            &[1],
            "CRTC 1 appears only as an old binding"
        );
    }

    #[test]
    fn a_power_row_for_a_crtc_outside_the_closure_is_refused() {
        let err = AtomicCrtcClosure::compute(
            &[crtc(1, true)],
            &[pw(1, true, true), pw(7, true, true)],
            &IDS,
            false,
            &[],
        )
        .expect_err("must refuse");
        assert_eq!(err, ClosureError::PowerOutsideClosure(7));
    }

    #[test]
    fn a_crtc_id_value_that_does_not_fit_u32_is_refused_not_truncated() {
        // Truncation would record CRTC 1 while the kernel receives
        // 0x1_0000_0001 — the closure would describe a different request than
        // the one being sent.
        let mut p = plane(31, 0, 1);
        p.props = vec![(IDS.crtc_id, 0x1_0000_0001)];
        let err = AtomicCrtcClosure::compute(&[p], &[pw(1, true, true)], &IDS, false, &[])
            .expect_err("must refuse");
        assert_eq!(err, ClosureError::CrtcIdOutOfRange(0x1_0000_0001));
    }

    #[test]
    fn duplicate_properties_and_duplicate_object_rows_are_refused() {
        let mut c1 = crtc(1, true);
        c1.props.push((IDS.active, 0));
        assert_eq!(
            AtomicCrtcClosure::compute(&[c1], &[pw(1, true, true)], &IDS, false, &[])
                .expect_err("must refuse"),
            ClosureError::DuplicateProperty {
                object: 1,
                prop: IDS.active
            }
        );
        assert_eq!(
            AtomicCrtcClosure::compute(
                &[crtc(1, true), crtc(1, false)],
                &[pw(1, true, true)],
                &IDS,
                false,
                &[],
            )
            .expect_err("must refuse"),
            ClosureError::DuplicateObject(1)
        );
    }

    #[test]
    fn a_plane_move_includes_both_powered_endpoints() {
        // spec:557-560 — detach retains the old CRTC, attach retains the new one.
        let c = AtomicCrtcClosure::compute(
            &[crtc(1, true), crtc(2, true), plane(31, 1, 2)],
            &[pw(1, true, true), pw(2, true, true)],
            &IDS,
            false,
            &[],
        )
        .expect("closure");
        assert_eq!(c.closure(), &[1, 2]);
        assert_eq!(c.expected_completion(), &[1, 2]);
        assert!(
            c.old_binding_only().is_empty(),
            "both CRTCs also appear as objects"
        );
    }

    #[test]
    fn a_detach_records_the_old_endpoint_as_serialization_invisible() {
        // The old binding is the one thing the wire cannot show. It must be a
        // closure member and must be listed so the re-scan can subtract it.
        let c = AtomicCrtcClosure::compute(
            &[crtc(2, true), plane(31, 1, 2)],
            &[pw(2, true, true)],
            &IDS,
            false,
            &[],
        )
        .expect_err("CRTC 1 has no power row");
        assert_eq!(c, ClosureError::UnknownPower(1));

        let c = AtomicCrtcClosure::compute(
            &[crtc(1, false), crtc(2, true), plane(31, 1, 2)],
            &[pw(1, true, false), pw(2, true, true)],
            &IDS,
            false,
            &[],
        )
        .expect("closure");
        assert_eq!(c.closure(), &[1, 2]);
    }

    #[test]
    fn an_unbound_endpoint_is_not_a_closure_member() {
        let c = AtomicCrtcClosure::compute(
            &[crtc(2, true), plane(31, 0, 2)],
            &[pw(2, false, true)],
            &IDS,
            false,
            &[],
        )
        .expect("closure");
        assert_eq!(c.closure(), &[2]);
    }

    #[test]
    fn a_disable_still_owes_completion_evidence() {
        // spec:1873-1876 — never empty merely because a disable makes
        // new.active false.
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, false)], &[pw(1, true, false)], &IDS, false, &[])
                .expect("closure");
        assert_eq!(c.expected_completion(), &[1]);
    }

    #[test]
    fn an_inactive_to_inactive_member_owes_nothing_and_is_not_an_error() {
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, false)], &[pw(1, false, false)], &IDS, false, &[])
                .expect("closure");
        assert_eq!(c.closure(), &[1]);
        assert!(c.expected_completion().is_empty());
    }

    #[test]
    fn an_off_to_off_member_with_a_page_event_fails_construction() {
        // spec:583-591 — prepare_signaling() creates event state for every
        // closure member when the global flag is set, and the atomic check then
        // rejects the off-to-off one. Fail here, not at the kernel.
        let err = AtomicCrtcClosure::compute(
            &[crtc(1, true), crtc(2, false)],
            &[pw(1, true, true), pw(2, false, false)],
            &IDS,
            true,
            &[],
        )
        .expect_err("must not construct");
        assert_eq!(err, ClosureError::OffToOffWithPageEvent(2));
    }

    #[test]
    fn the_kernel_event_set_is_the_expected_set_only_when_the_flag_is_set() {
        let with =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, true, &[])
                .expect("closure");
        assert_eq!(with.kernel_event(), &[1]);
        let without =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
                .expect("closure");
        assert!(without.kernel_event().is_empty());
    }

    #[test]
    fn the_present_set_is_the_consumer_subset_of_the_event_set() {
        // spec:573-574 — events in the set difference are drained but create no
        // protocol completion.
        let c = AtomicCrtcClosure::compute(
            &[crtc(1, true), crtc(2, true)],
            &[pw(1, true, true), pw(2, true, true)],
            &IDS,
            true,
            &[1],
        )
        .expect("closure");
        assert_eq!(c.kernel_event(), &[1, 2]);
        assert_eq!(c.present_event(), &[1]);
    }

    #[test]
    fn a_present_consumer_outside_the_event_set_is_rejected_not_dropped() {
        let err =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, true, &[9])
                .expect_err("a consumer with no event must not construct");
        assert_eq!(err, ClosureError::PresentConsumerOutsideEventSet(9));
    }

    #[test]
    fn a_closure_member_with_no_power_row_is_an_error_not_an_assumption() {
        // Power is retained metadata, so an absent ACTIVE property is fine — an
        // absent power ROW is not, and must never default to off.
        let err = AtomicCrtcClosure::compute(&[crtc(1, true)], &[], &IDS, false, &[])
            .expect_err("an unknown powered state must not default");
        assert_eq!(err, ClosureError::UnknownPower(1));
    }

    #[test]
    fn an_out_fence_the_caller_supplied_is_refused() {
        // The builder is the only thing permitted to add one; that is what makes
        // "exactly one per expected CRTC" enforceable at all.
        let mut c1 = crtc(1, true);
        c1.props.push((IDS.out_fence_ptr, 0));
        let err = AtomicCrtcClosure::compute(&[c1], &[pw(1, true, true)], &IDS, false, &[])
            .expect_err("a caller-supplied out-fence must not construct");
        assert_eq!(err, ClosureError::UnsolicitedOutFence(1));
    }

    fn kinds() -> BTreeMap<u32, ObjectKind> {
        BTreeMap::from([
            (1, ObjectKind::Crtc),
            (2, ObjectKind::Crtc),
            (31, ObjectKind::Plane),
        ])
    }
    /// Assemble the four parallel vectors from `(object, &[(prop, value)])`.
    fn list(objs: &[(u32, &[(u32, u64)])]) -> AtomicPropertyList {
        let mut l = AtomicPropertyList {
            objects: Vec::new(),
            count_props: Vec::new(),
            props: Vec::new(),
            values: Vec::new(),
        };
        for (object, entries) in objs {
            l.objects.push(*object);
            l.count_props.push(entries.len() as u32);
            for (p, v) in *entries {
                l.props.push(*p);
                l.values.push(*v);
            }
        }
        l
    }

    #[test]
    fn the_rescan_accepts_a_list_matching_the_recorded_closure() {
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
                .expect("closure");
        let props = list(&[(1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)])]);
        c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
            .expect("matches");
    }

    #[test]
    fn the_rescan_demands_equality_not_containment() {
        // spec:569-570. A CRTC that appeared after the closure was recorded
        // changes what the kernel will touch.
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
                .expect("closure");
        let props = list(&[
            (1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)]),
            (2, &[(IDS.active, 1)]),
        ]);
        let err = c
            .verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
            .expect_err("must differ");
        assert!(matches!(err, ClosureError::SerializedClosureDiffers { .. }));
    }

    #[test]
    fn the_rescan_subtracts_exactly_the_old_binding_only_members() {
        // A detach's old endpoint cannot appear in the bytes, so equality is
        // demanded against the closure minus those members — not waived.
        let c = AtomicCrtcClosure::compute(
            &[crtc(1, false), crtc(2, true), plane(31, 1, 2)],
            &[pw(1, true, false), pw(2, true, true)],
            &IDS,
            false,
            &[],
        )
        .expect("closure");
        assert_eq!(c.closure(), &[1, 2]);
        let props = list(&[
            (1, &[(IDS.active, 0), (IDS.out_fence_ptr, u64::MAX)]),
            (2, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)]),
            (31, &[(IDS.crtc_id, 2)]),
        ]);
        c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
            .expect("matches");
    }

    #[test]
    fn the_rescan_rejects_a_duplicate_out_fence_on_one_crtc() {
        // A set would collapse these and pass. spec:564-566 says exactly one.
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
                .expect("closure");
        let props = list(&[(
            1,
            &[
                (IDS.active, 1),
                (IDS.out_fence_ptr, u64::MAX),
                (IDS.out_fence_ptr, u64::MAX),
            ],
        )]);
        let err = c
            .verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
            .expect_err("must reject");
        assert_eq!(err, ClosureError::DuplicateOutFence(1));
    }

    #[test]
    fn the_rescan_rejects_an_out_fence_outside_the_expected_set() {
        let c = AtomicCrtcClosure::compute(
            &[crtc(1, true), crtc(2, false)],
            &[pw(1, true, true), pw(2, false, false)],
            &IDS,
            false,
            &[],
        )
        .expect("closure");
        assert_eq!(c.expected_completion(), &[1]);
        let props = list(&[
            (1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)]),
            (2, &[(IDS.active, 0), (IDS.out_fence_ptr, u64::MAX)]),
        ]);
        let err = c
            .verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
            .expect_err("must reject");
        assert!(matches!(err, ClosureError::OutFenceCoverageDiffers { .. }));
    }

    #[test]
    fn a_validation_rescan_requires_no_fences_and_therefore_passes() {
        // The draft made every active ValidationOnly request fail its own
        // re-scan: it omitted the fences and then demanded them.
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
                .expect("closure");
        assert_eq!(c.expected_completion(), &[1]);
        let props = list(&[(1, &[(IDS.active, 1)])]);
        c.verify_serialized(&props, &kinds(), &IDS, FencePolicy::Forbidden)
            .expect("a validation carries no fences and that is correct");
    }

    #[test]
    fn a_validation_rescan_rejects_a_fence_that_slipped_in() {
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
                .expect("closure");
        let props = list(&[(1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)])]);
        let err = c
            .verify_serialized(&props, &kinds(), &IDS, FencePolicy::Forbidden)
            .expect_err("must reject");
        assert!(matches!(err, ClosureError::OutFenceCoverageDiffers { .. }));
    }

    #[test]
    fn the_rescan_rejects_a_list_whose_counts_do_not_describe_its_values() {
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
                .expect("closure");
        let mut props = list(&[(1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)])]);
        props.count_props[0] = 9;
        let err = c
            .verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
            .expect_err("must be malformed");
        assert!(matches!(err, ClosureError::MalformedPropertyList(_)));
    }

    #[test]
    fn the_rescan_rejects_an_object_of_unknown_kind() {
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
                .expect("closure");
        let props = list(&[
            (1, &[(IDS.active, 1), (IDS.out_fence_ptr, u64::MAX)]),
            (99, &[(IDS.crtc_id, 1)]),
        ]);
        let err = c
            .verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
            .expect_err("must reject");
        assert_eq!(err, ClosureError::UnknownObject(99));
    }

    #[test]
    fn the_rescan_rejects_active_contradicting_recorded_power() {
        let c =
            AtomicCrtcClosure::compute(&[crtc(1, true)], &[pw(1, true, true)], &IDS, false, &[])
                .expect("closure");
        let props = list(&[(1, &[(IDS.active, 0), (IDS.out_fence_ptr, u64::MAX)])]);
        let err = c
            .verify_serialized(&props, &kinds(), &IDS, FencePolicy::Required)
            .expect_err("must reject");
        assert_eq!(
            err,
            ClosureError::ActiveContradictsPower {
                crtc: 1,
                serialized: false,
                recorded: true,
            }
        );
    }
}
