# Stage 2c-iii design — codex review, round 2

**Target:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`
revision 2 (`89169072`), with round 1 as the prior review.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Naming note (2026-09-19):** this record says C1, C2 and C3 for the design's
three plans. They were renamed **Ci, Cii and Ciii** in revision 3, so they
cannot be confused with Phases C.1 and C.2; the review text is left as written.
**Coverage: INCOMPLETE** (reported by the reviewer): the 2c-ii successor
replacement sequence and debt §§4.4/9.5 were not excerpted. Its open question —
whether the conductor moves the whole `CompletedPresentEvent` into accepted
direct state — the author checked: `managed_confirm_direct_dispatch`
(`backend.rs`) moves the whole `DirectPresentFrame`, event included, from
`queued_successor` to `pending`.

**Author verification (2026-09-19), every finding checked against the tree:**

- **B-1 — CONFIRMED.** Composited Presents complete through the GPU batch
  (`engine.rs:2777`–`2821`: `pending_present_completions` drained into a
  `PendingPresentBatch`), and the scene only sees a Boolean per output
  (`scene.rs:2013`). Revision 2's §3.3 gave them a second authority. Decision
  (author; the user may override): keep the GPU authority, as C.0 §12 preserves
  Phase A+B outcomes — composed commits are non-Present primaries, §3.3's
  carriage is direct-only, and the Present-carrying owner entry moves to C2 (§5.0).
- **B-2 — CONFIRMED.** Revision 2 created the transaction at confirmation,
  after `begin`/`send_on` had returned events. Fixed: installed inside the
  CommitId-aware ledger closure, events routed only after.
- **M-1 — CONFIRMED, and wider than reported.** No mutation dropped
  `PriorBufferReleased` alone. The author also found `PriorBufferReleased` is not
  an `OwnerEvent` in this code: it is the resource service's `KmsRelease`
  discharge. §4.2 now names the three gates in code terms; §8.2 drops each alone.
- Incorporation audit: B-1, B-2, M-3 APPLIED; M-1 TRADED (= this round's B-1);
  M-2 PARTIAL (= this round's M-1). Both closed in revision 3.

Revision 3 incorporates all three.

---

## Verdict

**2 blocking, 1 major, 0 minor**

**Coverage: INCOMPLETE**

This is a bounded design review, not a claim that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — `Dispatched` closed damage transaction | **APPLIED** | Section 4.2 now separates `Dispatched`, performs no damage action, and retains the transaction through `Accepted` ([target line 261](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:261>)). |
| B-2 — registration caller could be deferred | **APPLIED** | Sections 3.2 and 6.4 make a non-test caller on each converted owner path an acceptance condition and F8-stop rather than stage-3/4 debt ([target line 135](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:135>), [line 402](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:402>)). |
| M-1 — missing producer-to-owner Present contract | **TRADED** | Section 3.3 adds carriage and tests, but assigns composited Present requests to the KMS commit without resolving their existing renderer-completion ownership. This creates B-1 below. |
| M-2 — release gates not independently proven | **PARTIAL** | Section 8.2 adds three phrases, but two test the GPU-fence omission while no mutation independently removes `CompletionRetired` or `PriorBufferReleased`. See M-1. |
| M-3 — no unflip DMG-5 evidence | **APPLIED** | Section 6.1 requires per-output invalidation/repaint, and section 8.2 supplies unflip-specific mutations ([target line 371](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:371>), [line 446](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:446>)). |

## Findings

### Blocking

#### B-1 — Composed Present carriage has two competing terminalization owners

Section 3.3 requires a composed prepared intent to carry every Present request into the KMS owner and terminalize it from `Presented` ([target lines 186–204](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:186>)). Today, however, the compositor-facing predicate exposes only a Boolean derived from damaged drawable IDs, with no serial, FIFO position, target, event, or wake ([scene.rs lines 2013–2021](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2013>), [lines 4031–4039](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:4031>)). Actual `CompletedPresentEvent`s remain owned by `OpenFrame.pending_present_completions` and, after successful Vulkan submission, move into a GPU-completion batch ([engine.rs lines 2598–2607](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/engine.rs:2598>), [lines 2777–2821](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/engine.rs:2777>)).

Concrete failure: leaving an event in that batch while also attaching it to a composed KMS generation permits two completions/wake signals. Removing it leaves no specified transfer or correlation point, and latest-wins scene displacement can convert an already-rendered composited Present into the new §3.3 `Skip`. Inferring requests from `pending_presentation_for_output` is impossible because it contains no request identity. This violates exact-once, separately keyed terminalization ([stage-2c lines 119–125](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:119>); [C.0 lines 2260–2285](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2260>)).

Required correction: decide the authority explicitly. If composited Presents retain renderer/GPU completion, composed KMS commits are non-Present primaries and must not manufacture `Presented` completion ([C.0 lines 2136–2147](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2136>)). Otherwise specify an ownership-moving transfer from the renderer queue into one admitted scene generation, including output/CRTC correlation, displacement semantics, and removal from the old batch.

#### B-2 — The damage transaction is created after the point by which it must be retained

Section 4.2 says the transaction is “created when the conductor confirms” admission, is keyed by `CommitId`, and is retained before dispatch ([target lines 255–259](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:255>)). But the same design states that `CommitId` exists only inside `begin` ([target lines 166–170](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:166>)); `Dispatched` already means the record is installed and IPC sent ([C.0 lines 2121–2125](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2121>)). The authoritative damage contract requires retention before dispatch ([stage-2c lines 154–157](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:154>)).

Concrete failure: an owner batch is routed before post-`begin` confirmation installs the transaction; section 3.4 directs unknown-`CommitId` consumers to ignore it, permanently losing a milestone.

Required correction: create and install the transaction inside the CommitId-aware ledger closure, before IPC/event visibility; specify that returned owner events are routed only after installation and that admission confirmation follows successful `begin`.

### Major

#### M-1 — The pool-release mutation matrix still cannot prove all three gates

The design requires `CompletionRetired`, `PriorBufferReleased`, and GPU completion ([target lines 271–273](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:271>)). Section 8.2 tests “release at `CompletionRetired` alone,” then twice tests omission of the GPU gate; it never removes only `CompletionRetired`, nor only `PriorBufferReleased` ([target line 450](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:450>)).

An implementation releasing after `PriorBufferReleased + GPU` but before `CompletionRetired`, or after `CompletionRetired + GPU` without `PriorBufferReleased`, can evade the listed mutations while violating the ownership boundary ([stage-2c lines 113–117](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:113>)).

Required correction: provide three independent withheld-evidence cases and mutations, each dropping exactly one gate.

### Minor

None.

## Coverage and implementation checks

- Incorporation: all five prior findings audited.
- Architecture/contracts: composed/direct Present ownership, owner validation, event routing, damage creation, and non-deferral assessed.
- Safety/ownership: exact-once terminalization, transaction visibility, and pool-release gates assessed.
- Compliance/evidence: stage-2c §§3–5 and C.0 §§10.2, 10.4, and 12 checked.
- Excerpts used: **12/12**.
- Unassessed—not deemed sound: the complete 2c-ii successor-replacement sequence and debt-design §§4.4/9.5 were not excerpted before the budget was exhausted. The specific unresolved question is whether the conductor already transfers the complete `CompletedPresentEvent`/wake payload into accepted direct state, or retains only its generation.
- Deferred to implementation: Rust/API details, compilation, formatting, clippy, portability, deterministic tests, and hardware execution.