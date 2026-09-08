//! Turning a commit description into the exact request 2a puts on the wire.
//!
//! Order is normative, not stylistic. The closure is computed from the
//! description *before* any completion property exists, the out-fence
//! entries are appended, and only then is the serialized list re-scanned.
//! Computing the closure afterwards would let an ephemeral out-fence entry
//! enlarge it, which spec:592-595 forbids by name.

use std::collections::BTreeMap;

use crate::kms::{
    executor::{
        HostCallClass,
        protocol::{
            AtomicPropertyList, AtomicRequest, DRM_MODE_ATOMIC_ALLOW_MODESET,
            DRM_MODE_ATOMIC_NONBLOCK, DRM_MODE_ATOMIC_TEST_ONLY, DRM_MODE_PAGE_FLIP_EVENT,
            HostCallCorrelation, OutFenceSlot, ProtocolError,
        },
    },
    owner::closure::{
        AtomicCrtcClosure, ClosureError, CrtcPower, FencePolicy, ObjectKind, PropertyIds,
        SerializedObject,
    },
};

#[derive(Debug, Clone)]
pub struct CommitDescription {
    /// The minimal persistent property list — `spec:551-556`.
    pub objects: Vec<SerializedObject>,
    /// Retained powered state for every closure member. Not serialized; see
    /// `CrtcPower`. The re-scan cross-checks it against any `ACTIVE` that is.
    pub crtc_state: Vec<CrtcPower>,
    pub present_consumers: Vec<u32>,
    pub page_flip_event: bool,
    pub property_ids: PropertyIds,
}

impl CommitDescription {
    /// The object-kind map the re-scan needs, derived from the same objects
    /// the closure was computed from. There is deliberately no way to supply
    /// a different one.
    fn kinds(&self) -> BTreeMap<u32, ObjectKind> {
        self.objects.iter().map(|o| (o.object, o.kind)).collect()
    }
}

/// Do two built requests carry the same persistent properties?
///
/// Compares the four parallel arrays with every `OUT_FENCE_PTR` entry and its
/// value removed, because that is exactly and only how a live request differs
/// from the `TEST_ONLY` that validated it. Flags are excluded for the same
/// reason. Anything else differing means the lease would be certifying a
/// request nobody checked.
pub fn same_persistent_properties(
    a: &AtomicRequest,
    b: &AtomicRequest,
    out_fence_ptr: u32,
) -> bool {
    fn persistent(r: &AtomicRequest, out_fence_ptr: u32) -> Vec<(u32, u32, u64)> {
        let mut flat = Vec::new();
        let mut cursor = 0usize;
        for (index, object) in r.properties.objects.iter().enumerate() {
            let count = r.properties.count_props[index] as usize;
            for offset in 0..count {
                let prop = r.properties.props[cursor + offset];
                if prop != out_fence_ptr {
                    flat.push((*object, prop, r.properties.values[cursor + offset]));
                }
            }
            cursor += count;
        }
        flat
    }
    // The caller passes the id from the live description's `PropertyIds`;
    // both sides were built against the same device, so one id serves for
    // both. Ordering is preserved rather than sorted: two lists that carry
    // the same properties in a different order are different requests.
    persistent(a, out_fence_ptr) == persistent(b, out_fence_ptr)
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("closure: {0}")]
    Closure(#[from] ClosureError),
    #[error("protocol: {0:?}")]
    Protocol(ProtocolError),
}

pub fn build_atomic_request(
    desc: &CommitDescription,
    correlation: HostCallCorrelation,
    class: HostCallClass,
) -> Result<(AtomicRequest, AtomicCrtcClosure), BuildError> {
    build_atomic_request_with_modeset(desc, correlation, class, false)
}

