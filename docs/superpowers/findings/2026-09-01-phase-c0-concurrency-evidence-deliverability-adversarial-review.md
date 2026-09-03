# Phase C.0 concurrency, evidence and deliverability adversarial review

**Date:** 2026-09-01
**Reviewer:** Opus
**Reviewed spec:** Draft
`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
**Kernel basis:** local Linux tree at
`77cb8f24c2381a8abb7272d7bbdec548d6426a8a`
**Disposition:** Partially incorporated after technical verification and user
approval

## Verified corrections to the review

### The cursor-disjoint concurrency premise remains valid

The review's blocking B-1 quoted an outstanding-CRTC-commit check that is not in
`drm_atomic_helper_async_check()` at the pinned kernel revision. The function
checks `old_plane_state->commit` and its `hw_done`, then delegates to the
driver's `atomic_async_check`; it does not reject from
`old_crtc_state->commit`. `drm_atomic_helper_setup_commit()` assigns a commit to
planes enumerated in the atomic state. A primary transaction that omits the
unchanged cursor therefore does not attach that transaction's plane commit to
the cursor merely because both use the same CRTC.

The source argument is now normative in section 4.1. Driver-specific checks or
affected-plane expansion can still reject concurrent movement, so the required
production omitted-shape hardware arm remains. The non-production absorbed
shape no longer decides the architecture or merge.

### The claimed NVIDIA environment lever is absent from the merged code

A full-tree search found `YSERVER_HW_CURSOR_NVIDIA` only in historical
documentation/findings, not in the merged Rust sources or production runtime.
There is therefore no section 5 code-conversion item to remove. Goal 11 and the
no-runtime-lever acceptance rule remain.

### Delivery remains one PR

The suggested substrate and gamma PR split conflicts with the approved delivery
decision. C.0 remains one PR and one confirmed squash merge with four internal
review stages. Evidence reuse is improved through explicit tip-sensitivity
classification rather than by creating supported partial merge states.

## Incorporated blocking and major findings

1. **Open versus opaque wait evidence.** Reachable waits in open components
   produce `AuditedOpenWaitMax`. Proprietary NVKMS is explicitly opaque and uses
   a 30-second operational return observation plus `ObservedOpaqueWaitMax`;
   unavailable source no longer makes the RTX gate unsatisfiable.
2. **Single-slot multi-CRTC ceiling.** The N=2 gate measures a matched N=1
   physical rate `R1`, derives `SingleSlotCeiling`, then requires at least 45%
   per CRTC and 90% aggregate of that ceiling. Full aggregate refresh is not a
   C.0 promise. A future independent **Multi-CRTC Parallel Retirement** design
   may lift the limitation.
3. **Absorbed shape demotion.** `cursor-unchanged-absorbed` is optional bounded
   characterization. It has no phase scheduler, quota, merge effect or evidence
   outcome. Only the production cursor-omitted composed/direct strata are
   required, reducing the capped campaign by about 2 hours 19 minutes per
   device.
4. **In-process residual risk.** `InProcessQualified` explicitly accepts that a
   future ioctl outside finite evidence can wedge the server. Structural prompt
   VT/device-loss/logical-withdrawal containment belongs only to the process-
   isolated branch; under in-process execution those properties are empirical
   for validated cohorts.
5. **Capability layer split.** Discovery sets
   `atomic_kms_pipeline_structurally_capable`; the immutable release table sets
   `atomic_kms_cohort_validated`. C.1 admission requires both. A failed NVIDIA
   policy arm leaves cohort validation false without rewriting discovered
   structure.
6. **Software clock deferred.** C.0 ships only `KernelSequence`.
   `GET_SEQUENCE=EOPNOTSUPP` closes structural capability and qualification;
   deterministic tests alone cannot enable an unvalidated software-clock path.

## Incorporated correctness and process findings

7. Topology/install/recovery completion has a lifecycle-specific deadline
   derived from observed cohort evidence and capped at 30 seconds. The physical
   matrix includes a deliberately slow sink or controlled equivalent.
8. `OwnerMediatedLegacyMove` viability comes from immutable offline cohort
   evidence. Runtime ioctl return latency is not treated as proof that the
   driver's async hook executed.
9. Out-fence and page-event milestones remain typed separately, but the spec now
   records that mainline helpers back them with one
   `drm_pending_vblank_event`; wakeup order is defensive bookkeeping, not an
   expected hardware-time separation.
10. A non-desktop-only card uses a discovery-only fd/udev path so connector
    class can be observed without admitting live KMS mutations. If discovery
    cannot be maintained, a restart is required rather than claiming a runtime
    transition.
11. `EventToken` exhaustion uses checked increment and a debug assertion. The
    unreachable-timescale branch has no administrative restart or incarnation
    replacement machinery.
12. Gamma-unavailable hardware validation includes `redshift`, `gammastep` and
    a Proton title that sets gamma. Failure reopens the representation decision;
    C.0 does not invent a fake LUT.
13. Evidence is classified as tip-sensitive physical evidence or reusable
    deterministic evidence. Reuse requires a documented diff-scope proof;
    each manifest row records tested source paths, dependency/build hashes and
    its invalidation rationale. Final automated tests still run. A review change
    invalidates only affected classes, with unexplained cross-cutting changes
    invalidating all physical rows.
14. Hardware ownership is explicit: the author schedules the RTX 5060 Ti and
    the maintainer schedules the RX 6800 XT. Loss of either required device
    yields `EvidenceInsufficient`; a substitute Navi 21 board requires a spec
    revision and complete cohort evidence.

## Superseded prior measurement machinery

This disposition supersedes the absorbed-shape phase-targeting and weighted
quota machinery recorded in items 7 and 10–17 of
`2026-09-01-phase-c0-post-incorporation-adversarial-review.md`. Its item 20
cohort-wide audited-source deadline is replaced by the open/opaque split above.
Item 24's one-PR decision remains, while its all-evidence-identical-tip rule is
refined by the section 18 tip-sensitivity manifest. Item 7 of
`2026-09-01-phase-c0-coordinate-concurrency-adversarial-review.md` is superseded
by deferral of `FlipDrivenSoftware`. The production omitted-shape quota, fixed
buffer, corroborated returnability protocol, four evidence outcomes and global
binary architecture choice remain normative.
