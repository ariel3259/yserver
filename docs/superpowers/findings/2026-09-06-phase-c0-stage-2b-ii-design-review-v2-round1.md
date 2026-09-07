## Verdict

1 blocking, 3 major, 0 minor

Coverage: INCOMPLETE

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `13637318`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.
This is the first v2 review of this plan; its counts are not comparable to v1.

**Recorded usage:** 64,722 tokens reported by the completed process (exit 0),
excluding the author session. No build/test or nested reviewer was run.
Raw log: `/tmp/yserver-stage2bii-design-review-v2-round1.log`.

**Author verification:** B-1 verified as a missing explicit revocation rule for
evidence staged before last-consumer cancellation (plan lines 171–173 versus
spec lines 1795–1801 and 3085–3089); this is a plan-contract gap, not a claim
about already implemented behavior. M-1's example needs a qualification:
baseline `DeviceCommitOwner::send_on` treats `SendError::Ipc` as uncertainty,
not proven non-dispatch. Its proposed reset applies only to proven pre-IPC
refusals/cancellations (for example BoundaryViolation), never blindly to a
failed socket write. M-2/M-3 remain reviewer findings for disposition; no plan
corrections or additional review were performed in this round.

This is a design-review result only; it does not establish compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Audit |
|---|---|---|
| B-1 — overbroad qualification claim | APPLIED | `CompletionQualification` is now explicitly completion-mechanism-only and cannot publish structural, incarnation, or readiness bits ([plan:372](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:372>)). |
| B-2 — forbidden ioctl alias retained | APPLIED | Task 5 removes `IoctlReq`, centralizes the inferred cast, converts the actual consumers, and assigns all three portability gates ([plan:282](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:282>)). |
| B-3 — purpose-tagged tokens violate monotonic namespace | APPLIED | Task 3 specifies one raw counter starting at one, removes purpose bits/incarnation seeding, and resolves type through owner records ([plan:143](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:143>)). |
| M-1 — no production `atomic_enabled` source | APPLIED | The actual Atomic `SET_CLIENT_CAP` outcome is stored in `Device`; discovery is receiver-based and uses separate immutable discovery/mutable installation passes ([plan:400](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:400>)). |
| M-2 — unregistering an immediately successful fd | APPLIED | Unregister is conditional on `registered`; tests distinguish immediate success from Pending→Success ([plan:334](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:334>)). |
| M-3 — ambiguous QUEUE reply byte order | APPLIED | Exact offsets and unequal-value fixed-byte goldens are prescribed ([plan:199](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:199>)). |
| M-4 — legacy proof/refusal boundary unspecified | TRADED | Proof ownership, visibility, issuer, refusal variants, and EOF distinction are now concrete, but the new return contract cannot enforce its required event-before-proof ordering; see M-3. |
| M-5 — shutdown misses consumer cancellation | APPLIED | The shutdown path is explicitly named and exercised with shared consumers and an unresolved QUEUE reply ([plan:592](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:592>)). |

## Findings

### Blocking

#### B-1 — Cancelling the last consumer does not revoke already-staged QUEUE evidence

An event may arrive before `QueueAccepted`; the plan retains it and publishes nothing “until explicit success” ([plan:171](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:171>)). Last-consumer cancellation removes logical storage but deliberately retains already-observed evidence until exchange resolution ([plan:173](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:173>)). No rule prevents the later success reply from validating and publishing that retained sample.

Concrete sequence: QUEUE is sent; its kernel event is staged; all consumers are cancelled; `QueueAccepted` then arrives. Following the stated staging rule can advance the clock from a consumerless arm. The spec requires a no-consumer arm to become logically cancelled/tombstoned and its event to remain telemetry-only ([spec:1795](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1795>)); consumerless arms cannot advance either clock ([spec:3085](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3085>)).

Required correction: cancellation must irreversibly mark retained correlation/evidence non-publishable. A later reply may validate and dispose it, release the exchange lease, and tombstone identity, but must not update clocks or wake Present. Add the event→cancel-last-consumer→success permutation.

### Major

#### M-1 — Qualification has no transition for a candidate abandoned before IPC

