# Handoff: icewm panel tooltips never disappear (crossing churn)

Date: 2026-07-02 (late night). Machine: `air` (M1 Asahi), yserver on KMS via startx.
Branch with the related-but-separate fixes: `fix/passive-grab-activating-press-on-grab-window` (pushed).

## RESOLVED 2026-07-02 (HW-confirmed: air + silence)

Root cause was NOT resolver churn. It was **core-crossing delivery**:
`pointer_event_fanout_to_state` resolved crossing (Enter/Leave) events to the
**live deepest hit** (`resolve_pointer_hit` → `live_hit()`, pointer_fanout.rs)
for the WHOLE chain. On A→B sibling motion the producer emits `Leave(A)` +
`Enter(B)`, each stamped with its own window's `host_xid`, but delivery
collapsed both onto B (the destination / deepest hit). The widget being LEFT
(A, a sibling off B's propagation path) never received its `LeaveNotify`, so
icewm — which hides its per-widget tooltip on ANY LeaveNotify — orphaned it.

The "114 same-window Leave→Enter pairs" the churn analysis (below) measured were
the SYMPTOM of this collapse (B gets both a Leave and an Enter), not a resolve
flap. Proof: matched the pointer-trace log to `icewm.xtrace` — the `ef→f2`
transition (`Leave host=0x4000ef` + `Enter host=0x4000f2`) hit the wire as
`Leave event=0x1000fd` + `Enter event=0x1000fd` (both = destination `f2`), with
NO `Leave event=0x1000f9` for the widget left behind.

Fix (`crates/yserver-core/src/core_loop/pointer_fanout.rs`): route each core
crossing from its OWN window (`host_xid → resource`), mirroring the XI2 path
(which already did this, `:1003`). Also aligned the XI2-shadows-core dedup to
the same per-window target so a dual-selecting client can't get a double.
Regression test: `normal_crossing_leave_delivers_on_left_sibling_not_live_hit`.

Latent follow-ups left untouched (not triggered by icewm, no repro): (1) the
producer's `unwrap_or(new_xid)` in `update_pointer_window` (backend.rs) —
misattributes an intermediate crossing to the destination if that intermediate
lacks a `host_xid`; observed traces had host_xids on every intermediate. (2)
Core crossings still walk UP to an ancestor when the exact window doesn't select
the mask — pre-existing, slightly non-spec, but icewm's widgets all select
Leave so it never fires.

--- original investigation below (kept for the trace-analysis method) ---

## TL;DR

icewm panel tooltips appear on hover but **never disappear**. Root cause is a
**pre-existing bug in yserver's normal-motion pointer-window resolution**: it
emits large numbers of **spurious same-window `Leave(W) → Enter(W)` crossing
pairs**. Measured on one icewm session:

- yserver: **114** same-window `Leave→Enter` pairs
- Xorg (same session, same actions): **2**

icewm's `YWindow::handleCrossing` shows a tooltip on ANY `EnterNotify` with
`mode==NotifyNormal` and hides it on ANY `LeaveNotify`. The churn races its
tooltip lifecycle and orphans some tooltip windows (created + mapped, never
destroyed). Confirmed by user: reproduces with **no clicks at all** (pure
hover), so it is NOT the grab path.

This is NOT fixed yet. The two commits on the branch fix a *different*,
real bug (see below) and must not be confused with this one.

## What the two branch commits DO (and do not) fix

1. `fix(input): report sync passive-grab activating press on the grab window`
   — THE verified fix for the original "icewm panel buttons don't work at all".
   icewm frames its taskbar with a sync AnyModifier owner_events grab on the
   client container; the activating press must land on the grab window (Xorg
   moves the sprite up to it), not the deepest widget, so
   `YClientContainer::handleButton` runs and calls `XAllowEvents(ReplayPointer)`.
   Verified working on HW (panel buttons click).

2. `fix(input): emit NotifyGrab/NotifyUngrab crossings for passive grabs`
   — emits the grab crossing chains on passive-grab activate/deactivate (Xorg
   parity; explicit grabs already did this). Fixes the **click**-dismiss of a
   tooltip, i.e. clicking a panel item with a live tooltip now delivers a
   grab-Leave to the widget. Does **NOT** fix the hover churn. Kept on the
   branch partly to see whether it moves an XTS test — watch for that. Safe to
   drop this commit (`git rebase -i`/`git revert`) if it regresses anything; it
   is independent of commit 1.

## Evidence / how to reproduce the analysis

Traces captured (may be overwritten — regenerate if stale):
- `icewm.xtrace` — wire trace (icewm ↔ yserver), full protocol.
- `icewm-xorg.xtrace` — same session under real Xorg (the reference).
- `yserver-hw-awesome.log` — yserver debug log with `RUST_LOG=yserver::kms::v2::pointer=trace` (the `upw:` lines). NOTE: filename says "awesome" but it was the icewm run (recipe reuses the name).

Key commands used:

```
# spurious same-window Leave->Enter pairs, yserver vs xorg
for f in icewm.xtrace icewm-xorg.xtrace; do
  echo -n "$f: "
  grep -E "(Enter|Leave)Notify\([0-9]\).*mode=Normal" "$f" \
    | grep -oE "(Enter|Leave)Notify\([0-9]\).*event=0x[0-9a-f]+ child=None" \
    | grep -oE "(Enter|Leave)Notify|event=0x[0-9a-f]+" | paste - - \
    | awk '{print $1,$2}' | uniq -c \
    | awk 'pw==$3 && pk=="LeaveNotify" && $2=="EnterNotify"{n++}{pk=$2;pw=$3}END{print n+0}'
done
# yserver: 114, xorg: 2

# tooltip windows created but never destroyed (wire trace)
#   signature: override-redirect=true save-under=true event-mask=0
```

Representative churn (from `icewm.xtrace`, all at the SAME root position, so a
re-resolution not real motion):

```
LeaveNotify detail=Nonlinear event=0x001000a4 child=None root=(242,1581)
EnterNotify detail=Nonlinear event=0x001000a4 child=None root=(242,1581)
```

`detail=Nonlinear` ⇒ the pointer window flapped to a *different-subtree* window
and back — the prime suspect is the override-redirect tooltip window (child of
root, mapped on top, overlapping the panel area) being included in the deepest-
window hit test, and/or an unstable re-resolution on window map/restack.

## Where the bug lives

KMS backend v2 pointer resolution:
`crates/yserver/src/kms/v2/backend.rs`

- `update_pointer_window` (~line 6430): computes `from`/`to` nested windows,
  calls `crossings::normal_mode_crossings`, then round-trips each event through
  a **host_xid** via `emit_crossing`. The fallback `unwrap_or(new_xid)` (~6473)
  misattributes crossings for windows without host_xids (a secondary bug —
  intermediate/virtual events get stamped onto the destination widget).
- `resource_pointer_host_xid` (~6222): resolves the deepest window under the
  cursor, walking up to the nearest host-backed ancestor.
- `dispatch_motion_event` (~6505) is the only caller of `update_pointer_window`.

Contrast: the grab-crossing path (`emit_core_pointer_grab_chain` in
`process_request.rs`) delivers by **nested ResourceId directly** and is correct.
The normal-motion path is the one that churns.

## Hypotheses to test (in order)

1. **Override-redirect tooltip in the hit test.** Does `root_pointer_target_at`
   return the tooltip window when the pointer is near/under it, then the widget
   again on the next sample, flapping? Check whether the 114 pairs correlate
   with tooltip windows being mapped/positioned over the panel. If so, the fix
   is about hit-test stability, not the host_xid round-trip.
2. **Re-resolution on stacking changes.** The pairs at identical positions
   suggest `update_pointer_window` (or something upstream) re-runs on map/
   restack/configure and emits Leave+Enter even though the sprite window did not
   actually change. Find every path that re-resolves the pointer window without
   real motion; ensure a stack change under a stationary pointer only emits
   crossings if the deepest window genuinely changed.
3. **host_xid round-trip misattribution.** Even if 1/2 are the churn cause,
   `emit_crossing`'s `unwrap_or(new_xid)` is wrong for windows without host_xids
   (icewm's frame/container). Consider routing normal-motion crossings through
   nested-window delivery like `emit_core_pointer_grab_chain` does, instead of
   host_xid.

## Suggested next-session plan

1. Capture ONE run with BOTH the wire xtrace AND the pointer-trace log
   (`RUST_LOG=yserver::kms::v2::pointer=trace`), and note which panel item was
   hovered. The runs analysed so far were separate, which blocked exact
   correlation.
2. Pin the re-resolution trigger for one spurious `Leave(W)+Enter(W)` pair
   (map? restack? duplicate motion sample?).
3. Fix at the smallest correct layer; add a unit test in `crossings.rs` or a
   backend test asserting no self-crossing when the deepest window is unchanged.
4. Re-run icewm on HW: hover several panel items without clicking → tooltips
   must dismiss when moving away. Cross-check `awesome`.

## Memory

See `~/.claude/.../memory/yserver_passive_grab_sprite_moves_to_grab_window.md`
for the full three-bug writeup (delivery fix, grab-crossings fix, and this
open crossing-churn bug).