pub fn build_atomic_request_with_modeset(
    desc: &CommitDescription,
    correlation: HostCallCorrelation,
    class: HostCallClass,
    allow_modeset: bool,
) -> Result<(AtomicRequest, AtomicCrtcClosure), BuildError> {
    let fences = if class.is_validation() {
        FencePolicy::Forbidden
    } else {
        FencePolicy::Required
    };
    // A validation never carries a page event, so the closure it is checked
    // against must be computed without one. spec:320-323.
    let page_flip_event = desc.page_flip_event && fences == FencePolicy::Required;

    let closure = AtomicCrtcClosure::compute(
        &desc.objects,
        &desc.crtc_state,
        &desc.property_ids,
        page_flip_event,
        &desc.present_consumers,
    )?;

    let mut objects = Vec::new();
    let mut count_props = Vec::new();
    let mut props = Vec::new();
    let mut values = Vec::new();
    let mut out_fence_slots = Vec::new();

    for object in &desc.objects {
        let wants_fence = fences == FencePolicy::Required
            && object.kind == ObjectKind::Crtc
            && closure.expected_completion().contains(&object.object);

        objects.push(object.object);
        count_props.push((object.props.len() + usize::from(wants_fence)) as u32);
        for (prop, value) in &object.props {
            props.push(*prop);
            values.push(*value);
        }
        if wants_fence {
            out_fence_slots.push(OutFenceSlot {
                crtc_id: object.object,
                value_index: values.len() as u32,
            });
            props.push(desc.property_ids.out_fence_ptr);
            // The holder is initialized to -1 by the helper; this value is
            // overwritten with a pointer to it before the ioctl.
            values.push(u64::MAX);
        }
    }

    let properties = AtomicPropertyList {
        objects,
        count_props,
        props,
        values,
    };
    properties.validate().map_err(BuildError::Protocol)?;
    closure.verify_serialized(&properties, &desc.kinds(), &desc.property_ids, fences)?;

    let mut flags = match class {
        HostCallClass::SeatActiveNonblock => DRM_MODE_ATOMIC_NONBLOCK,
        HostCallClass::SeatActiveValidation | HostCallClass::ColdStartOrOfflineValidation => {
            DRM_MODE_ATOMIC_TEST_ONLY
        }
        HostCallClass::ColdStartOrOfflineBlocking => 0,
    };
    if page_flip_event {
        flags |= DRM_MODE_PAGE_FLIP_EVENT;
    }
    if allow_modeset {
        flags |= DRM_MODE_ATOMIC_ALLOW_MODESET;
    }

    Ok((
        AtomicRequest {
            correlation,
            class,
            flags,
            properties,
            out_fence_slots,
        },
        closure,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::executor::{
        HostCallClass, HostCallRequest,
        protocol::{DRM_MODE_ATOMIC_NONBLOCK, DRM_MODE_ATOMIC_TEST_ONLY, DRM_MODE_PAGE_FLIP_EVENT},
    };

    fn flatten(properties: &AtomicPropertyList) -> Vec<(u32, u32)> {
        let mut flat = Vec::new();
        let mut cursor = 0usize;
        for (index, object) in properties.objects.iter().enumerate() {
            let count = properties.count_props[index] as usize;
            for offset in 0..count {
                flat.push((*object, properties.props[cursor + offset]));
            }
            cursor += count;
        }
        flat
    }

    fn correlation(n: u64) -> HostCallCorrelation {
        super::super::test_fixtures::atomic_correlation_for_tests(n)
    }

    fn description_with_too_many_properties() -> CommitDescription {
        let mut desc = super::super::test_fixtures::single_active_crtc();
        let mut props = Vec::with_capacity(1025);
        for i in 0..1025 {
            props.push((100 + i, 0));
        }
        desc.objects = vec![SerializedObject {
            object: 1,
            kind: ObjectKind::Crtc,
            old_crtc_id: None,
            props,
        }];
        desc
    }

    #[test]
    fn one_out_fence_is_added_for_each_expected_completion_crtc_and_none_beyond() {
        let desc = super::super::test_fixtures::two_crtcs_one_off();
        let (req, closure) =
            build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
                .expect("build");
        assert_eq!(closure.expected_completion(), &[1]);
        assert_eq!(req.out_fence_slots.len(), 1);
        assert_eq!(req.out_fence_slots[0].crtc_id, 1);
    }

    #[test]
    fn every_slot_indexes_the_value_the_helper_will_overwrite() {
        // The helper replaces properties.values[value_index] with a POINTER to
        // its own holder storage before the ioctl (helper.rs:216-220), and the
        // kernel writes the fd into that holder (helper.rs:263-270). A misaimed
        // index therefore overwrites an unrelated property's value with a
        // pointer — which is why this assertion is load-bearing rather than
        // cosmetic.
        let desc = super::super::test_fixtures::single_active_crtc();
        let (req, _) =
            build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
                .expect("build");
        let slot = req.out_fence_slots[0];
        let flat = flatten(&req.properties);
        assert_eq!(
            flat[slot.value_index as usize],
            (1u32, desc.property_ids.out_fence_ptr),
            "the slot must index this CRTC's OUT_FENCE_PTR entry"
        );
    }

    #[test]
    fn a_validation_request_carries_no_out_fence_and_still_builds() {
        // The first draft made every active ValidationOnly request fail its own
        // re-scan. This is the regression guard.
        let desc = super::super::test_fixtures::single_active_crtc();
        let (req, closure) =
            build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveValidation)
                .expect("a validation over an active CRTC must build");
        assert_eq!(
            closure.expected_completion(),
            &[1],
            "the closure is unchanged"
        );
        assert!(req.out_fence_slots.is_empty());
        assert_ne!(req.flags & DRM_MODE_ATOMIC_TEST_ONLY, 0);
        assert_eq!(req.flags & DRM_MODE_ATOMIC_NONBLOCK, 0, "spec:320-322");
        assert_eq!(req.flags & DRM_MODE_PAGE_FLIP_EVENT, 0);
    }

    #[test]
    fn a_validation_never_carries_a_page_event_even_when_asked() {
        let mut desc = super::super::test_fixtures::single_active_crtc();
        desc.page_flip_event = true;
        let (req, _) =
            build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveValidation)
                .expect("build");
        assert_eq!(req.flags & DRM_MODE_PAGE_FLIP_EVENT, 0);
    }

    #[test]
    fn a_seat_active_commit_carries_nonblock() {
        let desc = super::super::test_fixtures::single_active_crtc();
        let (req, _) =
            build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
                .expect("build");
        assert_ne!(req.flags & DRM_MODE_ATOMIC_NONBLOCK, 0);
    }

    #[test]
    fn the_page_event_flag_and_the_kernel_event_set_agree() {
        let mut desc = super::super::test_fixtures::single_active_crtc();
        desc.page_flip_event = true;
        let (req, closure) =
            build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
                .expect("build");
        assert_ne!(req.flags & DRM_MODE_PAGE_FLIP_EVENT, 0);
        assert_eq!(closure.kernel_event(), &[1]);
    }

    #[test]
    fn construction_fails_before_submit_on_an_off_to_off_crtc_with_a_page_event() {
        let mut desc = super::super::test_fixtures::two_crtcs_one_off();
        desc.page_flip_event = true;
        let err = build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
            .expect_err("must not build");
        assert!(matches!(
            err,
            BuildError::Closure(ClosureError::OffToOffWithPageEvent(2))
        ));
    }

    #[test]
    fn the_request_carries_the_records_correlation_verbatim() {
        // spec:1673-1678 — user_data carries the commit's EventToken. 2a's
        // helper reads it out of the correlation, so the tuple handed over must
        // be the record's.
        let desc = super::super::test_fixtures::single_active_crtc();
        let c = correlation(7);
        let (req, _) =
            build_atomic_request(&desc, c, HostCallClass::SeatActiveNonblock).expect("build");
        assert_eq!(req.correlation, c);
    }

    #[test]
    fn an_oversized_property_list_fails_construction_not_encoding() {
        let desc = description_with_too_many_properties();
        let err = build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
            .expect_err("must fail here");
        assert!(matches!(err, BuildError::Protocol(_)));
    }

    #[test]
    fn same_persistent_properties_ignores_out_fences_and_flags() {
        let desc = super::super::test_fixtures::single_active_crtc();
        let (live_req, _) =
            build_atomic_request(&desc, correlation(1), HostCallClass::SeatActiveNonblock)
                .expect("live");
        let (val_req, _) =
            build_atomic_request(&desc, correlation(2), HostCallClass::SeatActiveValidation)
                .expect("val");
        assert!(same_persistent_properties(
            &live_req,
            &val_req,
            desc.property_ids.out_fence_ptr
        ));

        let mut diff_desc = desc.clone();
        diff_desc.objects[0].props[0].1 = 999;
        let (diff_req, _) = build_atomic_request(
            &diff_desc,
            correlation(3),
            HostCallClass::SeatActiveNonblock,
        )
        .expect("diff");
        assert!(!same_persistent_properties(
            &live_req,
            &diff_req,
            desc.property_ids.out_fence_ptr
        ));
    }

    #[test]
    fn fixture_request_builds() {
        let req = super::super::test_fixtures::request_for_tests();
        assert!(matches!(req, HostCallRequest::Atomic(_)));
    }
}