`begin_install_restore` records `Awaiting` before the split `send_on` step ([plan:238](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:238>), [plan:396](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:396>)). Qualification transitions cover completion, rejection, and invalidation, but not a known-before-IPC send failure or abandonment.

A failed control-socket send can terminalize the commit as never submitted while leaving `Awaiting(commit)` installed. Because a new candidate requires “no … current candidate,” subsequent real installs cannot qualify. The spec distinguishes cancellation before dispatch from uncertain post-dispatch outcomes ([spec:612](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:612>)).

Required correction: specify that every proven-pre-IPC terminal path atomically returns qualification to `Unqualified` for that generation, without poisoning, and test send failure followed by a successful candidate.

#### M-2 — Checked deadline overflow has no fail-closed state transition

The plan requires `Instant::checked_add` and forbids saturation ([plan:340](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:340>)), but deadlines are created after acceptance/HardwareComplete through APIs returning only events, and `MechanismFailure` has no deadline-construction failure ([plan:259](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:259>)).

If either hardware or per-CRTC Present deadline cannot be represented, the plan provides no legal transition: panic, absent timer, or saturation all violate its contract. An absent timer can retain an accepted record indefinitely despite the spec requiring missing evidence to reach `CompletionUnknown` at its deadline ([spec:1858](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1858>)).

Required correction: define checked-add failure as an immediate typed fail-closed mechanism failure retaining the record, and test both hardware- and Present-deadline construction boundaries.

#### M-3 — Legacy handover API cannot guarantee drain-event delivery before proof consumption

The plan requires returned drain events to be delivered before consuming `LegacyDrained` ([plan:426](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:426>)). But `try_finish_legacy_transport` returns those events to its caller, while the private issuer returns proof and events to that same method ([plan:414](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-06-phase-c0-stage-2b-ii-completion-evidence.md:414>)). With no callback or second-phase API, the method must either consume proof before its returned events can be delivered or return without finishing.

That creates an ordering hole at the exclusive-reader boundary required by the spec ([spec:1712](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1712>)).

Required correction: make handover explicitly two-phase, or let the backend synchronously apply the drained events before passing the proof to the owner. Test the observable ordering, not merely proof refusal conditions.

### Minor

None.

## Coverage and implementation checks

- Incorporation: all eight supplied round-2 findings audited.
- Architecture/contracts: QUEUE correlation, qualification ownership, cancellation, completion stream, legacy handover, and event-loop ordering assessed.
- Safety/failure semantics: staged evidence, fd lifecycle, deadlines, pre-IPC failure, and proof consumption assessed.
- Spec/verification: relevant identity, event, clock, fence, qualification, and cancellation requirements checked.
- Excerpts used: 12/12, all from the authoritative spec. No new source excerpts were available after the bounded spec coverage; baseline source facts established by the supplied prior were not independently re-audited.
- Coverage is incomplete because combined-output truncation prevented full assessment of portions of the spec’s detailed fence-poll/recovery and deadline text, notably lines 1901–2032 and 2153 onward. Those areas are unassessed, not sound.
- Real implementation must still run formatting, clippy, workspace tests, protocol goldens, three-target checks, and compiler-driven API/fixture reconciliation.

## Author supplementary verification — 2026-09-07

Completed the requested local follow-up, not another adversarial round. Read
the previously truncated spec sections 1901–2032 and 2153–2240 in full and
contrasted the affected plan contracts and tests with them. Checked baseline
`crates/yserver/src/kms/owner/device.rs:155–225` and its pre-IPC refusal test
at line 724. No builds, tests, compiler simulation, or external reviewer were
run. References below use the unchanged revision-3 plan and authoritative spec
linked above; these are design findings, not reproduced implementation bugs.

### Disposition of the four findings

- **B-1 confirmed.** Plan 169–173 retains staged evidence after last-consumer
  cancellation but does not revoke its eligibility for publication on a later
  successful reply. Spec 1795–1801 and 3085–3089 forbid consumerless arms from
  advancing clocks. Require an irreversible non-publishable state and the
  event → cancel last consumer → QueueAccepted test. Retaining correlation to
  reconcile the host call is necessary; retaining publication authority is not.
