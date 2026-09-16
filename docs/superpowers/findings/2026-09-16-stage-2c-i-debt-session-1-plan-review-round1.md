## Verdict

**1 blocking, 3 major, 1 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** `docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-1.md`
at `baf00958`, against `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md`.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.
First review of this plan.

**Recorded usage:** 52,554 tokens reported by the completed process (exit 0),
excluding the author session. 8/12 bounded excerpts.

**Author verification (2026-09-16):**

- **B-1 — CONFIRMED.** Spec §5.1 required "zero survivors over the enumerated
  baseline of section 2", a baseline that includes session 2's guards; it was
  written before the stage was split into sessions. The plan amended §§3.1 and
  4.4 but not §5.1, so Task 8's "exactly eight survivors" contradicted the
  governing text.
- **M-1 — CONFIRMED in the tool's code.** `load_tags` collected tag lines
  without binding them to the test below; the oracle accepted any failing test
  whose output carried any of the site's markers; nothing enforced marker
  uniqueness or recorded the strategy.
- **M-2 — CONFIRMED as a strength gap, not a false pass.** The test did kill
  its mutation — the author had run the tool on it (`CAUGHT_BY_ORACLE`), because
  the fixture's releasable, roleless rejected resource is dropped when
  processed — but the marked assertion checked only cardinality, which is
  weaker than the "untouched" invariant it names.
- **M-3 — CONFIRMED empirically.** Probed with `codex exec --sandbox
  workspace-write`: `/dev/dri` does not exist, `c0_2ci_fd_family_barrier_real_gbm_payload_drm`
  panics, and `c0_2ci_live_lifetime_adapters_vulkan` reports "environmental
  skip: no live Vulkan ICD available". The full census cannot run in the
  implementer's sandbox; the green-baseline precondition refuses it there, as
  intended.
- **m-1 — CONFIRMED.** The preface required Claude-harness skills and every
  commit carried a Claude Sonnet trailer, for a codex implementer.

**Corrections applied** (plan revision after this review):
- B-1: Task 1 Step 2 amends spec §5.1 — session 1 accepted with its 27 guards
  proven by oracle and exactly the eight session-2 survivors left; `} else if`
  sites beyond the legacy 67 reported, not accepted or rejected.
- M-1: the tool binds each tag to the `fn` directly below it; refuses unbound
  tags, duplicate markers and doubly tagged sites; proves a site only when that
  bound test fails carrying that marker; records the strategy; and classifies a
  kill under whole-body replacement as `CAUGHT_WHOLE_BODY`, never an oracle
  proof.
- M-2: the assertion now compares the rejected resources' allocation keys and
  the released-present count, not the vector length.
- M-3: an *Execution split* section. The implementer proves each family with a
  new `--deterministic-only` mode, which requires `--require-oracle`, checks only
  tagged sites and is never valid for survivor accounting; steps needing GPU and
  DRM are marked [H] and run by the coordinating session.
- m-1: codex preface citing the skills by path; `Implemented-By: codex (model
  luna, reasoning effort xhigh)` in place of the Claude trailer.

**Validated before committing the corrections, then reverted:** all 27 tests
compile, pass and are clippy-clean with the strengthened assertion; the revised
tool enumerates 67 legacy sites; in `--deterministic-only --require-oracle`
mode it proves both `on_available` guards through their bound tests (exit 0);
it refuses `--deterministic-only` without `--require-oracle`; and in full mode
it still reproduces the published `CAUGHT` for the untagged validate-all guard.

## Incorporation audit

| Prior finding | Status |
|---|---|
| None | First review; check 1 skipped as instructed. |

## Findings

### Blocking

#### B-1 — Task 8 accepts eight survivors while the authoritative acceptance clause requires zero

The corrected scope of 27 guards is internally reasonable: moving `consume_owner_write` alongside the handover mechanism avoids certifying a test fixture that session 2 will replace; the family-C wording matches the actual transitions; and the first `on_available` guard needs a state/disposition observable. But the plan amends only spec §§3.1 and 4.4 (plan lines 372–379), not §5.1.

The authoritative acceptance criterion still requires “zero survivors over the enumerated baseline” and every killing test to be related (spec lines 292–295). Task 8 instead declares success with exactly eight survivors (plan lines 1321–1328). Thus following the plan exactly can mark session 1 complete under a condition the governing spec expressly rejects.

Smallest correction: as part of Task 1, amend spec §5.1 to define session-1 acceptance as all 27 session-1 targets proven and exactly the eight enumerated session-2 survivors remaining. Preserve the baseline total and explicitly state that newly discovered `else if` sites remain reported but unaccepted pending a scope decision.

