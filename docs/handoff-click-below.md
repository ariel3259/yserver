# Handoff: click-lands-on-window-below (continue on bee)

Date: 2026-06-18. Cross-machine handoff (Claude's local memory does not travel
between machines — this doc does, via git). Continue on bee.

## Status

- **Focus fix (wezterm sloppy-focus) is DONE + HW-verified**, was squash-merged
  to master, then **REVERTED (`HEAD^1`)** because of the bug below. The focus
  commits live on branch **`fix/focus`**. `master` = clean baseline (no focus
  fix).
- Focus fix root cause, for reference: XI2 crossing delivery only checked
  `{deepest-hit, top-level}` windows for client selections; it missed muffin's
  `XI_Enter` selection on an *intermediate* client window (wezterm reparents its
  content into a child, so the deepest hit is below the window muffin watches).
  Fix = deliver XI2 crossings per producer chain-window instead of collapsing to
  the deepest. Files: `crates/yserver-core/src/core_loop/pointer_fanout.rs`
  (the crossing branch around `compute_xi2_targets`) + the producer change in
  `crates/yserver/src/kms/v2/backend.rs` (`resource_pointer_host_xid`).

## The open bug

Click pamac's close button → the click lands on a **firefox extension
UNDERNEATH** it. Deterministic, reproduces "in the first minute". This is the
same symptom that got #34 reverted: "focus on one window, click on the window
below."

## DECISIVE TEST — do this first

**Does click-below reproduce on the REVERTED master (no focus fix)?**

- **YES** → it's pre-existing (the #34-revert bug, still in baseline). The focus
  fix is exonerated and can go back in; click-below is its own separate hunt.
- **NO** (revert actually fixed it) → focus fix is implicated — but note it does
  **not** touch click *landing*: `root_pointer_target_at` (core resource-tree
  resolver) and XI2 *button* delivery are unchanged; the fix only changed
  *crossing* (Enter/Leave) delivery + the click-producer's grab `host_xid`. So a
  YES-on-`fix/focus` / NO-on-`master` result means something subtle is missed —
  trace hard before believing it.

Prior (state it, then prove it): expected result is **YES on both** =
pre-existing.

## Where click-below lives + how to split it

Click landing is decided by the core resource-tree hit-test:
`root_pointer_target_at` → `hit_test_children` (+ `window_input_contains`) in
`crates/yserver-core/src/server.rs`. Two candidates; a trace splits them:

1. **Stale stacking** — `hit_test_children` walks `children` top-to-bottom
   (`.iter().rev()`) and picks firefox because the resource tree thinks it's on
   top. Restacks *via `ConfigureWindow`* ARE tracked (`resources.rs`
   `restack_window` reorders `children`). So if it's stacking, suspect a restack
   path that does NOT go through `ConfigureWindow` (e.g. an override-redirect
   popup's ordering, or a raise mechanism we don't track).
2. **Input-shape** — pamac is a GTK CSD window. If its input shape makes the
   close-button region click-through, the hit falls to firefox below.
   (`window_input_contains` over `shape_windows`.)

**Trace the bad click:** `RUST_LOG=warn,yserver::kms::v2::pointer=trace`, click
pamac's close button, see which window `root_pointer_target_at` resolves vs
what's visually on top. If it resolves firefox while pamac is visually on top →
stacking (check resource children order). If it resolves nothing/falls through
at pamac's button → input-shape.

## Gotcha that cost a day — do not re-fall for it

**x11trace races chromium.** A failure that reproduces ONLY under x11trace
(especially chromium's keyring prompt / startup) is very likely a tracer
artifact, not a yserver bug — the proxy perturbs timing. It also silently
confounds bisects/A-B tests (hold the tracer constant, or test without it). An
entire "keyring regression" investigation this session was burned on a phantom
that was purely x11trace. For chromium timing, prefer `RUST_LOG` yserver-side
tracing over x11trace.

## Branch / git state (on silence)

- `master` = clean baseline (focus fix reverted via `HEAD^1`, squash merge so it
  was one commit).
- `fix/focus` = the focus fix (salvaged). **Push this + this doc so bee has it.**
