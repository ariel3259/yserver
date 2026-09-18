## Verdict

**3 blocking, 1 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md`
(a stage **design**, passed as the target the way the 2c-i design was in its
round 3), against the authoritative parent
`docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md`.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.
**Not comparable** to the stage 2b-ii and 2c-i rounds, which cite `13637318`.

**Recorded usage:** 41,613 tokens reported by the completed process (exit 0),
excluding the author session. 12/12 bounded excerpts used. No build, test or
nested reviewer was run.

**Author verification (2026-09-16), every finding checked against the tree:**

- **B-1 — CONFIRMED.** The parent design, lines 158–163, states it verbatim:
  "Cache removal, drawable destruction and pool replacement detach logical
  owners; they cannot destroy backing allocations retained by a live or
  quarantined lease. Stage 3 must transfer those retained owners … into its
  teardown supervision". Lines 221–230 give the supervisor the move-by-value
  bundle and forbid it publishing availability into a new incarnation. The
  reset design (lines 40–58, 342–350) promises erasure of *protocol-visible*
  state — resources, atoms, selections, grabs, properties — and says a reset
  "never touches KMS, Vulkan". So the target's section 3.3 posed a
  contradiction the parent had already dissolved into a two-level contract,
  and — the part that makes this blocking — its future-mutation criterion
  ("forced teardown drops a managed lease → the tripwire must fail") would
  fire on **correct** code: `Storage::destroy`'s Managed arm drops that exact
  Retain lease today, by design. This is the third time the author revised
  section 3.3's framing in two days, and the answer was in the document passed
  as `--spec`.
- **B-2 — CONFIRMED by experiment.** Compiled with `rustc --edition 2024 -D
  warnings` against a `#[must_use]` receipt: a bare `detach();` fails with
  "unused `Receipt` that must be used", but `let _ = detach();` and
  `drop(detach());` both compile cleanly and discard the receipt unaccounted.
  The target's "ignoring the receipt fails the build" holds only for the
  accidental bare-statement form. The device/incarnation correlation gap is
  also real: nothing in the proposed receipt names its registry.
- **B-3 — CONFIRMED.** `drm_cleanup.rs:376-383` is an unkeyed scalar
  (`register_pool_husk` is `+= 1`, `unregister_pool_husk` is
  `saturating_sub(1)`). With one husk live, a spurious unregister reaches
  zero and a pre-subtraction `debug_assert!(count > 0)` passes; release builds
  remove the assertion regardless. The proposed fix does not detect the hazard
  it names.
- **M-1 — CONFIRMED.** The census counts a mutation as caught when *any* test
  fails, so a mutation killed by an incidental panic or unrelated assertion is
  indistinguishable from one killed by the intended oracle. It enumerates only
  syntactic `if` guards and refusing match arms, so `?`-propagated refusals,
  helper-return propagation and absent guards are invisible to it, and "zero
  survivors" over-claims completeness. The target already identified guards by
  function and invariant, but its review procedure (section 4) still keyed
  mutation re-runs by line number.

No finding was a false positive. No plan corrections or further external
review were performed in this round; the corrections below are pending the
user's decisions on scope.

This is a design-review result only. It does not claim compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Status |
|---|---|
| None | No prior review exists; check 1 was skipped as directed. |

## Findings

### Blocking

#### B-1 — The reset “contradiction” conflates logical erasure with physical destruction

The target says proof-gating conflicts with reset because “nothing client-created survives,” leaves “which wins” unresolved, and proposes a tripwire that must fail if forced teardown drops a managed lease ([target lines 121–154](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:121)).

The authoritative design already separates these events:

- Logical destruction detaches the drawable owner; live backing allocations remain retained ([parent lines 134–164](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:134)).
- Before store/backend destruction, outstanding entries move to the process-lifetime teardown supervisor ([parent lines 221–272](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:221)).
- Missing KMS proof explicitly leaves entries quarantined and rooted ([parent lines 274–295](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:274)).

Reset’s “full erasure” concerns session-visible state and identity; it also says reset never touches KMS or Vulkan ([reset design lines 40–58](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-09-server-reset-design.md:40), [342–345](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-09-server-reset-design.md:342)). Moreover, `Storage::destroy` already drops the managed Retain lease while leaving physical reclamation to the service ([store.rs lines 804–829](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/store.rs:804)).

