# Stage 3b-i-1 plan — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-1-plan-modeset-execution.md`
revision 1 (`e0fb4198`), first review.

**Result:** 0 blocking, 3 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):**

- **M-1 — CONFIRMED,** and the design had the same contradiction (§3.4 vs §3.2).
  Supersession wins; design revision 9 and plan revision 2 corrected.
- **M-2 — CONFIRMED.** The completion looks up the current output by key
  (`scene.rs:4202`). Plan revision 2 adds `OutputInstanceId` and a test across
  two same-key replacements.
- **M-3 — CONFIRMED.** `begin_crtc_config` branches on `mode = None` before the
  VT check (`backend.rs:24429`). Plan revision 2 refuses every form at begin.
- **Loop closed:** no blocking finding; the plan proceeds to implementation.

## Verdict

**0 blocking, 3 major, 0 minor.**  
**Coverage: COMPLETE FOR DECLARED SCOPE.**

## Incorporation audit

| Prior findings | Status |
| --- | --- |
| None; this is the first review. | Check 1 skipped. |

## Findings

### Major

**M-1 — DPMS change has two incompatible pre-dispatch outcomes.** The plan says a `REC-4` event projected before dispatch supersedes the modeset, releases its prepared set, and returns `Superseded` ([plan:126](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-1-plan-modeset-execution.md:126); [spec:150](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:150)). Its staged-projection test instead requires a DPMS-on arriving before dispatch to make the entry stale and **re-prepare** it ([plan:212](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-1-plan-modeset-execution.md:212); [spec:258](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:258)). A normal DPMS-on is the `REC-4` event in that sequence, so both test outcomes cannot hold. Specify the distinct circumstance in which re-preparation is allowed, or make this test expect supersession and test epoch freshness through a separate scenario.

**M-2 — A late job needs an exact retired-owner identity when the output key is reused.** The plan keys bundles by `OutputKey` plus retirement ID but only says completions resolve through “the job’s identity”; its late-completion test does not require an active replacement with the **same** `OutputKey` ([plan:159](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-1-plan-modeset-execution.md:159); [plan:183](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-1-plan-modeset-execution.md:183)). Today a completion carries `job_id` and `output_key`, then looks up the current output by key ([scene.rs:4202](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:4202)). After two mode changes, one key can name a current state and multiple bundles. A late job could service the wrong owner or lose its release proof. Require a job-to-scene/pool-instance binding that survives retirement, and test a late completion across successive same-key replacements. This makes the spec’s bundle-routing requirement verifiable ([spec:436](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:436)).

**M-3 — Immediate VT-release refusal is unassigned for all request forms.** The spec requires a request made while the seat is released to fail at once, without parking ([spec:144](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:144)). The plan guards *dispatch* on `Owner ∧ Ready` and names `SeatReleased`, but does not assign a begin-time check or a test for it ([plan:122](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-1-plan-modeset-execution.md:122); [plan:294](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-1-plan-modeset-execution.md:294)). This matters especially for disable: the current `begin_crtc_config` branches on `mode=None` before its VT check ([backend.rs:24429](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24429)). Require the Owner begin path to reject enable, change, and disable while released before taking a slot; test each relevant form.

### Blocking

None.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** no prior review.
- **Architecture:** checked the slot, topology admission, Owner event routing, and core CRTC token handoff. The three findings concern conflicting outcomes or missing identity/admission contracts.
- **Safety:** checked retired-state ownership, completion correlation, release obligations, and the validation-to-dispatch handoff. I did not audit every resource path.
- **Spec and evidence:** checked the in-scope transaction, clocks, promotion, failure table, and named tests. The plan assigns formatting, CI-style clippy, feature, library, integration, and hardware gates. No gate was run here.

**Excerpts used: 24/24** beyond the plan; no prior review existed. Copied-route, two-device, position-only, and RANDR-gate behavior were outside this pass. Compiler correctness, test results, and portability remain for implementation verification; this verdict does not approve execution.