### Major

#### M-1 — The oracle does not establish that the tagged test or its own assertion killed the guard

The spec requires the association to live beside each test and requires both the tagged test and its guard-specific observable to fail (spec lines 84–97). The tool merely gathers tag lines globally into a site-to-marker map (plan lines 235–244), then accepts when any failing test’s output contains any marker assigned to that site (lines 274–307). It never records the test function following a tag, verifies adjacency, or enforces marker uniqueness.

Concrete failure: a tag can drift above test A while the same marker remains in test B—or a marker can be duplicated or printed independently. If B fails after mutation, the site is `CAUGHT_BY_ORACLE` even though the tagged test did not fail. The assertion’s semantics are also trusted entirely from its prose message.

The whole-body strategy compounds this: it deletes recovery/state operations before the refusal (plan lines 199–213), so a marked postcondition can fail because those operations vanished rather than because the refusal was swallowed.

Smallest correction:

- Parse each tag together with the immediately associated `#[test] fn`.
- Require one site, one marker, and one associated test; reject duplicate markers/tags.
- Require that exact test name to be among the failures and its failure block to contain the unique marker.
- Record the successful mutation strategy. Do not allow whole-body deletion to qualify as oracle proof without an explicit per-site justification that the marked assertion observes the refusal rather than removed preparatory/recovery effects.

#### M-2 — The first `on_available` test proves only cardinality, not “untouched”

The correction says the first transition-error guard protects rejected resources from being processed after a releasing-half failure (plan lines 30–32). The test, however, marks only `rejected_resources.len() == 1` (lines 1013–1049).

Actual control flow takes the rejected vector and may drop, retain, or alter each resource after the first return is removed (source `commit.rs` lines 441–489). A one-for-one replacement, reordering, or in-place disposition change preserves length and satisfies the marked assertion despite processing the resource. This is weaker than the claimed “untouched” observable and spec’s guard-specific state/disposition requirement (spec lines 92–97).

Smallest correction: capture and assert the rejected allocation/resource identity and its drop counter, plus any disposition/role state that processing can alter. The marker should be on that compound state assertion, not only the vector length.

#### M-3 — The required census cannot run in the stated implementation sandbox

Every census execution includes ignored hardware tests (plan lines 60 and 94). The plan itself acknowledges that absence of GPU/DRM access makes the green baseline refuse to start (lines 263–272), yet Task 1 requires an hour-long baseline census (lines 352–359) and Task 8 requires another full census (lines 1319–1328). The stated Codex workspace-write sandbox is not promised access to DRM/Vulkan devices.

The green-baseline precondition is sound and must not be weakened: it prevents pre-existing failures from making every mutation appear caught. The missing contract is who runs the hardware-required measurement when the implementation harness cannot.

Smallest correction: split execution explicitly. Codex may implement and run non-hardware gates, while the full green-baseline and acceptance censuses must be run by the user or a named hardware-capable environment, with their JSON and transcript supplied before Task 1/8 can be accepted. Do not silently omit `--include-ignored`.

### Minor

#### m-1 — The execution and commit instructions falsely assume a Claude harness

The plan mandates unavailable `superpowers:subagent-driven-development` or `superpowers:executing-plans` behavior (plan line 3) and requires every commit to attribute Claude Sonnet 5 (line 23 and each commit recipe), although the stated implementer is Codex/luna. That produces inaccurate provenance and an execution prerequisite the implementer cannot satisfy.

Smallest correction: replace the harness-specific preface with ordinary sequential Codex instructions and remove or correct the Claude co-author trailers.

## Coverage and implementation checks

- **Incorporation:** no prior review existed. The three scope corrections were assessed. Their substance is sound, but the acceptance clause was not updated, producing B-1.
- **Architecture/contracts:** reviewed census identity, tag ownership, oracle association, task dependencies, session boundaries, acceptance accounting, and hardware execution ownership.
- **Safety/failure semantics:** checked green-baseline behavior, restoration intent, mutation ordering, `consume` transition recovery, `on_available` ordering, releasability disposition, and handover-dependent grant creation. The green precondition correctly prevents baseline-failure false positives.
- **Spec/verification:** all families B–I and §§2, 3, and 5.1 were mapped to tasks. No other in-scope normative family was missing.

**Excerpts used: 8/12**: three spec excerpts and five bounded logical source excerpts from `commit.rs` and `transport.rs`. Locator searches were used only to find named functions.

Not reassessed: API spelling, constructors, imports, compilation, individual tag identities, and the reported 27-test results, per the boundary. Actual formatting, clippy, test, flake, workspace, mutation, and hardware results remain deferred to implementation; this review does not claim they pass.