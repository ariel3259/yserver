# Copied scanout route design — codex review, round 1

**Target:** `docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md`
revision 1 (`fcda73c8`), against the 2c-iii conversion design as the passed parent.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage COMPLETE FOR DECLARED SCOPE, 24/24 excerpts used: `CompletionPoller`
internals, device-teardown implementation, transport encoding and unrelated
owner-ledger paths are recorded as unassessed.

**Author verification (2026-09-22), every finding checked against the tree:**

- **B-1 — CONFIRMED.** The copied acquisition reserves the destination as
  `Write` and the renderer source as `Write`
  (`platform.rs:6347`, `platform.rs:6355`), and `is_compatible`
  (`resources/availability.rs:116`) refuses a `Read` while a writer is live and
  a `Write` while any use or pending obligation exists. So B cannot re-reserve
  the source while A's lease lives, and releasing A's lease first leaves an
  interval with no live use on it. Revision 1 asserted CP-4/CP-5 without ever
  saying how ownership crosses the boundary. Fixed: section 3.2 rewritten around
  one named transition.
- **M-1 — CONFIRMED.** `register_scanout_render_completion` is fallible and its
  existing caller propagates the error (`scene.rs:6577`). Revision 1's failure
  section covered submission failure, cancellation, supersession and deadline
  expiry, but not a successful copy whose wake registration then fails. Fixed:
  section 3.3 gains that row.
- **M-2 — CONFIRMED by reading the table itself.** CP-4's row mutated only the
  read obligation, so an implementation that registered no destination
  obligation could still pass every listed mutation, and section 6's hardware
  claim was circular without it. Fixed: CP-4 gets a second mutation and the
  evidence must record the destination obligation's registration and retirement.

**Found by the author while verifying, not by the review:**
`ReadObligation::new` and `bind_read_obligation` have **no production caller** —
`resources/guard_tests.rs:202`, `:221`, `:244` and `resources/tests.rs:1851` are
the only ones. CP-4 as written rested the source's retention on machinery that
only tests drive, which is the F-T6-4 defect class from plan Ciii's hardware run
(`2026-09-22-stage-2c-iii-plan-ciii-accepted.md`). The route does not stop being
sound — it becomes that machinery's first production caller — but the design now
says so, and the plan's evidence must prove the production path rather than a
test calling it by hand. Related: `ResourceService::register`
(`resources/mod.rs:865`) mints an obligation without consulting `is_compatible`,
so obligations and leases move independently; that is what makes the transition
of section 3.2 expressible.

Revision 2 incorporates all three findings and this observation.

---

## Verdict

**1 blocking, 2 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result only; it does not approve implementation or claim that code compiles or tests pass.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| None | N/A | First review; check 1 skipped as instructed. |

## Findings

### Blocking

#### B-1 — The A→B resource handoff has no exclusive ownership protocol

The design jumps from A’s completion to submitting and registering B ([target design](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:99), lines 99–107), while asserting that B owns both allocations and that the source can never be recycled between stages (lines 132–146). It does not define how ownership crosses that boundary.

The existing acquisition reserves both destination and source as `Write` ([platform.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:6341), lines 6341–6365). A source `Read` lease cannot coexist with A’s live writer, while another `Write` is also blocked by live uses and pending obligations ([availability.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/availability.rs:116), lines 116–126). Servicing A removes its obligations and drops its batch ([resources/mod.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:1540), lines 1540–1560), with availability processing occurring in the same service operation (lines 393–406).

Therefore:

1. Taking B’s source `Read` before retiring A fails `Busy`.
2. Retiring A first releases its resource ownership before B has acquired its read obligation unless a no-yield/atomic transition is explicitly guaranteed.
3. Intermediate reservation or copy-preparation failure has no defined disposition for the partially transferred destination/source pair.

This contradicts CP-4/CP-5 and can expose the source for reuse before B finishes. It also violates the authoritative requirement that composed readiness follows all finished producer waits ([authoritative spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:307), lines 307–312) while keeping producer fences out of the owner ioctl (lines 389–393).

Smallest correction: specify one authoritative A→B transition transaction. It must state which entity retains the destination write ownership, how the source changes from A’s write ownership to B’s read ownership without a reusable interval or event-loop yield, and the rollback/quarantine disposition at every failure boundary. The design may choose an atomic resource-service transition or prove an equivalent exclusion mechanism, but cannot delegate the ownership ordering itself.

### Major

#### M-1 — Successful copy followed by completion-wake registration failure has no recovery owner

The selected architecture requires the copy’s sync-file to wake the loop while the resource service authorizes readability (target lines 68–80 and 117–120). Yet the failure section covers copy submission failure, cancellation, supersession and deadline expiry only (lines 148–165). Section 9 delegates which poller is used without defining failure after B has already submitted (lines 304–307).

The existing analogous registration is fallible ([scene.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:6575), lines 6575–6585). Concrete sequence: B submits successfully and owns a live batch; registering its sync-file fails. The resource service may still poll and retire the batch through its periodic deadline wake, but the design specifies no generation-correlated event that may then promote or displace the buffer. The generation can remain `Rendering` indefinitely, or an implementation may incorrectly use generic availability as an offer signal.

Smallest correction: assign this boundary explicitly. On wake-registration failure, require the generation to become `Displaced`, ensure any submitted batch remains registered until retirement/quarantine, remove partial waiter state, and forbid an offer. Alternatively, define and correlate a resource-service-only completion path that does not depend on poller registration.

#### M-2 — Verification cannot prove the destination write obligation exists

CP-4 requires both a destination `GpuObligation` and a source `ReadObligation`, but its mutation removes only the read obligation (target lines 268–269). CP-1 mutations change offer timing, not whether a destination obligation was registered (lines 264–266). Consequently, an implementation could wait for B’s fence, call the service with no destination obligation, and then offer; the listed mutations can still pass even though the service never authorized destination readability.

The hardware claim that the destination was not offered before “its obligation retired” (lines 219–223) is circular unless evidence first establishes that the correct destination key and obligation were registered. This falls short of both the target’s one-mutation-per-criterion rule (lines 258–260) and the authoritative verification rule ([authoritative spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:576), lines 576–580).

Smallest correction: add a mutation that removes or miskeys the destination write obligation, with an observation proving the destination remains unreadable/unofferable. Fixture or hardware evidence claiming ordering must record the destination obligation’s registration and retirement, not merely fence order or a service call.

### Minor

None.

## Coverage and implementation checks

- Incorporation: skipped because no prior review exists.
- Architecture/contracts: checked producer stages, ownership handoff, copied-pool adoption, per-device resource validation, eligibility, exclusivity and event-loop delivery.
- Safety/failure: checked source/destination lease compatibility, batch retirement, supersession/cancellation claims and the post-submit registration boundary.
- Compliance/evidence: compared authoritative sections 3, 4, 6 and 8 against CP-1–CP-11, hardware evidence and mutations.

Excerpts used: **24/24** beyond the target—4 authoritative-spec excerpts and 20 source excerpts. The budget was exhausted, so CompletionPoller internals, device-teardown implementation, transport encoding and unrelated owner-ledger paths remain unassessed and are not deemed sound.

Builds, tests, clippy, formatting, cross-target checks, fixture construction and GPU execution remain correctly deferred to the implementation plan and real toolchain.