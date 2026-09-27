# C.0 spec revision 5 — codex review, round 1

**Target:** `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md` revision 5 (`74885742`), same-file review against the rest of the document and the user decisions of 2026-09-16, 2026-09-19 and 2026-09-24.

**Result:** 1 blocking, 1 major, 1 minor; coverage COMPLETE FOR DECLARED SCOPE (24/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.157.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Comparability:** this round ran `codex-cli 0.157.1` (updated with the system on 2026-09-27); earlier rounds ran 0.155.1. The instrument SHA is unchanged, but the CLI version is one of the five variables the review README names, so counts are not strictly comparable.

**Author verification (2026-09-27):** all three CONFIRMED and fixed in the same revision.

- **B-1** — CAP-4 required a slow helper ioctl as the cause; a prompt ioctl whose cursor update retires late never demoted. With `OwnerMediatedLegacyMove` gone, CAP-4 alone guards Goal 4. Fix: a second cause term, p99 dispatch-to-fence latency of cursor-only commits above two mode periods (`CursorCompletionMax`); cursor-only commits carry no client primary, so slow clients and unrelated primaries cannot trigger it. Test 85.
- **M-1** — the retained direct shape contract lost the below-cursor stacking requirement. Fix: structural capability requires the primary below the cursor (`zpos` checked or set; DRM plane-type order when absent); failing pairs keep the software/unflip transition. Test 86.
- **m-1** — §9.2.1 kept "coordinate-only intent is never absorbed" from the removed lane. Fix: a compatible changed position is absorbed like any changed cursor generation.

## Review as received

## Verdict

**1 blocking, 1 major, 1 minor**  
**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a bounded design review. It does not establish that the code compiles, tests pass, or the implementation is approved.

## Incorporation audit

| Prior findings | Disposition |
| --- | --- |
| None | First review; check 1 skipped. |

## Findings

### Blocking

**B-1 — CAP-4 cannot reliably enforce the retained coordinate-latency goal.** The spec retains the shipping latency goal and says measured demotion preserves it, but CAP-4 requires *both* a low cursor retirement rate and a slow helper ioctl ([§6.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:392), [§16.3](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2798)). A synchronous atomic ioctl can return promptly while its cursor update remains pending until a later fence. Under continuous motion, a device can therefore have visibly delayed updates, with each fence still inside the hardware deadline, yet never satisfy the slow-ioctl half of CAP-4. It stays on hardware despite missing the retained latency goal. Add a runtime latency or completion-based demotion trigger with attribution rules that exclude slow clients and unrelated primary work; test prompt ioctl replies paired with delayed cursor retirement.

### Major

**M-1 — The retained direct-scanout shape omits the below-cursor stacking check.** Revision 5 says it preserves the direct frame composition contract, but the new predicate checks coverage, scale, format and HDR state without requiring that the selected primary compose below the cursor ([§7](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:871), [revision note](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2822)). If a selected plane configuration places the full-output XRGB8888 primary above the cursor, it passes the stated shape check while hiding the hardware sprite. Serializing commits cannot fix stacking. Require and test a primary-below-cursor order for direct eligibility, or document and verify the discovery guarantee that makes the check unnecessary.

### Minor

**m-1 — A removed coordinate-path rule remains in scheduler admission.** Section 9.2.1 still says “coordinate-only intent is never absorbed,” while the revised scheduler treats changed cursor positions as ordinary persistent cursor-plane state and requires compatible absorption ([§9.2.1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1205), [§16.2 test 33](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2464)). For an aged position update beside a ready direct successor, the two instructions permit different admission decisions and an avoidable extra commit. Delete the obsolete exclusion and state that a compatible changed position may be absorbed; an unchanged cursor remains omitted.

## Coverage and implementation checks

- **Incorporation:** No prior review.
- **Architecture and contracts:** Checked revised capability and cursor policy, owner admission, cursor lifecycle, direct scanout, stage 5 activation, and the stage 2c-ii §8 stub. The stub contains no remaining coordinate-path door.
- **Safety and failure semantics:** Checked single-slot ordering, detach-before-software-reveal rules, and the per-incarnation deadline bootstrap. The inspected [deadline implementation](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/deadlines.rs:37) uses the specified 30-second unobserved ceiling; the inspected qualification record is keyed to incarnation and topology.
- **Compliance and verification:** Checked the revised §16.2 tests, §16.3 evidence rule, §17 acceptance and §18 implementation gates. Physical runs remain falsification evidence, and formatting, CI Clippy, tests and portability gates are assigned to implementation.

**Excerpts used: 24/24.** Investigation stopped at the reading limit. Unchanged recovery and telemetry internals and broader production admission paths were not audited and are not assessed as sound. No builds, tests, benchmarks or compiler experiments were run.