Concrete failure: a correct future reset removes the old XID and drops its Retain lease, while a pending `KmsRelease` keeps the backing entry in the supervisor. The proposed future mutation treats that required lease drop as something the tripwire must reject.

Required correction: replace the open question with the authoritative two-level contract: reset erases old-generation identities immediately, transfers proof-bearing backing ownership atomically to the supervisor, and prevents new-generation access. The tripwire must assert those properties and delayed physical cleanup—not fail merely because a managed lease is dropped.

#### B-2 — The husk receipt is neither compulsory nor identity-correlated

The target proposes a `#[must_use]` receipt that is “discharged either by handing it to a registry or by explicitly asserting it was empty,” and claims ignoring it fails the build ([target lines 84–102](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:84)). `#[must_use]` is advisory: `let _ = receipt` or `drop(receipt)` satisfies the compiler without accounting anything. More importantly, the design does not bind the receipt to the originating device/incarnation/registry.

Concrete failure: detaching device A produces a nonempty receipt; it is discarded or applied to registry B. A remains permanently over-counted, while B can under-count and mint a false fd-family barrier. This violates the per-device proof rule ([parent lines 274–289](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:274)).

Required correction: specify a linear, device/incarnation-bound husk registration token or receipt. Its only normal consumers must be matching-registry discharge or a checked-empty operation; dropping an undisposed nonempty value must fail closed. Do not claim `#[must_use]` alone makes accounting unskippable.

#### B-3 — A debug-only underflow check cannot protect release authority

The target retains `saturating_sub` and adds only `debug_assert!` ([target lines 104–109](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:104)). The current production operation is an unkeyed scalar decrement ([drm_cleanup.rs lines 376–383](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:376)).

Concrete failure: with husk A still live, a duplicate unregister for B can consume A’s count. A pre-subtraction `debug_assert!(count > 0)` passes because the count is one; release builds remove the assertion anyway. The registry can then certify zero aliases while A remains. Thus the proposed fix neither detects mismatched unregisters nor provides a production failure policy.

Required correction: consume the unique registration token identified in B-2, or otherwise validate exact registration identity. Underflow/mismatch must be an unconditional error or fail-closed state that prevents barrier minting—not saturation plus a debug assertion.

### Major

#### M-1 — “Zero survivors” proves mutation sensitivity, not systematic refusal coverage

The census enumerates syntactic guards in only three files and mutates each guard independently ([target lines 24–46](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:24)). Its global acceptance criterion is zero survivors, with review keyed by mutable line numbers ([target lines 156–174](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:156)).

This cannot detect:

- a required refusal guard that is absent;
- distinct predicates combined inside one guard;
- refusals encoded through `?`, helper-return propagation, assertions, or state-transition APIs;
- a mutation killed by an incidental panic or unrelated assertion rather than the intended disposition.

Concrete failure: every enumerated mutation is killed while an absent identity check still permits stale evidence, or a swallowed error still returns the wrong externally visible disposition through a non-guard construct. The report nevertheless declares the surface complete.

Required correction: define zero survivors narrowly as coverage of the enumerated guard-clause baseline. Add a bounded refusal/invariant inventory derived from the parent contracts, record excluded control-flow forms, and require each mutation to be killed by an oracle for the intended state/error/result—not merely any test failure. Use stable function/invariant/site identifiers rather than line number as the mutation identity.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** skipped; no prior review.
- **Architecture/contracts:** checked reset ownership transfer, production-activation boundary, managed destruction, and husk accounting.
- **Safety/ownership:** checked proof-gated lifetime, device correlation, exact unregister semantics, and release-before-proof scenarios.
- **Specification/verification:** checked R8/R9, stage-3/4 activation ownership, census scope, tripwire validity, and evidence strength.
- **Excerpts used:** 12/12 bounded spec/source excerpts, plus locator searches and the required single target read.
- **Verified ground:** the reset path performs logical backend frees; managed `Storage::destroy` detaches its lease; physical cleanup remains service-owned; current husk accounting is scalar.
- **Not assessed as sound:** every individual census classification, detailed guard implementations in `commit/gpu/transport.rs`, or future supervisor implementation.
- **Deferred to implementation:** real compiler diagnostics, exact APIs, test construction, formatting, clippy, full suites, hardware runs, flake loop, and portability gates.