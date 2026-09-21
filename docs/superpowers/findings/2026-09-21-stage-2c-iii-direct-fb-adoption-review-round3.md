# Direct framebuffer adoption design — codex review, round 3

**Target:** revision 3 (`e4d2706d`), with round 2 as the prior review.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage INCOMPLETE (24/24): DRI3 same-XID re-import/disconnect, family-closure
minting, the owner-unflip dispatch glue and successful exit-retirement release
were not reopened.

**Author verification (2026-09-21), every finding checked against the tree:**

- **B-1 — CONFIRMED.** Alias accounting is keyed by adopted `AllocationKey`
  only (`register_payload_alias`, `drm_cleanup.rs:310`); a failed cleanup keeps
  the device alias; `consume` cannot succeed once frozen. Fixed: keyless counted
  pending-cleanup entries with retry / freeze-handoff / family-closed dispositions.
- **M-1 — CONFIRMED.** The commit consumer never sees a preparation-time lease
  or a never-submitted successor's drop. Fixed: the count is the service's
  live-use count on the key, valid because only direct-frame holders lease a
  framebuffer allocation (pinned by a mutation).
- **M-2 — CONFIRMED.** The unflip seam moves the whole record to
  `ExitRetirement` (`backend.rs:21168`) and `ResourcesStillCurrent` only extends
  (`commit.rs:358`). Fixed: four unflip rows in 3.3, rejection restores `Current`.
- Incorporation: round-2 M-1 APPLIED; B-1 and M-2 PARTIAL, closed here.

Revision 4 incorporates all three.

---

## Verdict

**1 blocking, 2 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result only; it does not claim that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Round-2 finding | Status | Audit |
| --- | --- | --- |
| B-1 — failed cleanup lacks a retry owner | **PARTIAL** | Revision 3 adds a pending-cleanup owner and ordinary R3 retry, but does not integrate that owner’s unadopted device alias/right with fd-family accounting and teardown handoff. See B-1. |
| M-1 — preparation-time storage adoption breaks existing consumers | **APPLIED** | Section 2.5 no longer adopts storage, uses the existing present pin for unmanaged backing, passes an optional lease for already-managed storage, and addresses device scope. |
| M-2 — no allocation-correlated last-role hook | **PARTIAL** | Section 2.3 names a per-key count and `1 → 0` rule, but assigns it to a consumer that does not observe preparation or never-submitted successor transitions. See M-1. |

## Findings

### Blocking

#### B-1 — Pending cleanup is not integrated into fd-family alias accounting or teardown

After service adoption fails, the payload has no `AllocationKey`, yet it still contains both its cleanup right and `Option<Rc<Device>>` ([drm_cleanup.rs:546](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:546)). A failed cleanup restores the right and deliberately retains the device alias ([drm_cleanup.rs:608](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:608)). Existing payload-alias accounting is keyed exclusively by an adopted `AllocationKey` ([drm_cleanup.rs:310](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:310)).

Revision 3 says only that the registry retains this payload until retry or “the existing teardown barrier takes ownership” ([target lines 194–204](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:194)). It defines neither a counted alias for the keyless payload nor the consuming transition that freezes it, transfers it into the incarnation bundle, marks its right closed after family closure, and drops the alias without stale-handle ioctls. Once frozen, ordinary `consume` retries cannot succeed ([drm_cleanup.rs:326](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:326)).

Concrete failure: adoption fails; `RMFB` fails; the registry retains the payload; teardown freezes the registry; the unregistered `Rc<Device>` survives outside the barrier’s alias set. The barrier can then falsely certify closure or can never complete. This violates the parent’s fallible handback rule ([authoritative lines 185–191](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:185)) and the binding requirement that pending cleanup and every fd-family alias move together without hidden references ([2c-i lines 242–269](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:242)).

Required correction: give each pending entry an incarnation-counted alias independent of `AllocationKey`, and specify its freeze, supervisor handoff, family-closed disposition, right transition, device-alias release, and `Preparing`-charge release. Test cleanup failure followed by freeze/handoff/barrier, not only a later successful retry.

### Major

#### M-1 — The designated count owner cannot observe all direct-lease creation and release

Section 2.3 assigns the per-`AllocationKey` count to “the commit consumer, which already sees every direct-role transition” ([target lines 120–132](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:120)). That premise is false.

The allocation lease and index are created during `Preparing`, before any commit event ([target lines 95–104, 177–184](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:95)). Successor replacement and pre-submit refusal can release it without dispatch. The consumer excerpt handles commit events such as completion, rejection and quarantine ([commit.rs:285](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:285)); unflip explicitly terminalizes a queued successor before manipulating consumer-owned records ([backend.rs:21164](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21164)).

Concrete failure: candidate K is adopted and indexed, then replaced while still `Successor`. No commit event reports either lease transition, so the count cannot perform K’s `1 → 0` removal. Conversely, if only committed leases are counted, retiring Current can remove the index while an unseen Successor still holds K. Live-token checking prevents stale aliasing, but the binding “remove when its position retires” contract and same-source reuse required for P3-3 fail ([2c-i lines 425–431](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:425); [authoritative lines 520–538](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:520)).

Required correction: place counting at the authoritative lease mint/clone/drop boundary, or define explicit producer-to-consumer events for every Preparing, Successor, undo, refusal and retirement transition. Extend evidence to a never-dispatched allocation’s `1 → 0`, not merely Current/Successor `2 → 1`.

#### M-2 — Rejected unflip has no `ExitRetirement → Current` rollback contract

The plan requires unflip to move the direct allocation into `ExitRetirement` ([target lines 216–221, 278](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:216)), but its rejection row only says `ResourcesStillCurrent` returns the old resources ([target line 235](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:235)). Baseline unflip moves the complete resource record from `Current` to `ExitRetirement` ([backend.rs:21168](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21168)), while `ResourcesStillCurrent` merely appends returned resources without restoring their role ([commit.rs:358](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:358)).

Thus a rejected unflip can leave the still-scanned framebuffer labelled `ExitRetirement`, with `Current` vacant and exit capacity permanently occupied. That contradicts the role meanings ([2c-i lines 400–407](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:400)) and the authoritative unflip contract ([authoritative lines 481–494](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:481)).

Required correction: specify and test rejection restoring `ExitRetirement → Current` and preserving its count/index. Define separately that success retires it only after release proof, while unknown/quarantine retains it under the teardown owner.

### Minor

None.

## Coverage and implementation checks

- Incorporation: all three round-2 findings audited.
- Architecture: checked adoption/count producers, commit-consumer reachability, unflip movement, and rejection return.
- Safety: checked present-pin refcounting, relayout/FreePixmap retention, R3 retry, hidden device aliases, freeze and teardown handoff.
- Compliance/evidence: checked authoritative §§3.2, 5.4 and 6.1–6.4; C.0 §§10.2/10.4; 2c-i resource, capacity and teardown rules; debt §§9.2/9.5.

**Excerpts used: 24/24.** DRI3 same-XID re-import/client-disconnect behavior, the complete family-closure minting implementation, the actual owner-unflip dispatch glue, and downstream successful exit-retirement release were not reopened; they are unassessed, not sound.

Builds, tests, formatting, clippy, portability, Rust borrowing and exact API shapes remain deferred to implementation.