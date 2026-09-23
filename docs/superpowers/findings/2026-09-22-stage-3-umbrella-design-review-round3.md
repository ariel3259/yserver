# Stage 3 umbrella design — codex review, round 3

**Target:** `docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md`
revision 3 (`54838721`), against the C.0 specification, prior review round 2.

**Result:** 1 blocking, 2 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE
(24/24; two of them Xorg source views). Trend: r1 2B 5M (incomplete), r2 1B 2M,
r3 1B 2M. Incorporation: round-2 B-1 APPLIED; M-1 and M-2 TRADED.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-22):** all three CONFIRMED by reading revision 3
against C.0 — revision 3 let a RANDR request be `Deferred` (C.0 `REC-5`
lines 892–899 make that nonterminal) while holding every later publication
behind it (B-1); it labelled transaction terminal states (C.0 §10, lines
1955–1971) as `REC-5` event dispositions (M-1); and it answered an absorbed
request `Success` with no `lastSetTime` effect (M-2).

**Root cause, and why revision 4 rewrites instead of patching.** Round-1 B-2,
round-2 M-1/M-2 and round-3 B-1/M-1/M-2 are six findings against one rule —
how an asynchronous RANDR request is answered and published — and every patch
exposed the next layer. That is the pattern recorded before (plan Ciii r3→r4,
r8→r9). Revision 4 replaces the block with one contract that keeps Legacy's
and Xorg's synchronous shape: one RANDR mutation in flight server-wide
(publication order is dispatch order by construction); a request never waits
on a nonterminal prerequisite (answered at once as Legacy would, otherwise
bounded by the completion deadline or executor watchdog); `Success` if and
only if its own transaction is installed, with a total outcome → reply →
publication table and no absorption answered `Success`. Self-review also
corrected revision 3's classification: a client modeset is not one of the ten
`REC-4` kinds nor a `LifecycleDesired` field — it is class-1 client work that
a lifecycle event supersedes.

---

## Verdict

**1 blocking, 2 major, 0 minor**  
**Coverage: COMPLETE FOR DECLARED SCOPE**

Revision 3 fixes round 2’s failure classification and Owner `MechanismFailed` route at the umbrella-design level. Its new RANDR publication rules still permit a later device’s result to wait indefinitely, and the reply table does not fully separate commit outcomes from lifecycle-event dispositions.

## Incorporation audit

| Round-2 finding | Status | Assessment |
| --- | --- | --- |
| B-1 — rejection conflated with completion loss; Owner exits | **APPLIED** | [3a separates `FailedBeforeSubmit` from `CompletionUnknown` and replaces the Owner exit with poison entry](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:202>). The current [Legacy drain route](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20693>) has a separate exit; the design retains Legacy behavior. Recovery out of poison remains assigned to 3d. |
| M-1 — superseded parked request never resolves | **TRADED** | [The new table supplies a wake and reply](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:264>), but mixes commit outcomes with `REC-5` event dispositions and leaves an absorbed-success protocol effect unresolved. See M-1 and M-2. |
| M-2 — cross-device completion moves `lastSetTime` backward | **TRADED** | [Dispatch-order publication](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:284>) addresses reverse completion order when both requests terminate. A nonterminal predecessor can indefinitely block a later device. See B-1. |

## Findings

### Blocking

**B-1 — A nonterminal request can starve publication on every other device.** The gate holds a completed request until *every* preceding request has a terminal disposition ([plan lines 284–296](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:284>)). `REC-5` explicitly permits `Deferred` to remain nonterminal pending an external prerequisite ([spec lines 892–899](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:892>)); an executor lease can also remain stalled while other devices proceed ([spec lines 761–765](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:761>)). For example, device A’s earlier modeset is deferred across VT release and remains unable to converge after reacquire because its old executor has not reaped. Device B’s later modeset applies, but its RANDR publication waits on A indefinitely. Per-device hardware progress does not release B’s client-visible result. Define a bounded protocol rule for nonterminal predecessors that preserves truthful timestamps without holding later devices indefinitely; test this case, not only two successful commits completing in reverse order.

### Major

**M-1 — The reply table treats commit outcomes as `REC-5` dispositions.** [The table calls `FailedBeforeSubmit` and `CompletionUnknown` terminal dispositions of a request’s event](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:264>). In the parent spec they are **transaction** terminal states ([§10 lines 1955–1971](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1955>)); `REC-5` separately defines event dispositions and makes terminal ones immutable ([lines 879–899](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:879>)). A retryable explicit rejection can end one commit while the requested target remains desired. The table would immediately answer `Failed` without saying whether that event remains pending for retry or which valid `REC-5` disposition eventually closes it. It also has no same-target `AbsorbedByEvent` case, although it claims every terminal disposition resolves the request; unlike `AbsorbedByTransition`, absorption by an event alone proves no installation. Specify the commit-outcome → event-disposition → reply mapping, including retry and both absorption types.

**M-2 — An absorbed success has no defined request timestamp effect.** [The table returns `Success` for `AbsorbedByTransition` but publishes only the winner’s result](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:270>). Suppose a parked `RRSetCrtcConfig` is satisfied by a later VT-acquire installation. The winner need not carry that request’s `set_time`, so the requester can receive `Success` with no corresponding `lastSetTime` update. The current continuation applies `set_time` on a changed successful CRTC set and uses the resulting timestamp in its reply ([source lines 4934–4980](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:4934>)); [Xorg’s CRTC-set path](https://fossies.org/linux/xorg-server/randr/rrcrtc.c) likewise sets `lastSetTime` on success. Define how an absorbed successful request carries its timestamp and notification obligations through the winner, or define a compatible failure status. Include absorbed success in the core protocol evidence.

## Coverage and implementation checks

- **Incorporation:** Checked all three round-2 findings against revision 3. The current Owner and Legacy `MechanismFailed` branches are distinct; other exit sites were not exhaustively audited.
- **Architecture and safety:** Checked coordinator/driver ownership, the device slot, parked-request completion, disposition identity, the nonterminal predecessor sequence, and §6.4, §9.2, §10, and §13. Stage 5’s entry boundary and `TransportState` removal agree with the [§18 amendment](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:4056>).
- **Specification and evidence:** Checked the proposed gates against [§16’s deferred-event requirements](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3045>). The reverse-completion test does not establish progress past a nonterminal predecessor.
- **Reading and implementation boundary:** 24/24 spec/source views used: 22 bounded local excerpts and two Xorg source views. The web viewer displayed wider context than requested; no further source inspection followed. Detailed sub-stage contracts, fixture capability, hardware results, and other Owner failure routes remain unassessed. No build or test ran; formatting, regular clippy, tests, and portability gates remain implementation checks.
