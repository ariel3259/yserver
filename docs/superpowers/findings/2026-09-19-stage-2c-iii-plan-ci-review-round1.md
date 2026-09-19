# Stage 2c-iii plan Ci — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md`
revision 1 (`a151bfcc`), against the 2c-iii design revision 4 + §4.6 (`23f3fd0b`).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
**Coverage: INCOMPLETE** (24/24). The open question: whether the live Vulkan
fixture can drive tick → render-completion drain → offer without a manual offer.
Revision 2 makes that an explicit F8 point in Task 4.

**Author verification (2026-09-19), every finding checked against the tree:**

- **B-1 — CONFIRMED.** Decision 7 freed a displaced buffer after its render,
  while Task 7 required `CompletionRetired` + `KmsRelease` for every owner buffer,
  and a never-admitted one gets neither. `BoPhase` (`vk/scanout.rs:114`) ties
  `Submitted`/`Pending` to the legacy flip's IN/OUT fences
  (`scene.rs:7994`–`8034`, `platform.rs:7260`–`7315`), and revision 1 mapped no
  owner transitions. Fixed: decision 10, one lifecycle table.
- **M-1 — CONFIRMED.** Only successful composed retirement was tested; the
  `ResourcesStillCurrent` arm and the direct dispatch keep whole-state
  replacement today. Fixed: two tests, R27–R28.
- **M-2 — CONFIRMED.** Only the copied case was tested. Fixed: unmanaged and
  mixed-output tests, R29–R30.
- **M-3 — CONFIRMED.** Spec §8.2 names "apply at `Presented`"; revision 1
  mutated `Accepted`. Only `CompletionUnknown` was tested among the invalidation
  sources. Fixed: R15 now matches the spec; per-source test, R31, with an F8
  for any source without a signal.
- **M-4 — CONFIRMED.** `PendingAck` is per output index, while
  `register_scanout_render_completion` (`platform.rs:4644`) already keys by
  `OutputKey`. Fixed: decision 11, R32.

Revision 2 incorporates all five.

---

## Verdict

**1 blocking, 4 major, 0 minor**

Coverage: **INCOMPLETE**

This is a design-review result only; it does not claim compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Status |
|---|---|
| — | No prior review exists; check 1 was skipped as instructed. |

## Findings

### Blocking

#### B-1 — The owner BO lifecycle deadlocks generations displaced before admission

The plan gives contradictory release rules. A prepared generation displaced before admission must return its buffer after rendering completes ([plan:25](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:25), [plan:136](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:136); spec §4.1, lines 297–307). Task 7 instead says every owner composed buffer requires `CompletionRetired`, a discharged `KmsRelease`, and GPU retirement before becoming free ([plan:196](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:196)).

A generation displaced before admission has no `CommitId`, was never current, and can receive neither `CompletionRetired` nor a commit-keyed `KmsRelease`. It therefore either leaks permanently or must be freed by violating Task 7.

This also intersects an undefined phase transition. The existing managed submission leaves the BO `Submitted` and only moves it to `Pending` after a successful legacy flip (`scene.rs:7994–8034`); page completion later performs `Pending → OnScreen` and retires the previous BO (`platform.rs:7260–7315`). Removing the flip leaves no specified owner transition for the prepared, accepted, current, displaced, or rejected cases.

Required correction: add an authoritative owner BO state table. Explicitly exempt never-admitted generations from KMS gates and define their render-completion/GPU-retirement path back to `Free`; separately map `Accepted`, `HardwareComplete`, `CompletionRetired`, resource availability, rejection, and invalidation onto submitted/current/retiring BOs and producer-fence ownership. Extend the displacement test to prove release without fabricating a retirement event.

### Major

#### M-1 — Per-member preservation is untested on failure and direct dispatch

Task 2 correctly states that unrelated current members survive retirement, rejection, and `ResourcesStillCurrent`, on both primary and direct dispatch ([plan:95](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:95); spec §4.6, lines 386–394). Its only two-CRTC test exercises successful composed retirement ([plan:105](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:105)).

