# Stage 3c design — codex review, round 3

**Target:** revision 3 (`cbd91944`), third review (prior: round 2).

**Result:** 3 blocking, 1 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (24/24). Round 2: M-1, B-1 applied; B-2, M-2 partial.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-26):** all four CONFIRMED.

- **B-1** — rev 3 §4.1 (any failed probe applies nothing) vs §3.3 (acquire continues): contradiction. Rev 4 makes acquire an explicit exception with a per-participant table.
- **B-2** — `yserver-core/src/core_loop/run.rs` `drain_requesterless_publications` queues behind an active install-capable mutation. Rev 4: urgent withdrawals bypass the FIFO; publications built from the current model; mutations validate at their turn.
- **B-3** — `run_suspend`s DRM-event drain is a fixed 1 s on the core thread. Rev 4: Owner commits dispatched first, absolute deadline, Legacy waits capped at the remaining budget.
- **M-1** — rev 3 §4.4 still said "when the transition reaches `Applied`". Rev 4 uses the episode terminal rule, with timestamps for partial outcomes.

## Review as received

## Verdict

**3 blocking, 1 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review. It does not establish that the code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Round-2 finding | Status | Revision 3 |
| --- | --- | --- |
| B-1 — unknown release leaves outputs published | **APPLIED** | Requires immediate logical withdrawal and a named failure publication ([plan:118](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:118)). |
| B-2 — failed or unanswered acquire probe has no terminal path | **PARTIAL** | Adds a deadline and withdrawal, but the acquire continuation conflicts with the episode’s all-or-nothing rule. See B-1. |
| M-1 — mixed server omits Legacy resume | **APPLIED** | Orders a scoped Legacy resume before input resumes ([plan:165](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:165)). |
| M-2 — no combined publication rule | **PARTIAL** | Adds a gate turn and participant outcome table, but §4.4 retains a conflicting single-transition publication trigger. See M-1. |

## Findings

### Blocking

**B-1 — A failed acquire probe both aborts and continues the episode.** The general rule says *any* failed probe applies and publishes nothing; the acquire rule closes the failed Owner device and continues with the others ([plan:234](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:234), [plan:175](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:175)). With two Owner devices, one successful probe and one timeout, the first rule prevents the healthy device’s reinstall; the second expects it to proceed. Input can resume while that device remains dark. Make acquire an explicit exception: define each participant’s terminal disposition, which devices resume or reinstall, and the combined publication. Test a mixed-success acquire through the production entry.

**B-2 — Urgent withdrawal can wait behind an unrelated RANDR mutation.** Removal must withdraw and publish at once, including while the VT is released ([plan:152](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:152), [plan:338](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:338)); C.0 requires immediate logical withdrawal on device loss ([C.0:2054](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2054)). Yet the current core queues requester-less publications while an install-capable mutation holds the gate ([source:1720](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1720)). If device A has a dispatched modeset and device B is removed, B can remain client-visible until A terminates. Publishing outside the gate also needs a rule preventing A’s later publication from restoring stale B state. Specify urgent withdrawal ordering and snapshot reconciliation across the gate; test removal and unknown-release withdrawal during an unrelated dispatched modeset.

**B-3 — Synchronous Legacy suspend can prevent the one-second hand-off.** A mixed release runs scoped Legacy suspend on the core while the Owner hand-off relies on a `next_wakeup` deadline ([plan:112](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:112)). The existing suspend performs synchronous work followed by a DRM-event drain allowed one second ([source:14856](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:14856)). If it occupies the core through the Owner deadline, the loop cannot service that wakeup or send `VT_RELDISP(1)` on time. C.0 forbids waiting past the VT drain deadline ([C.0:2050](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2050)). Define a mixed-server release sequence whose Legacy work cannot starve the absolute hand-off deadline, and test it with Legacy suspend consuming its bound.

### Major

**M-1 — §4.4 contradicts the combined commit-outcome rule.** Revision 3 says an episode publishes once after *every* participant is terminal, including rejected and withdrawn participants ([plan:249](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:249)). Section 4.4 still says publication occurs when “the transition” reaches `Applied` ([plan:314](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:314)). With one applied commit and one rejected commit for a physically gone connector, that trigger permits early publication or no withdrawal publication. Replace it with the episode-level terminal rule, including the partial-failure timestamp and notification decision.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** Checked all four round-2 findings against revision 3; two remain partial.
- **Architecture and safety:** Checked coordinator and per-device ownership, acquire and release ordering, gate publication, deadlines, and C.0 withdrawal and precedence rules. The gate interaction and mixed-server deadline need the contracts identified above.
- **Spec and evidence:** Scenario tests are directed through production core entries by [plan §6](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:376); classification is an adapter test. Stubbed executor replies can check ordering and disposition, but cannot prove physical `ACTIVE=0`, master hand-off, actual udev delivery, or helper reap. The planned card1 runs cover ordinary VT and connector behavior; the cross-device failure sequences in B-1 and B-2 are not named tests. Implementation inherits the umbrella’s format, clippy, test, and hardware gates ([umbrella:444](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:444)); none were run.
- **Reading limit:** 23 spec/source selections, counted as **24/24** 120-line excerpt units; one 151-line selection exceeded the per-excerpt cap. Investigation stopped. Probe-worker fd lifetime, fixture wiring, and 3d teardown were not assessed and are not judged sound. Compiler, portability, and test outcomes remain implementation checks.