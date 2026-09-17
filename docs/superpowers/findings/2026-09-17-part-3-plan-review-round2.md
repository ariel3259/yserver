# Part 3 plan — codex review, round 2

**Target:** `docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md` revision 2 @ `571da432`
**Result:** 0 blocking, 2 major, 0 minor; coverage complete for declared scope.
Round 1 on revision 1 (same instrument, so the counts compare): 2 blocking, 3 major.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA.

## Author verification (2026-09-17)

Both verified against the tree, both taken; revision 3 carries them.

- **M-1 — CONFIRMED, and a fair downgrade of round 1's M-3 to PARTIAL.**
  `c0_2ci_sink_gamma_gate_four_states_drm` opens its own node through
  `TestDevice::open_real_drm_or_ignore` — deliberately without master — and
  drives four gate states. My Task 6 wrote a *new* test over a different
  fixture, which would have changed the fixture, the output selection and
  possibly which arms run: it would have measured something else and let the
  record claim the existing assertion was master-dependent without ever having
  run that test's path under master. Revision 3 parameterises the existing
  test's own body into a helper, leaves the existing test calling it unchanged,
  and invokes the same helper over the same four states with a master-holding
  device.
- **M-2 — CONFIRMED.** A flip is accepted asynchronously: `on_page_flip_complete`
  (`platform.rs`) is what retires the pending BO, and
  `sync_file::query_status` distinguishes `Pending`, `Success` and `Error(i32)`.
  My plan said "read the CRTC afterwards" with no deadline and no correlation,
  which is timing-dependent evidence; worse, a lost event or a stuck fence would
  hang the run, and a process killed by hand never unwinds, so the fixture's
  display restoration would not run — the exact outcome round 1's B-1 was about.
  Revision 3 adds a bounded, correlated wait as a Global Constraint, and makes a
  timeout or fence error panic so restoration happens.

Trend across the two rounds, same instrument: 2 blocking + 3 major → 0 blocking
+ 2 major, with both majors about the test protocol rather than the design.

No implementation has been dispatched.

---

## Verdict

**0 blocking, 2 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only. It does not claim that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Audit |
|---|---|---|
| B-1 — no display restoration | **APPLIED** | The fixture now snapshots the original CRTC state and mandates unwind-safe restore → resource destruction → master release ordering, including loud restoration-failure reporting ([plan lines 40–41](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:40), [123–131](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:123)). |
| B-2 — two out-fence owners | **APPLIED** | BO state is explicitly the sole owner; tests borrow through that owner or close only a duplicate ([plan lines 42–43](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:42), [224–228](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:224)). This agrees with `transition_to_pending` adopting the fd ([scanout.rs lines 387–398](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/vk/scanout.rs:387)). |
| M-1 — live tests not serialized | **APPLIED** | The fixture owns a process-wide guard and the run command mandates `--test-threads=1` ([plan lines 41–42](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:41), [276–280](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:276)). |
| M-2 — census cannot express mutations | **APPLIED** | Task 1 defines reversible, source-anchored `p3-2`/`p3-3` mutations, baseline and compile checks, failure reporting, and restoration in `finally` ([plan lines 67–93](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:67)). Task 7 requires each named assertion to kill its mutant ([286–292](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:286)). |
| M-3 — master-held audit inferred | **PARTIAL** | Task 6 supplies a master-held gamma execution, but substitutes a new test that drives the same entry point for running the identified existing hardware test under master. That does not fully satisfy the claimed correction; see M-1. |

## Findings

### Blocking

None.

### Major

#### M-1 — Task 6 does not run the identified existing hardware test with master

The specification requires identifying tests that encode the no-master outcome “by running them with master” ([spec lines 464–468](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:464)). The existing gamma test constructs its own non-master file, substitutes it into a synthetic backend, and exercises all four transport states ([tests.rs lines 3223–3236](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/tests.rs:3223), [3239–3320](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/tests.rs:3239)).

Task 6 instead creates another test over `LiveKmsFixture` and merely drives “the same gamma entry point” ([plan lines 252–260](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:252)). That changes fixture construction, output selection, state setup, and potentially which arms execute.

Concrete failure scenario: the new Legacy-only or live-output path succeeds under master, while the existing four-state test would still fail before reaching the relevant ioctl because of its synthetic CRTC/output setup. The record would then claim the existing assertion was master-dependent without ever executing that test’s complete path under master.

Smallest correction: refactor the existing test body into a parameterized helper accepting the device/backend fixture, retain the current non-master test unchanged in meaning, and add a master-held invocation of that exact helper over all four states. Record both outcomes and identify the existing test by name.

#### M-2 — Hardware observations have no bounded, ordered completion contract

Submission acceptance is asynchronous. Production bookkeeping transitions the submitted BO only when the page-flip-complete callback runs ([platform.rs lines 7166–7223](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:7166)). Yet P3-1 requires only an accepted submission followed by a device readback “afterwards” ([plan lines 181–187](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:181)); P3-2 and P3-4 require completion/fence observations without specifying a deadline or a matching-completion wait ([205–207](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:205), [224–228](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md:224)).

Concrete failure sequences:

1. `submit_flip_with_fences` returns `Ok`.
2. P3-1 reads the CRTC before the matching event is consumed and observes the old framebuffer, producing timing-dependent evidence.
3. Alternatively, a lost event or permanently pending fence leaves the test and run script blocked indefinitely.
4. Because the fixture never unwinds, its promised restoration does not execute; manual termination can leave restoration dependent on kernel client cleanup rather than the stated contract.

The canonical fence query distinguishes `Pending`, `Success`, and `Error` ([sync_file.rs lines 20–40](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/platform/sync_file.rs:20)), so an explicit bounded protocol is possible.

Smallest correction: add to the fixture/test contract a deadline-bounded wait for the matching page-flip completion, correlate it to the submitted output/BO before P3-1 readback and P3-2 discharge assertions, and poll P3-4’s canonical status only until `Success`, `Error`, or deadline. Deadline/error exits must panic through normal fixture unwinding so display restoration runs, and the record must distinguish timeout from fence error.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** audited all five round-1 findings; four are applied and M-3 is partial.
- **Architecture/contracts:** checked fixture authority, device selection, serialization, mutation producer/oracle relationship, gamma-test reuse, and asynchronous completion delivery.
- **Safety/ownership:** checked master/display teardown, out-fence ownership, BO transitions, panic recovery, ordering, and missing deadlines.
- **Compliance/verification:** mapped spec §§9.2–9.4 and §3.0 against Tasks 1–7, including real evidence, master detection, mutations, logging, and the explicit §16.3 exclusion.
- **Excerpts used:** **12/12**. Verified ground includes spec §§3.0 and 9, the existing gamma test, census behavior, obligation registration/discharge, BO fence ownership, page-flip retirement, canonical fence status, and the referenced managed-buffer fixture.
- **Unassessed:** exact connector/encoder/plane APIs, detailed fixture construction, exact mutation replacement text, and test-body Rust mechanics. These are not deemed sound.
- **Deferred to implementation:** compiler/type/borrow checks, ioctl portability, formatting, `cargo clippy --all-targets -- -D warnings`, deterministic counts, mutation compilation, and actual kernel/driver outcomes.