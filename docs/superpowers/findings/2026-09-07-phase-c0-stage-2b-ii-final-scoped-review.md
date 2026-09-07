## Verdict

1 blocking, 0 major, 0 minor

Coverage: COMPLETE FOR DECLARED SCOPE

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `13637318`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.
This pass has a narrower declared scope than the prior v2 round; counts alone
are not a whole-plan quality comparison.

**Recorded usage:** 36,780 tokens reported by the completed process (exit 0),
excluding the author session. Log:
`/tmp/yserver-stage2bii-final-scoped-review-authorized.log`.
The initial sandbox launch failed before reviewer initialization; its log is
`/tmp/yserver-stage2bii-final-scoped-review.log`. No verdict came from that launch.

**Author verification:** confirmed B-1 against revision 4's handover contract
at plan lines 457–483. A consuming Vec plus unit-only error result specifies
neither retained suffix ownership nor a latch preventing a later proof after
partial application failure. The failure sequence is therefore permitted by
the plan, not a reproduced production bug. A fix must preserve exact-once
effects as well as events: a failed event cannot be retried blindly if it has
already caused side effects. No plan correction or further pass was performed
as part of this review.

This verdict covers only revision 4’s incorporation of prior B-1 and M-1/M-2/M-3, including their direct interactions. It does not certify the whole plan, implementation, compilation, or test results.

## Subsequent author correction — revision 5

On user authorization, the plan now replaces fallible partial-batch application
with final per-event Applied/Cancelled dispositions. Internal failure latches
handover closed and requests shutdown while still disposing the entire batch;
no partially applied event is retried. The drain tuple retains valid-prefix
events even when proof issuance fails. Task 7 specifies three-event and
malformed-tail regressions, preserving resource and normal Present semantics.
This addresses B-1 at the design-contract level; no implementation tests or new
independent review have occurred. The verdict below/above remains the historical
revision-4 verdict and its line references refer to that revision.

## Incorporation audit

| Prior finding | Status | Audit |
|---|---|---|
| B-1 — staged QUEUE evidence survives last-consumer cancellation | APPLIED | The retained exchange now has an irreversible `publishable` latch. Last-consumer cancellation clears it before logical removal; reply and event paths must consult it, and a fresh arm cannot restore authority ([plan:175–184](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:175)). Task 3 covers event → cancellation → acceptance, fresh-token isolation, legacy output, and contradiction handling ([plan:647](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:647)). This satisfies consumerless-arm cancellation requirements ([spec:1795–1801](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1795), [spec:3085–3089](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3085)). |
| M-1 — qualification remains Awaiting after proven pre-IPC retirement | APPLIED | Matching commit and topology generation are reset atomically only on proven pre-IPC terminalization. Attempted-write `SendError::Ipc` explicitly remains dispatched/uncertain ([plan:430–440](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:430)). Task 6 tests refusal, cancellation, stale identity, construction failure, recovery through a newly admitted candidate, and attempted-write separation ([plan:707](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:707)). This matches the baseline `send_on` uncertainty boundary ([device.rs:155–225](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:155)) and spec COMMIT-1 ([spec:612–617](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:612)). |
| M-2 — post-acceptance deadline overflow lacks a fail-closed transition | APPLIED | Observation-time checked-add failure now enters typed `DeadlineOverflow`, poisons the incarnation, closes qualification, retains the ledger, and forbids completion/retirement. Multi-CRTC Present-map construction is transactional ([plan:351–369](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:351)). Task 6 separately tests both post-acceptance timer origins and preserves non-poisoning pre-admission lifecycle refusal ([plan:708](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:708)). This correctly distinguishes lifecycle validation from post-dispatch uncertainty ([spec:2157–2210](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2157)). |
| M-3 — handover cannot apply legacy events before proof consumption | TRADED | Successful-path ordering is corrected: the backend synchronously applies events, consumes proof only afterward, and returns unit ([plan:469–475](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:469)). However, the fallible consuming application API leaves no owner for an unapplied suffix after partial failure. See B-1. |

## Findings

### Blocking

#### B-1 — Partial legacy-event application can discard the unapplied suffix and later authorize handover

`apply_legacy_drain_events` consumes the entire `Vec` and returns only `io::Result<()>` ([plan:469–471](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:469)). The plan explicitly permits application failure after an applied prefix, drops the proof, and requires that prefix not be replayed ([plan:477–480](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:477)), but defines no owner or retry contract for the remaining events.

Concrete sequence:

1. The exclusive drain removes events A, B, and C from the DRM stream and issues proof P.
2. Synchronous application delivers A, then fails on B.
3. P is dropped and the consumed vector drops B and C.
4. A retry drains to EAGAIN, sees no outstanding owner work, issues a new proof, and removes the legacy permit.
5. Handover completes although B and C were never delivered.

This defeats the correction’s event-before-proof condition and can lose clock/page-flip effects at the exclusive-reader boundary ([spec:1705–1723](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1705)). Task 7 checks permit retention and absence of prefix replay, but not preservation and exact-once delivery of the suffix ([plan:720](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:720)).

Smallest correction: make event application infallible after prior validation, or return and retain the exact unapplied suffix in backend-owned handover state. Block further draining, proof issuance, and permit removal until that suffix has been applied. Add a three-event regression where the middle event fails once, then verify A is not replayed, B/C are eventually applied exactly once, and the permit remains until completion.

### Major

None.

### Minor

None.

## Coverage and implementation checks

- Incorporation: all four requested prior findings and the author’s verification qualifications were assessed.
- Architecture/contracts: checked publication authority, qualification ownership, deadline failure routing, exclusive drain ownership, synchronous delivery, and proof consumption.
- Safety/failure semantics: checked late replies, attempted-write uncertainty, transactional timer installation, partial event application, retry, and permit removal.
- Specification/verification: checked the named cancellation, dispatch uncertainty, completion failure, deadline, and exclusive-reader requirements.
- Excerpts used: 7/12 beyond the plan and prior—six authoritative-spec excerpts and one baseline-source excerpt.
- Verified baseline ground: `send_on` distinguishes proven refusals from attempted-write `Ipc`.
- Unassessed—not claimed sound: unchanged whole-plan contracts, production conversion, recovery/reopen/fd retirement, resource retirement, full Present terminalization, and later stages.
- Deferred to implementation: Rust API reconciliation, compilation, formatting, `cargo clippy --all-targets -- -D warnings`, tests, portability builds, and runtime behavior.
