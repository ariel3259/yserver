# Stage 3b design — codex review, round 1

**Target:** `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
revision 1 (`0252f805`), first review.

**Result:** 2 blocking, 3 major, 0 minor; coverage INCOMPLETE (24/24; the clock
bootstrap of a first enable unassessed).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):**

- **B-1 — CONFIRMED.** C.0 §9.4 (spec line 1657): atomic `EBUSY` with no
  owner-tracked live record is an ownership invariant failure — close
  readiness, record the evidence, no immediate retry. Revision 2 replaces the
  retry row.
- **B-2 — CONFIRMED.** The bound omitted the wait at the gate and the composed
  unflip before dispatch. Revision 2 adds a gate queue deadline for parkable
  requests, puts the unflip in the table with its terminal handoff, and proves
  the whole-request bound.
- **M-1 — CONFIRMED.** Legacy treats a position change as non-idempotent
  (`backend.rs:24821`); a position-only change alters no KMS property on this
  server (the output's root offset is composition state). Revision 2 defines
  it as a logical transaction with no commit.
- **M-2 — CONFIRMED.** Revision 2 builds the new scene states during
  preparation; promotion is an infallible swap and identity remap, and the
  post-install scene-failure row is gone (renderer loss stays global).
- **M-3 — CONFIRMED.** Revision 2 states that `REC-4` events reach the arbiter
  outside the gate; only publication is ordered, and a requester-less
  publication waits only behind a dispatched mutation.
- **Unassessed question — a real defect, found by the author.** A first enable
  (or a DPMS-on after a mode installed dark) targets a CRTC that is inactive.
  In the kernel `GET_SEQUENCE` on such a CRTC fails: `drm_crtc_vblank_off`
  bumps the refcount and sets `inmodeset` (`drm_vblank.c:1372`), so
  `drm_vblank_get` returns `-EINVAL` (`:1228`). C.0 §10 requires a current
  clock only before an **event-bearing** commit, but the 2b owner requires one
  for every expected-completion CRTC of a `LifecycleInstallRestore` commit
  (`owner/device.rs` `validate_completion_context`, and
  `lifecycle_clock_readiness`), so the enable could never dispatch, and a
  probe of the dark CRTC would close qualification. Revision 2 narrows the
  requirement to C.0's text for lifecycle commits (section 3.5).

## Verdict

**2 blocking, 3 major, 0 minor.** Coverage: **INCOMPLETE**. This is a design review, not a claim that the design compiles, tests pass, or is approved for implementation.

## Incorporation audit

| Prior review | Status |
| --- | --- |
| None | Skipped as instructed. |

## Findings

### Blocking

**B-1 — Atomic `EBUSY` must not be retried.** The [3b failure table](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:318) prescribes a bounded retry. [C.0 §9.4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1657) says atomic `EBUSY` without an owner-tracked live record is an ownership failure: close readiness, enter the bounded topology/recovery path, and do not immediately retry. If a foreign commit occupies the device, the proposed retry submits against state the Owner cannot account for. Replace the retry row with C.0’s readiness-closure and handoff rule.

**B-2 — The stated request bound omits waits before commit.** The [~95 s bound](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:408) counts PRIME, lifecycle/clock, host call and hardware completion. It excludes waiting behind an earlier mutation at the [server-wide gate](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:350) and the required [composed unflip before dispatch](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:283). For example, client B can wait through client A’s near-deadline transaction, then through its own unflip and commit, exceeding the stated bound; an unflip that cannot retire has no specified terminal handoff to B. The [umbrella obligation](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:298) covers the whole parked request. Define a safe pre-dispatch queue deadline and propagate unflip completion or failure into the parked request.

### Major

**M-1 — Position-only `SetCrtcConfig` has no transaction contract.** Legacy treats a change in `x` or `y` as non-idempotent ([baseline guard](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24820)). The [3b transaction](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:154) specifies KMS property changes for enable, disable and mode change, but none for position alone. C.0 requires a [minimal changed-property list](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:560). A position-only request therefore has no defined way to reach installed success and publish Legacy-equivalent state. Specify whether it is a gate-ordered logical transaction or intentionally changes a real KMS resource, and test its reply, timestamps and events.

**M-2 — Scene failure can leave other devices indexed against the wrong outputs.** The design updates `platform.outputs` before a [fallible scene promotion](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:235), while acknowledging that insertion or removal shifts other devices’ indices and requires [reassociation by `OutputKey`](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:263). Its failure row nevertheless says those devices continue ([§6](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:323)). If rebuilding A fails after A’s insertion shifts B, B can continue with scene state at its old index. Require an infallible identity remap, or staged scene construction followed by an atomic mapping swap, before B can resume.

**M-3 — Requester-less changes lack an event-versus-gate ordering rule.** A pre-dispatch modeset must be superseded promptly by `REC-4` ([3b §3.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:143); [umbrella driver contract](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:119)). Section [7.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:431) says a requester-less change enters the same occupied gate, but does not say whether its `REC-4` event reaches the arbiter before gate admission. If the event waits, a client modeset parked on a prerequisite can prevent the event that should supersede it. State that event handling and supersession proceed outside the gate, while their publication waits for its ordered turn; test that sequence with the proposed producer.

## Coverage and implementation checks

Incorporation: no prior review. Architecture: checked the gate, driver/result boundary and sampled core ready-ring behavior. Safety: checked `EBUSY`, request deadlines, resource and scene handoffs. Compliance: checked the six umbrella RANDR obligations, relevant C.0 rules and proposed evidence. **24/24 bounded spec/source excerpts** were used; no builds or tests were run. The design assigns formatting, clippy and test gates through umbrella §5.3; real compilation, tests and any changed ioctl portability checks remain implementation work.

The unresolved follow-up question is whether a first enable on a previously inactive CRTC can satisfy 3b’s **pre-dispatch current-clock requirement**, and, if it cannot, what bounded path permits that enable. The excerpt limit prevented checking that clock bootstrap contract; it is unassessed, not sound.