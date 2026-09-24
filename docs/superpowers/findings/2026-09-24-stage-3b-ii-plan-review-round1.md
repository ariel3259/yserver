# Stage 3b-ii plan — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md`
revision 1, first review.

**Result:** 2 blocking, 1 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):** B-1 CONFIRMED (`run.rs:1603` drains only
on `CrtcConfigReady`); B-2 CONFIRMED (`KillClient`'s inline removal bypasses
`disconnect_with_pending_cleanup`, `run.rs:884`); M-1 CONFIRMED. All applied
in plan revision 2; a round 2 is needed (blocking findings).

## Verdict

**2 blocking, 1 major**  
Coverage: COMPLETE FOR DECLARED SCOPE

## Incorporation audit

| Prior findings | Status |
| --- | --- |
| None. This is the first review; check 1 was skipped as requested. | — |

## Findings

### Blocking

**B-1 — Requester-less publications have no wake contract.** [Task 4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:130) adds a backend drain “called where the loop drains ready CRTC configs.” Today that drain runs only on `Message::CrtcConfigReady` ([run.rs:1603](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1603)), and the backend sends that wake when it announces a CRTC result ([backend.rs:4417](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:4417)). If a test producer queues a requester-less publication while the gate is idle and no CRTC result arrives, the core can remain asleep and never publish it, contrary to [spec §7.5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:720) and the plan’s idle-gate case. Specify who sends a wake on publication enqueue, and require the core to drain publications on that wake even when there are no ready CRTC tokens.

**B-2 — Gate cleanup lacks a contract for inline client removal.** [Task 2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:83) requires a disconnected waiting head to leave the FIFO, but does not account for `KillClient` removing *another* client inline. That path calls `process_disconnect` without `disconnect_with_pending_cleanup`; the loop only notices that the client count fell ([run.rs:884](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:884)). Sequence: A is in flight, B is the waiting head, D kills B, then A finishes. If gate removal is wired only to the cleanup helper, B can retain the head position and delay or prevent C’s admission, violating [spec §7.1’s FIFO admission contract](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:597). Require gate cleanup or a live-client prune for every client-removal path, and cover the inline removal case.

### Major

**M-1 — The protocol-order differential omits required compound cases.** [Spec §8.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:825) requires wire-level coverage of FIFO admission, the `Q` expiry cases with changed validation and malformed input, a synchronous waiter that does not expire, and the mixed-server Owner A / Legacy B / Owner C sequence. [Task 5’s case list](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:148) names concurrent clients and `Q`, but does not require those interactions. Task 3’s `RecordingBackend` tests exercise scheduling; they cannot establish the specified Legacy-versus-Owner bytes and ordering. Add the named compound scripts to Task 5’s core-path differential, or identify an equivalent wire-level case for each.

## Coverage and implementation checks

- **Check 1:** No prior review.
- **Checks 2–3:** Examined gate ownership, ready-ring dispatch, disconnect paths, completion handoff, deadline wake, and requester-less delivery. The concrete risks are B-1 and B-2.
- **Check 4:** Compared the plan’s named tests with normative spec §§7 and 8.2; M-1 remains a verification gap.
- **Reading:** 24/24 bounded spec/source excerpts. Prerequisite 3b-i interfaces were treated as given. Exact opcode implementation, Owner fixture behavior, API typing, builds, tests, formatting, Clippy, and portability remain for implementation and its real gates; this review makes no claim that they pass.