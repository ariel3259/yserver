# Phase C.0 addendum: the Owner route honours the direct hold

**Status:** revision 3 (review round 1: M-1 protected set, M-2 lifecycle
handoffs, M-3 test preconditions; round 2: M-1 the invariant's two ownership
states, M-2 per-variant mutation reachability). Direction approved by the user 2026-10-03:
Legacy parity, not a symptom fix. A production defect of the Owner composed admission (stage 3b)
against the direct scanout state (stage 2c Ciii, M2), found by the load
criterion through `c0_merge_unmap_direct_window_unflips_vulkan`.

**Related specifications:**

- `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
- `docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md` (Ciii)
- `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`

## The defect

Two loaded traces (codex, 2026-10-03) show the same sequence on one CRTC: a
direct Present commit is accepted, presented and retired; M2 records it as
`scanout_m2.current`. The retirement wake then runs admission with
`current_direct = true`, no unflip requested, and a composed generation ready.
Admission chooses `Tier::Primary / Composed` and dispatches an ordinary
FastUpdate commit, which takes the CRTC's current resources. When it retires,
`current_resources` holds the composed commit and no `DirectRole::Current`,
while M2 still tracks the direct frame: the window image stays pinned, and the
next Ciii unflip finds no direct resource to retire (`PreparationRefused`).

Cause: `admission.rs` computes `current_direct` but applies it only to direct
successor readiness; composed readiness looks at output power and composed
readiness alone, and the decider admits any ready composed candidate. Neither
the Owner retirement routing (`retire_owner_unflip` needs a recorded unflip
commit) nor the M2 completion match (`scanout_m2.pending` only) covers an
ordinary composed commit.

It is latent in production today (Owner is not activated) and would bite at
activation: a fullscreen client in direct scanout with anything repainting
behind it (a panel clock) would get composed frames over the direct frame.

## What Legacy does

`scanout_m2.hold_direct` is set when a direct frame is pending or queued and
is cleared only by `request_direct_unflip` or `stop_direct_after_scanout_replaced`.
The composite tick returns without composing while
`hold_direct && !unflip_requested`, or while a direct transaction or an unflip
is still in flight. A composed image replaces a direct frame only through the
unflip (atomic, or degraded per-output flips tracked by
`unflip_awaiting_outputs`), and `retire_direct_output` clears M2 when that
replacement has retired on every output. Damage behind a held direct frame
accumulates; it never causes an unflip by itself.

## Rules for the Owner route

0. **The protected set.** A direct frame is a *grouped* frame: the existing
   path accepts it only for the homogeneous single-device topology
   (`direct_scanout_topology_eligible`) and installs its framebuffer on every
   output of that device; the Present CRTC is only its pacing identity. The
   protected set is therefore every CRTC of the device that M2's direct
   ownership covers — the pending frame, the current frame, and a queued
   successor all protect the same complete set. It is derived from M2's direct
   ownership and the device topology, never from the candidate's CRTC, and is
   used identically for readiness, bundle selection, maintenance absorption
   and the invariant. Direct ownership of only some CRTCs of a multi-CRTC
   device does not exist today and stays refused by this addendum.
1. **No ordinary composed commit over a direct frame.** While M2 holds a
   pending, current or queued direct frame and no unflip is requested, a
   composed primary candidate for any CRTC of the protected set is not
   admissible: its readiness is `Waiting` with a reason naming the direct
   hold. This holds for every admission entry (tick, retirement wake,
   readiness wake). A maintenance absorption must not carry a protected
   composed primary either; maintenance commits that do not replace the
   primary plane keep progressing. Damage keeps accumulating in the scene.
2. **Replacement paths.** On a live, healthy incarnation the ordinary
   replacement of a direct frame by composed content is the Ciii unflip only,
   and M2 is cleared when it retires (unchanged). The owner-ordered lifecycle
   and topology paths keep their existing handoffs and are not routed through
   Ciii: VT release (`ACTIVE=0`), VT acquire reinstall (which already calls
   `stop_direct_after_scanout_replaced`), DPMS transitions, a client modeset,
   and device removal / quarantine. This addendum changes none of them and
   adds no early release: every one still releases the direct frame only on
   its existing proof (retirement, reinstall, or quarantine), and a logical
   withdrawal is not proof that scanout stopped.
3. **No new unflip triggers.** This addendum adds none; the existing reasons
   (`request_direct_unflip` callers) stay the only ones. Scene damage behind a
   held direct frame must not request an unflip.
4. **Invariant.** On a live, healthy incarnation with no lifecycle
   transition, no owner-ordered modeset and no Ciii unflip in flight on the
   device, at every core-loop iteration boundary, M2's direct ownership is in
   exactly one of two states, and the commit ledger matches it:
   - **Stable:** M2 holds a current frame D0 and no direct successor commit
     is in flight. `current_resources` holds D0's `DirectRole::Current`
     resources for the whole protected set.
   - **Direct replacement in flight:** M2 holds D0 current and a pending
     successor D1 whose commit is dispatched. D0's resources are that
     commit's retained old-state dependencies (taken out of
     `current_resources` by the direct dispatch) and D1's resources are its
     new state, for the same protected set; when D1 retires, D1 becomes
     Stable. The identities are exact (D0's and D1's commit and source
     generations), so an ordinary composed commit in flight on the protected
     set can never satisfy this state: that is the defect this addendum
     forbids.
   During the excluded transitions the invariant is suspended, and each
   transition's own existing proof decides when the direct frame is released.

The rejected alternative (making the Ciii unflip accept composed resources as
the direct retirement, or routing every composed replacement through Ciii) is
recorded so it is not proposed again: the first corrupts the retirement
accounting, the second would leave direct scanout on every background repaint.

## Tests

Each through the existing core-loop driver (B), ending with the end-state check
(A), load-clean under criterion F, with a mutation that must fail.

**Precondition for every rule-1 test (M-3):** the mutation is meaningful only if
a genuinely ready composed offer reaches admission. The traces show how it
arises in production: a composed generation is prepared and offered before the
direct frame is held, and is still offered when the direct retirement wake runs
admission. The fixture establishes that offer through production entries, and
the test first proves its readiness independently of the new gate (the
composed producer reports it `Ready`); only then is the gate's `Waiting`
meaningful, and the mutation (gate removed) must cross the dispatch boundary
(a composed commit is dispatched).

- `c0_merge_unmap_direct_window_unflips_vulkan` (existing): its
  `current direct Owner allocation` assertion holds. Mutation: drop rule 1 —
  it fails again under the full-sweep load.
- Held direct frame + ready composed offer + retirement wake: no composed
  commit is dispatched, the direct frame stays M2-current with its
  `DirectRole::Current`, the composed damage is still pending. Mutation: drop
  rule 1 — a composed commit is dispatched.
- **Pending direct frame — ownership characterization, not a dispatch
  mutation.** While a submitted direct frame is not yet presented the device
  slot is occupied and `admission_wake` returns `SlotBusy` before readiness is
  evaluated, so no composed commit can be dispatched in that interval with or
  without rule 1. The test checks the protected set and the ownership state
  (Direct replacement in flight, rule 4) across the interval; the dispatch
  mutation evidence for this frame is the post-retirement case above. The
  occupied slot is never bypassed to manufacture a witness.
- **Queued successor:** the queued direct successor is held in a production
  waiting state (its source not yet ready), so it cannot win the retirement
  admission and mask the composed candidate; with the gate removed, the
  ready composed offer is dispatched (mutation), with the gate it waits.
- **The complete protected set:** a device with two outputs; the direct frame
  is paced on A; a ready composed offer on B is held too. Mutation: derive the
  set from the candidate's CRTC — B's composed commit is dispatched.
- **Unflip with a failing shadow, then a production retry:** an unflip is
  requested while a composed offer is ready; its shadow materialization first
  fails; no ordinary composed commit is dispatched while it waits; the
  production retry dispatches the Ciii unflip and M2 clears only when it
  retires. Mutation: admit the ordinary candidate while the unflip waits — it
  fails.
- **Progress while ordinary composition is blocked:** a maintenance commit
  that does not replace the primary plane, and a VT release, both proceed
  while a direct frame is held. Mutation: block them under rule 1 — they stall.
- **Rule 4 at every iteration:** checked through the driver's per-iteration
  observation in each test above (not only at the end), then the end-state
  check.

Hardware (C, coordinator): `c0_hw_3b_modeset_owner_on_card1_drm` and the
direct-scanout hardware tests that exist on card1, before the commit.
