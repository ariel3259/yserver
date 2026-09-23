# Stage 3a-i plan — codex review, round 2

**Target:** revision 2 (`b43350ca`). **Result:** 4 blocking, 1 major, 0 minor;
coverage COMPLETE FOR DECLARED SCOPE (19/24). Trend: r1 2B 4M 1m, r2 4B 1M.
Incorporation: M-1, M-3, m-1 APPLIED; B-2, M-2 PARTIAL; B-1, M-4 TRADED.

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument
`docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):** all five CONFIRMED against C.0 — B-1
(line 798: prompt logical progress while a host call is outstanding; §10 VT row
line 2050: release the seat immediately); B-2 (§10's table, lines 2044–2055,
gives the encountering transition's row: only normal live operation creates an
incident); B-3 (`REC-5` line 827: every event gets its id before arbitration);
B-4 (line 928 and §10 DPMS row line 2051: DPMS-on resumes the paused attempt);
M-1 (item 63: during **each** active transition).

**Disposition — rewritten, not patched.** Revision 2 implemented `REC-4/5/6`
but not C.0 §10's per-row `CompletionUnknown` table, and its R4-2a did not
distinguish logical from physical obligations. Revision 3 rewrites Task 3
around both normative tables and R4-2a as "logical now, physical after the
receipts".

---

## Verdict

**4 blocking, 1 major, 0 minor.**  
Coverage: **COMPLETE FOR DECLARED SCOPE**.

This is a design review. It does not establish that the code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Round 1 finding | Status | Revision 2 assessment |
| --- | --- | --- |
| B-1: winner advances before safety actions complete | **TRADED** | [R4-2a](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:207) adds acknowledgments and a delayed/failed-ack test, but its pending rule can block prompt logical teardown (B-1 below). |
| B-2: first completion loss has no incident creation path | **PARTIAL** | [R6-0](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:160) adds a `RecoveryId` allocator and creation test. It leaves the representative event ID without a producer and applies an overbroad loss rule to active transitions (B-2, B-3). |
| M-1: matrix delivered after its consumer | **APPLIED** | The [matrix is Task 3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:154); the arbiter is Task 4. |
| M-2: no mixed-arrival ledger evidence | **PARTIAL** | The [new test](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:255) checks exact results, but only during active `VTRelease` (M-1 below). |
| M-3: ordinary-work tag unproven | **APPLIED** | Optional transition ID and a [delayed ordinary-reply test](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:258) are specified. |
| M-4: poisoned DPMS falsely counted `Applied` | **TRADED** | [Deferred(ReadinessClosed)](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:239) prevents false completion, but the incident’s DPMS-on resume is unresolved (B-4). |
| m-1: replaced deferred representative endpoint | **APPLIED** | [R5-3 and its test](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:134) require `SupersededBy(newer)`. |

## Findings

### Blocking

**B-1 — Pending acknowledgments can block required logical teardown.** [R4-2a](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:207) keeps the winner pending for a delayed acknowledgment and poisons it on a failed one. Suppose `VTRelease` supersedes a DPMS commit whose host call remains outstanding, then quarantine-transfer acknowledgment fails or never arrives. The rule gives `VTRelease` no way to finish its prompt logical and seat obligations. C.0 requires progress without waiting for that host call and immediate seat release; removal and shutdown likewise have prompt logical obligations ([§6.4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:798), [§10](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2050)). Specify which logical winner actions proceed despite failed or missing receipts, while physical actions remain fenced. Require receipts to be independent of the loser’s terminal wait and correlated to the current winner, including when another event supersedes the pending winner; test those sequences.

**B-2 — Generic completion-loss handling contradicts lifecycle-specific fate.** [R6-0](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:160) includes any §10 `CompletionUnknown` trigger, while [R4-6](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:232) makes completion loss `Poisoned` and terminalizes the transition. If a commit being drained for `VTRelease` expires with no prior incident, these rules can create a fresh incident and end `VTRelease` in `Poisoned`. C.0 instead requires seat release and invalidation of any existing incident; a fresh attempt is authorized only after a later acquire ([REC-6](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:925), [§10 VT row](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2050)). Separate first loss during normal live operation from loss encountered by an active lifecycle row, and test the row-specific outcome for VT release, removal, and shutdown.

**B-3 — First incident has no specified representative event ID.** [Task 1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:82) reserves `LifecycleEventId` allocation to the coordinator. [Task 3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:160) creates an incident from an acknowledged completion loss, then requires later losses and recovery events to be `AbsorbedByEvent` of its representative, without specifying how that result obtains an event ID. A first ordinary-work loss can therefore create a `RecoveryId` with no event representative for the next loss to reference. C.0 requires event IDs before arbitration and duplicate recovery events to reference the incident representative ([REC-5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:827), [duplicate rule](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:872)). Specify the coordinator-to-incident delivery of that first ID; assert exact event-ID dispositions in the first-loss test.

**B-4 — Poisoned DPMS-on has no route to resume a paused incident.** [R6-1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:170) resumes an incident after a DPMS-on transition. [R4-7](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:239) makes poisoned DPMS logical-only and leaves its projection deferred until recovery installs. After DPMS-off pauses an existing incident, DPMS-on can thus wait for an installation that itself waits for the incident to resume. C.0 expressly requires DPMS-on to resume that same paused attempt without creating an ID or reviving `RecoveryFailed` ([REC-6](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:928), [§10 DPMS row](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2051)). Define logical DPMS-on as the resume boundary for the incident while its projection remains `Deferred`; test ID, budget, disposition, and absence of a KMS action together.

### Major

**M-1 — The mixed-arrival test still cannot establish item 63 across active kinds.** The [new ledger test](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:255) permutes lower arrivals only under `VTRelease`; pairwise elections and a third-kind transition-count test do not check exact retained fields and successive winners for other active kinds. C.0 item 63 requires lower-priority permutations *during each active transition* ([§16.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3045)). Extend the generated test across active kinds, asserting the ledger and convergence order for valid mixed lower arrivals.

### Minor

None.

## Coverage and implementation checks

I read the plan and prior review once and used **19/24 bounded excerpts** beyond them: 16 from the relevant C.0 and stage-3 spec sections, and three from the existing owner-event and admission routes. The source excerpts confirm terminal outcomes pass through the current event route and `Admitted::Topology` is currently unsupported; they do not establish the future driver’s behavior.

The incorporation, ownership and event-delivery, safety and failure, and specification and evidence checks are complete for this pure-layer scope. The plan assigns nightly formatting, CI-form clippy, tests, compile-fail checks, and compiled mutation checks to implementation. No ioctl or ABI change is proposed here, so a portability build claim was not assessed. I ran none of those gates. Driver execution in 3a-ii, hardware behavior, and later-stage transitions remain unassessed, not presumed sound.
