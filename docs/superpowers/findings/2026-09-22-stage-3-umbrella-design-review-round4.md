# Stage 3 umbrella design — codex review, round 4

**Target:** `docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md`
revision 4 (`e582131d`), against the C.0 specification, prior review round 3.

**Result:** 1 blocking, 3 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE
(24/24). Trend: r1 2B 5M (incomplete), r2 1B 2M, r3 1B 2M, r4 1B 3M.
Incorporation: round-3 M-1 APPLIED; B-1 and M-2 TRADED.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-23):**

- **B-1 — CONFIRMED.** `apply_crtc_config` returns the no-op for an already
  installed configuration (`backend.rs:24103`), and the continuation answers
  `Success` with no rebuild or notification (`process_request.rs:4963`);
  revision 4's "`Success` iff installed" excluded it.
- **M-1 — CONFIRMED.** The PRIME qualification probe has its own 30 s process
  watchdog (`probe_executor.rs:46`) and precedes commit dispatch, where the
  C.0 §10.3 clocks start.
- **M-2 — CONFIRMED.** Legacy requests complete synchronously
  (`backend.rs:23686`) and the core parks per client (`core_loop/run.rs:558`);
  nothing named would order an Owner request against a Legacy one.
- **M-3 — CONFIRMED, and it corrects the author's own claim.** Xorg does not
  enforce monotonicity either: `ProcRRSetCrtcConfig` converts the client time
  (`randr/rrcrtc.c:1389`) and stores it (`:1481`) with no `InvalidTime`
  check, and yserver mirrors that (`process_request.rs:4543`). "`lastSetTime`
  never moves backward" was an overclaim; the obligation is parity.

**Disposition — scope, not another patch.** Rounds 1–4 produced ten findings
against one subject, the Owner's asynchronous RANDR protocol, each round one
layer deeper. The umbrella's own out-of-scope clause gives detailed sub-stage
design to the sub-stage specs, and revisions 2–4 had pulled 3b's design into
the umbrella. Revision 5 removes that design and states the six obligations
the 3b spec must meet, each traced to the confirmed findings above and in
rounds 1–3; they become the 3b spec's acceptance criteria and its
protocol-order gate cases. The remaining umbrella contracts (driver, failure
edges, projections, result boundary, differential gate, §18 amendment) were
accepted as APPLIED by rounds 2 and 3 and are unchanged.

---

## Verdict

**1 blocking, 3 major, 0 minor**  
**Coverage: COMPLETE FOR DECLARED SCOPE**

Revision 4 removes the prior review’s indefinite `Deferred` predecessor. Its rewritten RANDR contract still has a valid request with no success outcome, an unaccounted pre-commit wait, and gaps in the server-wide ordering claim. This is a design review; it does not establish that implementation builds or tests pass.

## Incorporation audit

| Round-3 finding | Status | Assessment |
| --- | --- | --- |
| B-1 — nonterminal predecessor blocks later publication | **TRADED** | [Plan §3b item 2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:279) removes `Deferred` for client requests. The claimed completion bound omits pre-commit qualification; see M-1. |
| M-1 — commit outcomes confused with `REC-5` dispositions | **APPLIED** | [Item 3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:289) classifies client modesets outside `REC-5` and maps transaction outcomes to replies. |
| M-2 — absorbed success lacks the request’s timestamp effect | **TRADED** | [Item 3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:305) answers absorption `Failed`, resolving that case. Its “success iff installed” rule excludes a valid idempotent request; see B-1. |

## Findings

### Blocking

**B-1 — A valid no-op CRTC request has no compatible success outcome.** [Plan §3b item 3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:289) allows `Success` only when the request’s transaction installs state. But a client may request the configuration already installed: the existing backend returns `Ok(false)` without touching hardware ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24103)), and the protocol replies `Success` without changing `lastSetTime` or sending change notifications ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:4963)). The Owner rule must either fail that valid request or submit an unnecessary transaction with different protocol effects, contradicting the plan’s Legacy parity gate. Add an explicit idempotent-success outcome with the existing timestamp and notification behavior, including when hardware cannot start.

### Major

**M-1 — Pre-commit qualification is outside the stated RANDR wait bound.** [Plan §3b](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:242) retains the `begin_crtc_config → Pending` path, but [item 2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:279) bounds an in-flight request only by the commit deadline or executor watchdog. A PRIME qualification probe precedes commit submission ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:4269)) and has its own 30-second watchdog ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/probe_executor.rs:35)); C.0’s commit clocks start at IPC dispatch or accepted ioctl ([spec §10.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2202)). A probe taking 29 seconds followed by a commit taking 29 seconds holds every later RANDR request for about 58 seconds while neither commit clock expires. Define the protocol wait bound across qualification and commit, its timeout wake, and evidence for that sequence.

**M-2 — The mixed Legacy/Owner gate has no assigned owner.** The plan promises a server-wide gate while retaining production Legacy writers ([plan §§2.5, 3b](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:151), [item 1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:271)). The lifecycle driver is per device, and today’s core parking table blocks only each originating client ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:558)); Legacy disables and probe-less changes return synchronously ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:23686)). If Owner device A is pending when another client sends a Legacy request for B, B can publish first unless a shared protocol gate intercepts that Legacy path. Specify which component gates **both** transports, releases queued entries on terminalization or disconnect, and leaves C.0’s DRM commit ownership per device ([spec §13](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2521)). Add a mixed-transport ordering case to the layer-1 evidence.

**M-3 — Dispatch order alone cannot keep `lastSetTime` monotonic.** [Plan §3b item 1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:271) claims that serial publication prevents `lastSetTime` moving backward. The request supplies `set_time` ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:4657)), and successful changed requests write it directly ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:4943)). The inspected request path does not compare that timestamp with the current `lastSetTime` ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:4543)). Thus successive successful requests carrying times 100 and 90 publish in order yet leave time at 90. State the timestamp validation or publication rule, including how a parked request is checked after its predecessor, and verify its response against the protocol oracle. The precise Xorg response was not verified in this pass.

## Coverage and implementation checks

The incorporation audit covered all three round-3 findings. Architecture and safety checks covered gate ownership, mixed transports, per-device commit ownership, request termination, and no-op behavior. Spec and evidence checks covered C.0 `REC-1..6`, §9.2, §10/10.3, §13, §16, and the §18 amendment; the proposed tests do not yet establish the three ordering and timing cases above.

**Excerpts used: 24/24** bounded spec/source excerpts. Other RANDR mutation entry points, exact Xorg timestamp responses, detailed sub-stage designs, and hardware behavior remain unassessed; no conclusion that those areas are sound follows. No build, test, benchmark, or compiler check ran. Formatting, regular all-targets clippy, tests, and portability remain implementation gates.
