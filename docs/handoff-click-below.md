# Handoff: click-below / nemo-rises (cinnamon, sloppy focus)

Date: 2026-06-19 (silence). Branch: **fix/focus** (not pushed unless noted).
Supersedes the 2026-06-18 version of this file.

## Repro

Two overlapping file managers, **nautilus (GTK4, frameless) on top of nemo
(GTK3, framed)**, cinnamon **focus-mode = sloppy** (focus-follows-mouse).
Click inside nautilus (far from edge, no mouse move). Two symptoms historically:
1. the click lands in **nemo** (the window below) — "click-below";
2. **nemo rises to the top** after the click.

## What is FIXED (this branch, HW-confirmed)

**Click landing.** Symptom 1 is gone — clicks now land on the window that was on
top when you pressed. User-confirmed on HW ("the click didn't land in nemo").

Fix = `resolve_pointer_hit()` in
`crates/yserver-core/src/core_loop/pointer_fanout.rs`: for ButtonPress/Release the
target is **sampled at event generation** (descend from the producer-stamped
`host_xid`), not re-resolved live at delivery. A WM restack landing between the
press and its fanout was retargeting the in-flight click to the window raised on
top in between (X11/Xorg sample the sprite at event time). Both the delivery path
and `try_match_passive_grab` use the helper. Regression test:
`button_target_locked_at_generation_survives_restack`. Green: yserver-core 820,
yserver 521, nightly fmt + clippy clean.

Also on this branch (cherry-picked from `fix/pointer-producer-resource-authority`,
commits 4db110d3 + fa27e430): producer resolves via core tree + XI2 crossings
per chain-window. Keep.

## What REMAINS (the real bug)

**nemo still rises** after clicking nautilus. Mechanism, evidenced on muffin's
own wire (`cinnamon.xtrace`, x11trace run):

```
029:> XI2 Leave  event=nautilus  root=(667,463)
029:> XI2 Enter  event=nemo      root=(667,463)   ← same position
029:< XIQueryPointer → child = nemo's frame
029:< SetInputFocus focus=nemo                    ← muffin focuses nemo
029:< ConfigureWindow nemo-frame AboveSibling(nautilus)   ← raises nemo
```

muffin is in **sloppy focus**, so an `Enter(nemo)` = focus + raise. Once muffin
raises nemo, the whole tree (one source now) follows it to nemo.

**IMPORTANT framing correction (don't repeat the earlier mistake):** this is
NOT a render-vs-input divergence. Yesterday's Step 1-2 + the combine DID collapse
render and hit-test onto the core tree (render walks `top_level_order` synced from
core children; hit-test reads core children). In the HW run, the clicks resolved
nautilus AND the pre-click scanout showed nautilus — render and input **agreed**.
So it is **one source**, behaving consistently. The bug is the **focus loop**:
muffin focuses+raises nemo, and everything correctly follows.

The open question is the **SEED**: the first moment muffin focuses nemo when it
should keep nautilus. Under sloppy focus muffin focuses the window under the
pointer; somewhere yserver hands muffin an `Enter(nemo)` (or a pointer-window
answer of nemo) while nautilus is the one the user raised. Find that first
intrusive `SetInputFocus(nemo)` in `cinnamon.xtrace` and the event muffin
received right before it.

## Ruled out

- **Restack storm** (Nautilus helper window restacked ~84×/run): NOT the cause.
  Xorg storms the same window 42×/run and works fine.
- **Render-vs-input divergence**: NOT it (see correction above; one source now).

## Also spotted (separate, same area)

The COW (`0x103`, cinnamon overlay) sometimes **captures clicks** when its input
shape is `region=none` → hit stack shows `0x103 … input_shape=none shape_ok=true
<== FIRST HIT`, click delivered to the cinnamon stage (c27). Known-class
(region=0 vs empty input shape). Tangled into the same focus area; may be a
contributor to the seed.

## Next steps

1. Find the **seed**: in `cinnamon.xtrace` (15:55/16:02 run, kept in repo root),
   the first intrusive `SetInputFocus(nemo)` by muffin (conn 029) and the event
   immediately before it. (Last look was mid-loop; the first one is upstream.)
2. If the seed is an `Enter(nemo)` while nautilus is on top → why does yserver's
   crossing producer resolve nemo there (input shape? transient stacking?).
3. Bigger picture: per
   `docs/superpowers/findings/2026-06-18-pointer-stacking-dual-authority-diagnosis.md`,
   the durable fix is converging ALL pointer/focus consumers onto one order;
   point-fixes keep relocating the symptom (landing fix is an example — fixed
   landing, raise remains). Scope before swinging (prior unify attempts HW-failed).

## Tooling

- `just yserver-cinnamon-hw` (logs clickhit/restack/focus + scene) for the
  yserver-side view; `just yserver-cinnamon-hw-trace` writes `cinnamon.xtrace`
  (muffin's full wire) — the decisive artifact for "what muffin reacts to".
- Scanout dump: **Ctrl-Alt-F12** (also dumps scanout) → `yserver-v2-scanout-*.ppm`
  (run 0 = first dump). Take it BEFORE the click for ground-truth top window.
- Diagnostic traces added this session: `yserver::input::focus` (FOCUS-EMIT +
  SetInputFocus) and `CONFIGURE-REQ` (with stack_mode/sibling) on the `restack`
  target — both in `process_request.rs`.
- Xorg arbiter captured: `cinnamon-xorg.xtrace` (repo root) — tangled (many
  nemo/nautilus instances), hard to grep-align to a single click.