- **M-1 confirmed with corrected example.** Plan 396–398 installs Awaiting and
  describes completion/rejection/invalidation, but not retirement after proven
  non-dispatch. Baseline send_on terminalizes and retires on Reaped, Stalled,
  AlreadyInFlight, ReservationMismatch or BoundaryViolation. The planned
  candidate therefore needs a matching commit/generation reset on that path;
  no automatic retry is implied. The review's failed-control-socket example is
  incorrect: SendError::Ipc marks Dispatched and leaves reconciliation to the
  queued terminal event. It must not reset qualification as harmless refusal
  (spec 612–617). Test a proven pre-IPC refusal followed by a newly admitted
  valid candidate, and separately preserve uncertainty for an attempted write.
- **M-2 confirmed, restricted to post-acceptance deadline construction.** Plan
  340–368 requires checked arithmetic; Task 6 step 3 precomputes durations
  before dispatch. Neither the event-only acceptance/fence APIs nor the
  MechanismFailure enum at 259 specifies what happens if adding a duration to
  the observation Instant fails. Define a typed immediate unknown/poison
  transition retaining the record for hardware and missing-Present timer
  construction; never silently omit the timer. This is a proposed fail-closed
  completion of the contract, not a spec-mandated enum name. Crucially, spec
  10.3 distinguishes unavailable/unrepresentable lifecycle measurements or
  an observed maximum above 28 seconds: those leave the cohort unvalidated
  before dispatch, not poisoned under the fast timer. Preserve that admission
  refusal and test the two post-acceptance construction boundaries separately.
- **M-3 confirmed as an API/order contradiction.** Plan 414–427 returns drain
  events from try_finish_legacy_transport while requiring their delivery before
  consuming its internally issued proof. No second phase or synchronous event
  application is specified. Task 7 also requires delivery to backend consumers,
  not platform logging/discard. Define either internal synchronous application
  before proof consumption or an explicit two-phase handover, preserving
  stopped admission and exclusive draining throughout. Test observable delivery
  before permit removal, not just proof identity/refusal. Spec 1712–1723 supplies
  the exclusive-reader requirement; the stronger delivery order is the plan's
  own contract. No production handover is claimed for this stage.

### Previously unassessed sections

- **Fence status and failures (spec 1901–1968):** plan 263–269 and 334–336,
  plus Task 5, preserve canonical success queries, Pending registration,
  complete multi-CRTC success, independent Presented evidence, and quarantine
  on error. Closing a successful fence descriptor does not release the
  accepted resource ledger. Unknown cannot transfer that ledger through
  CompletionRetired. No additional contract gap found in these checks.
- **Failure scope and recovery boundary (spec 1969–2032):** the plan's monotonic
  incarnation latch and retained unknown ledger cover the local owner failure
  boundary. Recovery, fd-family retirement, real resource teardown and full
  protocol terminalization are explicitly deferred by plan scope to later
  stages; this follow-up does not certify those unimplemented requirements or
  treat an out-fence/raw-fd close as universal teardown proof.
- **Deadlines and terminalization boundary (spec 2153–2240):** the four timer
  origins, constants, per-CRTC Present timers and lifecycle measurement guard
  match plan 340–368 and Task 6. M-2 remains the missing post-acceptance error
  transition. Section 10.4's full Present/idle/release ledger is explicitly
  deferred to producer/resource integration, not supplied by these timers.

**Follow-up status:** the named missing-section checks and disposition of all
four findings are complete. All four require plan corrections; none is fixed
by this report. The original review's INCOMPLETE verdict, counts and provenance
remain unchanged: this author supplement does not retroactively extend the
external review's coverage or certify the whole plan. No implementation or
plan correction was performed.

## Subsequent author fold-back — 2026-09-07

On user authorization, plan revision 4 applies B-1 and M-1/M-2/M-3 with the
qualifications above. It adds irreversible sequence publication revocation,
matching pre-IPC candidate cleanup, typed post-acceptance DeadlineOverflow,
and synchronous legacy event application before proof consumption. Tasks 3,
6 and 7 name the corresponding regression scenarios. These are plan edits,
not implemented behavior or passing tests. Historical findings, line references,
coverage and reviewer counts above describe revision 3 and are preserved;
this fold-back does not claim a new external verdict.
