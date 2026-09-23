# Ciii: direct entry does not require a composed return

**Date:** 2026-09-23. **Found by:** the coordinator, answering section 6 of the
stage 3a design (`docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`),
which codex review rounds 3–5 of that design kept reaching.

## The question

The Ciii Owner unflip is ready only when exit retirement is vacant, **a
composed return is established** and the direct shadow is materialized
(`render/admission.rs:1007`). On an Owner device the composed return is the
output's composed `OwnerBuffer` in state `Current` (`render/admission.rs:844`,
`scene.rs:1944`). If a direct unit can be current while no composed return
exists, a client that destroys its window leaves its last frame on screen
until something else establishes one — with or without DPMS.

## Case (b) — can the return be lost while direct is current? No path found

- The only transition out of `Current` for a composed buffer is its
  replacement by a newer composed buffer on the same output
  (`retire_owner_buffer`, `scene.rs:2470`; `into_releasing` at `:2511`). A
  direct commit never touches the composed owner buffers.
- A relayout on an Owner device (`admission_note_layout_change`,
  `render/admission.rs:748`) advances the layout generation and withdraws a
  queued direct successor; it does not touch composed owner buffers.
- `scene.drain_all` and `reset_scanout_bos_for_suspend` are Legacy DPMS/VT
  steps; stage 3a scopes them to Legacy devices with a mutation of its own.
  3b and 3c must keep the invariant when they convert modeset and VT.

## Case (a) — can direct be entered without one? Yes, nothing prevents it

- Direct readiness (`render/admission.rs:950`–`970`) checks eligibility,
  producer readiness and ordinary retirement — **not** the composed return.
- Direct eligibility (`direct_present_eligibility_decision`,
  `backend.rs:833`) checks CRTC eligibility, seat, outputs, cursor, root
  overlay, authoritative root, borders and offsets — **not** the composed
  return.
- What makes the case unlikely is only
  `SCANOUT_M2_ELIGIBLE_ROOT_PROBATION` (8 stable eligible Presents before
  entry), during which the root is normally composed. That is a probability,
  not a guarantee: an Owner device whose first composed frame never reached
  `Current` before probation ends can enter direct with no way back.

## Ownership and proposed fix

The code is Ciii's (accepted 2026-09-22), so by the project's rule — if it
affects us, it is ours — it is fixed by us, in its own commit, named as a stage
2c-iii addendum, not absorbed into 3a. Proposed invariant: **direct entry
(no direct unit current) is admissible only when every affected output has an
established composed return**; otherwise the direct intent waits with
`ComposedReturnNotEstablished`, the same reason the unflip uses. A direct
successor while direct is already current is unaffected (the return was
established at entry and case (b) keeps it). Evidence: a fixture that offers
direct entry on an Owner output with no composed `Current` and sees it wait,
then admitted once a composed frame retires; mutation: remove the check and
the fixture must fail.
