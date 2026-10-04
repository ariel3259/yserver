# Stage 3d spec — design review round 1

**Result:** 0 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`; model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.

**Coordinator verification:** M-1 verified (admission refuses a modeset whenever the lifecycle state is not
`Ready`, admission.rs ~1143; C.0 §10 terminalizes affected protocol work). Design: the invisibility contract
was too broad. The user chose to park RANDR mutations in the RandrMutationGate during recovery (decision 2a).
The unassessed late-result path is now stated explicitly from C.0 `REC-4`. Revision 2.

## Verdict

**0 blocking, 1 major, 0 minor**

**Coverage: INCOMPLETE**

The design preserves the principal recovery and shutdown barriers. One verification contract needs correction; stale-result ownership was not fully traced within the reading budget. This result does not approve implementation or establish that checks pass.

## Incorporation audit

| Prior review | Result |
| --- | --- |
| None | First review; incorporation audit skipped. |

## Findings

### Blocking

None demonstrated.

### Major

**M-1 — The invisibility gate conflates stable topology with unchanged request outcomes**

The [plan’s byte-equality gate, lines 186–187](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md:186), is broader than decision 2’s promise that outputs remain published during successful recovery.

Concrete sequence: a normal frame loses completion, admission closes, and recovery starts. While the screen is frozen, a client requests a valid modeset. The existing backend refuses modesets whenever lifecycle state is not `Ready` ([admission.rs:1143–1150](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1143)). The fault-free counterpart can accept that request. Recovery subsequently succeeds without changing topology, but the connections have already received different request outcomes.

Those differences cannot simply be suppressed: the authoritative umbrella requires failed or acceptance-unknown transitions to answer `Failed`, and requires named exceptions for other client-visible differences ([umbrella:360–365](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:360)). C.0 also requires completion-loss handling to terminalize affected protocol work ([C.0:1702](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1702)).

**Smallest correction:** scope fault-free equality to stable RANDR topology, read-only topology results and absence of recovery-only topology notifications. Define and separately test request refusal and affected Present terminalization during recovery. Include a client request during the closed-admission interval so a narrowly scripted equality test cannot overstate invisibility.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation:** skipped because there is no prior review.

2. **Architecture and cross-task contracts:** checked the pure-arbiter/effectful-driver boundary against [umbrella:101–129](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:101), REC precedence/fate, fresh-incarnation setup, and acquire reuse. Existing acquire code combines device preparation with global seat, input and publication handling. Reuse must therefore preserve the plan’s current recovery transition rather than invoke the whole VT-acquire route. The design’s device-scoped reinstall and current-transition requirements provide that boundary; exact extraction belongs to implementation.

3. **Safety, ownership and failure semantics:** checked barrier-before-reopen, same-incident stalled resume, supersession, DPMS pause/resume, failure without recursion, withdrawal, removal and shutdown ordering against C.0 §6.4/§10. Verified that executor lease retirement and complete fd-family closure are separate obligations: the cleanup registry additionally checks control closure and non-payload aliases, then discharges payload aliases ([drm_cleanup.rs:563–631](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:563)). The proposed production-holder inventory is appropriate, but its completeness and the complete device-loss destruction graph remain implementation obligations.

   Shutdown’s stated seat-before-wait and reap-before-close order matches C.0. Executor destruction already avoids blocking on an unreaped child; this alone does not establish supervisor retention or deadline behavior.

   **Unassessed:** the complete accepted-stale/rejected-stale result path, particularly returned-fd adoption and quarantine routing after supersession. No soundness conclusion is made for that path.

4. **Specification and verification:** checked the umbrella’s client differential, exhaustive arbiter requirements and inherited implementation gates. Criteria A/B/C/F are retained. Hardware cycles and mutations remain required evidence, not evidence obtained by this review. Formatting, regular clippy with `--all-targets -- -D warnings`, required feature configurations, real compiler/tests, and Linux glibc/musl/FreeBSD buildability remain implementation checks. No builds, tests or hardware runs were performed.

**Reading budget:** **24/24 excerpt equivalents**—23 source/spec reads, conservatively charging one accidental 137-line read as two. That read exceeded the 120-line cap; the initial locator also exceeded its 40-line output cap. Investigation stopped.

The specific unresolved follow-up is: **after recovery is superseded, which owner adopts every fd returned by a late successful executor result, and which winning quarantine receives both possible resource sets?** Trace that reply-to-driver handoff before claiming complete coverage.