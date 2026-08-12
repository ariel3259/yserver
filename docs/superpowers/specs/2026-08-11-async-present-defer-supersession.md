# Async Present Defer + Supersession Spec (delta)

Date: 2026-08-11. Status: **Approved for implementation.**
Related findings: `docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md`
(§4a, §4b). Supersedes the "async collapses the due rule to always-now"
decision recorded in `present_scheduler.rs` for the async (`PresentOptionAsync`)
case only.

## Problem

A fullscreen game without vsync (CS2 on RADV) presents uncapped (~200-300/s,
`PresentPixmapSynced` + `PresentOptionAsync`). Because async presents get
`effective_target_msc = None`:

- `classify_msc_due(None, _, _)` returns `ExecuteNow` always
  (present_scheduler.rs:94-95) → no parking.
- `supersede_covered_pending_presents` early-returns on `effective_target_msc ==
  None` (process_request.rs:8810-8812) → no supersession.

Result: every present executes immediately, marks full damage, re-composes on
the same GPU the game renders into → `page_flip/s` collapses below refresh
(60 → 27-47 Hz observed). Xorg's present scrap logic coalesces such floods; we
do not.

## Design

Two changes, both restricted to async (`eff = None`) presents. Synced-present
behavior is unchanged.

### 1. `classify_msc_due` — park async presents while a flip is in flight

```
classify_msc_due(eff=None, clock_msc, flip_in_flight):
    return Park      if flip_in_flight
    return ExecuteNow otherwise
```

Rationale: an async present cannot flip before the current in-flight flip
retires (KMS allows one flip in flight per CRTC). Parking to the next vblank
adds no latency vs today (ExecuteNow also waits for the flip), but makes the
flood coalesceable. This is Xorg `present_scmd` scrap behavior for free-running
clients.

### 2. `supersede_covered_pending_presents` — allow async successors to scrap parked async predecessors

Replace the `let Some(target) = successor.effective_target_msc else { return }`
early-return with a same-group rule keyed on the **async option bit**
(`masked_options & PRESENT_ALL_ASYNC_OPTIONS != 0`), NOT on `eff`:

- async successor → may scrap parked entries for the same window whose
  `masked_options & PRESENT_ALL_ASYNC_OPTIONS != 0` (both async);
- synced successor → unchanged (same-target rule only);
- an async successor never scraps a synced predecessor and vice-versa (they are
  different groups — a synced present is scheduled against an explicit target).

Rationale for keying on the option bit: in no-clock environments
(nested/headless/pre-first-flip KMS) synced presents also carry
`effective_target_msc = None`; keying the group on `eff` alone would let an async
successor scrap a source-not-ready synced predecessor there, violating the
"synced unchanged" constraint.

The coverage predicate `present_supersession_covers` (including the
`successor_presents_full_extent` successor gate) is unchanged and still applies
to both groups. A superseded async victim parks a `CompleteNotify{Skip}` with
`effective_target_msc = 0` (immediate delivery); a superseded synced victim keeps
its target gate.

## Out of scope

- Direct scanout for fullscreen games (separate efficiency work — the
  scanout-m1/m2 gate relaxations are in the companion plan, which also adds
  direct-level latest-wins supersession). This spec is the PRIMARY flood fix.
- GetImage / MIT-SHM readback latency (separate).

## Acceptance

1. A no-vsync fullscreen client flooding ~200-1000 async presents/s keeps
   `page_flip/s` at refresh (measured via `YSERVER_LOOP_TELEMETRY=1`).
2. Async presents that are superseded deliver `CompleteNotify{Skip}` in
   present_id order (existing per-window ordered-delivery machinery). Note: the
   order guarantee holds for a **pure-async** flood (the CS2 case); a mixed
   sync/async window can still see an async completion overtake a held-back
   synced entry — that is pre-existing round-4 F6 "async outside hold-back by
   design" behavior, not introduced by this spec.
3. Synced (vsync-on) presents are bit-for-bit unaffected: unit tests for
   `classify_msc_due` and the supersession group rule pass unchanged.
4. CI: `cargo clippy --all-targets -- -D warnings` clean.
