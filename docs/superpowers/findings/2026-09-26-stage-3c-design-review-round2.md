# Stage 3c design — codex review, round 2

**Target:** revision 2 (`c7d9fea6`), second review (prior: round 1).

**Result:** 2 blocking, 2 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (24/24). Round 1: 4 applied, B-2 partial.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-26):** all four CONFIRMED.

- **B-1** — C.0 §10 state table: `ExecutorStalled` = logical withdrawal; the withdrawal definition removes outputs/CRTCs from the active RANDR model. Rev 3 withdraws at once and names the failure publication as an exception to the VT no-event invariant.
- **B-2** — `run_resume` exits on a failed probe (`render/backend.rs`, step 2 comment); rev 2 left the acquire episode without a deadline or disposition. Rev 3: every episode has a deadline; an Owner device whose acquire probe fails is withdrawn, not the server ended (coordinator decision, listed in §9); input resumes at acquire.
- **M-1** — rev 2 scoped the Legacy suspend but had no scoped Legacy resume. Rev 3 orders probe episode → scoped Legacy resume → Owner transitions → input.
- **M-2** — rev 2 promised one publication without a gate or partial-failure rule. Rev 3: one gate turn per episode, publication after every participant is terminal, outcome table.

## Review as received

## Verdict

**2 blocking, 2 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design-review result. It does not establish that the design compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Round-1 finding | Status | Revision 2 |
| --- | --- | --- |
| B-1 — removal deferred during VT release | **APPLIED** | Exempts removal from seat deferral, drains udev while away, and skips removed devices on acquire ([plan:139](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:139)). |
| B-2 — reinstall on a still-leased incarnation | **PARTIAL** | Closes the old incarnation and bars reinstall, but its stated `ExecutorStalled` outcome leaves outputs published and dark rather than logically withdrawing them ([plan:113](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:113)). See B-1 below. |
| B-3 — forced-reprobe timeout | **APPLIED** | Ends the gate turn, invalidates the late result, and arms a separate rebuild ([plan:211](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:211)). |
| M-1 — combined probe failure boundary | **APPLIED** | An episode gathers every device’s probe result and applies none if a probe fails ([plan:199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:199)). The later commit/publication boundary is a separate gap, M-2. |
| M-2 — udev-to-device contract | **APPLIED** | Specifies typed monitor records, device classification, and tests through that adapter ([plan:270](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:270)). Actual kernel event delivery remains deferred hardware evidence. |

## Findings

### Blocking

**B-1 — An unknown VT release retains outputs that `ExecutorStalled` must withdraw.** At the one-second bound, the plan enters `ExecutorStalled` but says the closed device’s outputs remain “published but dark” after acquire ([plan:113](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:113)). C.0 defines `ExecutorStalled` as **logical withdrawal** and defines withdrawal as removing affected outputs and CRTCs from the active RANDR/backend model ([C.0:764](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:764), [C.0:2015](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2015)). If the helper remains in an ioctl, clients can still discover an unusable output. Require immediate logical withdrawal on this outcome, and keep it withdrawn until qualified recovery. Specify its failure-event treatment alongside the umbrella’s ordinary VT no-event invariant ([umbrella:367](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:367)).

**B-2 — A failed or unanswered acquire probe has no terminal acquire path.** One failed probe makes the episode apply nothing; only a *next edge* starts another ([plan:199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:199)). Acquire needs that probe before reinstall and reopens admission and input only at `Applied` ([plan:150](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:150)). A probe error—or a worker that never answers—can therefore leave the acquired VT paused indefinitely. Legacy instead exits on resume probe failure ([source:14969](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:14969)). Define a deadline and terminal failure disposition for every episode, including acquire; state whether acquire exits as Legacy does or closes and withdraws the affected Owner devices. Test both error and no-reply paths through the acquire entry.

### Major

**M-1 — The mixed-server acquire omits Legacy resume.** Release explicitly runs scoped Legacy suspend ([plan:103](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:103)); acquire specifies only Owner reinstall after master acquisition ([plan:145](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:145)). Legacy resume performs the required full modeset relight and cursor re-arm ([source:15039](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:15039)). A mixed server can return from VT with its Legacy outputs still dark. Specify scoped Legacy resume and its ordering relative to Owner completion and input resume; add a mixed-server acquire test. This is required for the umbrella’s Legacy client parity ([umbrella:351](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:351)).

**M-2 — One probe episode has no combined commit-outcome publication rule.** The plan gathers all probes, then applies per-device commits and promises one publication, while §4.4 publishes when “the transition” reaches `Applied` ([plan:199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:199), [plan:260](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:260)). If device A applies before B rejects or stalls, the text permits A’s early publication or leaves the episode without a publication outcome. Legacy reconciles the combined snapshot before its single publication ([source:15894](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:15894)). Define who holds the episode’s RANDR gate turn, when combined state may publish, and what a partial commit failure publishes. Include mixed Legacy/Owner participants and test two devices with one late or failed *commit*, not only a failed probe.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** Checked all five round-1 findings against revision 2; B-2 remains partial.
- **Architecture and safety:** Checked VT hand-off, mixed-device paths, probe episodes, failure deadlines, and publication ownership against the cited baseline and C.0 `REC-4`/§10 contracts.
- **Spec and evidence:** Mapped all 27 named tests to production entry paths and considered what their stubs can observe. Fixtures can establish ordering and disposition; they cannot establish physical `ACTIVE=0`, kernel master hand-off, actual udev delivery, or helper reap. The named tests do not cover the acquire probe failure or partial commit outcome above.
- **Reading limit:** 22 selections, counted as **24/24 excerpt units** because two reads exceeded 120 lines and were counted as two each; investigation stopped there. Worker-fd behavior, fixture wiring, and 3d teardown remain unassessed, not judged sound. Builds, tests, formatting, CI clippy, portability, and hardware runs belong to implementation; none were run here.