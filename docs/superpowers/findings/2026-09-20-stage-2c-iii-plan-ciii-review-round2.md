## Verdict

**1 blocking, 1 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only. It does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| B-1 — unflip request cannot reach admission | APPLIED | Capacity movement is assigned to dispatch; retry is assigned to the `maybe_composite` fork on later requested ticks; every production cause continues through `request_direct_unflip`, which becomes the Owner request entry ([plan lines 85–113](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:85), [284–307](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:284)). |
| B-2 — copied-route fence/conductor mismatch | APPLIED | The unsound copied route was removed and its required producer restructuring is explicitly assigned to a follow-on plan; Owner entry remains forbidden for affected devices ([plan lines 177–186](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:177)). |
| M-1 — copied evidence precedes reachability | APPLIED | The copied-route exclusivity case and fixture left this plan with the route itself ([plan lines 417–429](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:417)). |
| M-2 — no executable §6.4 hardware test | TRADED | Task 7 now owns and names the hardware test, but its name is unintentionally selected by the routine `--include-ignored` gate, violating the test’s no-run boundary. See B-1. |
| M-3 — portability gates absent | APPLIED | All three required target checks are assigned to the last task and coordinator ([plan lines 241–247](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:241), [519–521](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:519)). |
| M-4 — cursor mutation had preservation backwards | APPLIED | The transaction must omit cursor and gamma properties; T22/T23 add them, matching the existing primary-only atomic request ([plan lines 393–409](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:393)). |

## Findings

### Blocking

#### B-1 — The routine ignored-test gate will execute the tty-only DRM test

The plan’s universal gate runs:

`cargo test -p yserver --lib c0_conv_ciii_ -- --include-ignored`

in debug and release ([plan lines 218–239](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:218)). Task 7 names its ignored hardware test `c0_conv_ciii_owner_route_on_card1_drm` ([plan lines 477–488](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:477)). The substring filter matches that name, and `--include-ignored` makes it runnable. The assertion at plan line 238 that this filter “never selects” the `_drm` test is therefore false.

Concrete sequence: Task 7 creates the test; the implementer performs the mandatory software checks; the broad Ciii filter selects the `_drm` test and attempts to take DRM master outside the coordinator-controlled tty2 run and without the required user approval. This violates both the plan’s hard rule and the spec’s hardware-run boundary ([spec lines 511–514](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:511), [645–653](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:645)).

Smallest correction: add an exact `--skip c0_conv_ciii_owner_route_on_card1_drm` to both routine Ciii `--include-ignored` commands, or name the hardware test outside the routine Ciii prefix. Keep its exact tty2 invocation only in the coordinator block.

### Major

#### M-1 — Route-exclusivity mutations cannot produce the observation the plan requires

Task 5 says each mutation merely “forces its legacy branch,” while its test must observe an actual legacy write or plane change and must not rely on transport-gate refusal ([plan lines 431–437](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:431)). That restriction is normative ([spec lines 502–509](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:502)).

The route predicate and legacy authorization read the same gate: Owner selects the owner route, while `allows_legacy` is true only in `Legacy` ([platform.rs lines 3440–3455](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:3440), [transport.rs lines 412–428](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/transport.rs:412)). For the unflip sink, a false permit returns before constructing or submitting the atomic request ([modeset.rs lines 1718–1733](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/drm/modeset.rs:1718)).

Concrete sequence: install an Owner gate; mutate the high-level fork to take the legacy branch; the site obtains `false` from `allows_legacy`; the sink exits before the ioctl. No plane changes. A test that fails because the owner commit vanished is observing the defense’s refusal indirectly; a test demanding the promised external write will not catch this mutation.

Smallest correction: define each mutation and observation so the gate cannot be causal—for example, observe invocation of the real legacy sink at a recorder immediately before authorization, or make the test-safe mutation force both the branch and an accepted fake sink transaction. The test must fail on that independent legacy-sink observation, not merely on missing owner progress or the refusal error.

### Minor

None.

## Coverage and implementation checks

All four checks were performed. **24/24 bounded excerpts** were used. Verified ground covered all six prior corrections, §§6.1–6.4 and §§8.2–8.4, request/retry execution points, direct-group device constraints, dispatch/resource ownership, retirement routing, transport authorization, the three enumerated legacy sites, hardware-test selection, and P3-2/P3-3 registration semantics.

Budget-exhausted and therefore unassessed—not declared sound—are exhaustive device-blind layout-change paths, real card1 retained-allocation reachability, and the eventual detailed test-fixture instrumentation.

Compilation, formatting, clippy, cross-target checks, Vulkan/hardware execution, and mutation runs remain deferred to implementation.