Today `CompletionRetired` and `ResourcesStillCurrent` replace the entire current vector (`commit.rs:245–311`), while both primary and direct dispatch take the whole current state (`admission.rs:668–738`, `791–838`). An implementation could fix only composed retirement while leaving rejection or direct dispatch destructive; every named test would still pass.

Required correction: add two-CRTC tests and mutations for `ResourcesStillCurrent`/rejection and direct dispatch, proving B remains current and obligation-free in each arm.

#### M-2 — Owner eligibility evidence omits unmanaged pools and device-wide scope

The normative rule forbids `Owner` when **any** device output is copied or unmanaged (spec §4.6, lines 372–375; [plan:137](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:137)). The sole test and mutation cover only a copied output ([plan:56](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:56), [plan:144](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:144)).

A predicate checking copied routes but accepting unmanaged pools—or checking only the initiating output rather than every device output—survives. That permits `Owner` while an unconverted producer still requires the legacy route.

Required correction: add unmanaged-pool and mixed-output device tests, plus independent mutations for omitting each device-wide scan.

#### M-3 — Damage evidence does not establish the full milestone table

The spec requires apply only at `HardwareComplete`, with `Presented` doing nothing, and invalidation for incarnation, recovery, topology, VT release, and device loss (spec §4.2, lines 315–337). The plan mutates apply-at-`Accepted`, whereas the spec’s required mutation is apply-at-`Presented` ([plan:62](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:62); spec §8.2, lines 555–564). It tests only `CompletionUnknown`, not the enumerated invalidation sources ([plan:64](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:64), [plan:186](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:186)).

Thus an implementation can apply twice—at `HardwareComplete` and `Presented`—or leave topology/VT transactions live while all named mutations pass.

Required correction: restore the `Presented` mutation and assert no second damage/history/cursor action; add parameterized evidence for every invalidation source, including `owes_repaint`.

#### M-4 — Transaction entries lack a stable output-identity contract

The spec identifies a transaction by the exact `(output, buffer index, generation)` set (spec §4.2, lines 309–312). Task 6 lists buffer and generation but does not require a stable output key or define topology-invalidation ordering ([plan:174](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:174)). Existing render completions deliberately use device-qualified `OutputKey` so vector reordering cannot retarget completion (`platform.rs:4644–4665`).

If transaction members retain output indices, a topology reorder before `HardwareComplete` can ack and mutate the wrong output. Permuted completion tests without topology change do not catch this.

Required correction: require stable `OutputKey`/CRTC identity plus generation validation, define invalidation-before-reindex ordering, and test a stale event across topology change.

### Minor

None.

## Coverage and implementation checks

- Incorporation: skipped; no prior review.
- Architecture: task ordering is broadly coherent, but BO lifecycle and transaction identity are not.
- Safety/ownership: checked registration rollback intent, per-member replacement, render-completion ownership, BO phases, rejection, and event routing.
- Verification: checked named tests and R1–R26 against the relevant spec rows; found the failure/direct, eligibility, Presented, and invalidation gaps.
- Excerpts used: **24/24** — six authoritative-spec excerpts and eighteen bounded source excerpts.
- Verified ground: current whole-state dispatch/consumption, shared managed flip coupling, render-completion registry, BO phase machine, sole owner-event batch router, and the existing Vulkan managed-pool fixture.
- Unassessed: the upstream C.0/stage-2c authority sections were not independently reread, and constructor-level feasibility of combining the live Vulkan scene fixture with a `KmsBackend` owner/conductor remains unresolved. The specific follow-up question is whether that fixture can drive the real tick-to-drain-to-`admission_offer_composed` seam without a manual test-only offer.
- Exact Rust signatures, borrows, trait bounds, compilation, clippy, portability, and runtime test behavior remain deferred to implementation.