# Per-output direct scanout

## Intent

A fullscreen client on one monitor of a multi-monitor setup is scanned out directly on that monitor, while the other monitors keep composing at their own rate. The user's case: a game fullscreen on the main monitor (MSI, 48–240 Hz, DisplayPort) and Discord or a browser, sometimes fullscreen, on the secondary monitor (LG, FreeSync 60–75 Hz). Today, with both monitors on, nothing is ever scanned out directly, so the game is always composited (one copy per frame) and never gets VRR.

**Failure, as the user defines it:** the main monitor losing direct scanout because of something that happens on the other monitor, or direct scanout not being per monitor at all. VRR without direct scanout does not count as success.

This spec is PR A (point 4 of the VRR order agreed on 2026-10-20). PR B (point 3, VRR with several monitors on) builds on it and is out of scope here (see Non-goals).

## Current state (measured in the code, master 3ac369ee)

- **Direct scanout is root-wide.** `ScanoutM2State` (`kms/state.rs:261`) is one state for the whole screen. `submit_direct_frame` builds one plane state per entry of `platform.outputs` and submits them in one atomic transaction (`kms/scanout.rs` ~946); every CRTC shows its own region of the same root-sized client buffer.
- **Only `Root` coverage is admitted.** The Present classification already computes `ScanoutM0Coverage::Output(idx)` when the window rect and the source extent equal one output (`kms/scanout.rs` ~2729), but `scanout_m1_probe_eligible` (`kms/scanout.rs:249`) and the M1 probe (`maybe_probe_scanout_m1`, ~2249, which also requires the outputs to tile the root exactly) admit `Root` only. With two monitors the root is 3840×1080 and a game on one monitor is `Output`, so it never probes.
- **While direct, no output composes.** The compose pass returns early while a direct frame is pending, unflipping or held (`kms/scanout.rs` ~3579–3612). With today's root-wide direct that is correct; with a per-output direct frame it would freeze the other monitors.
- **Unflip triggers are root-wide.** For example, any top-level restack unflips an unredirected direct frame (`portable/windows.rs:1926`), so a restack of a window on the other monitor would drop the game out of direct scanout. There are about 30 distinct unflip reasons (cursor, shapes, overlay, stack, configure/unmap/destroy, authoritative root Present, CRTC transform, storage reallocation, successor failures).
- **VRR is already per CRTC.** `live_direct_vrr_wanted_by_output` and `direct_frame_wants_vrr` (`kms/scanout.rs` ~1100–1187) decide VRR per output from the direct frame that output shows. There is no explicit "one output" VRR limit: with two monitors on there is simply never a direct frame.
- **The M1 probe is cached.** The TEST_ONLY commit runs once per client buffer, cached by `source_id` and invalidated on topology change; entering direct also needs a run of 8 eligible Presents (`SCANOUT_M2_ELIGIBLE_ROOT_PROBATION`). Neither costs anything per frame once direct.

## Prior art (behaviour only)

- **Xorg** (MIT, `hw/xfree86/drivers/modesetting/present.c`): a window can flip only if it covers the entire X screen, one window at a time; VRR is set on every CRTC at once. Per-output direct scanout is therefore a capability Xorg does not have; there is no Xorg client contract for it beyond Present's ordinary completion semantics, which stay as today.
- **wlroots** (MIT, `types/scene/wlr_scene.c`): each output decides direct scanout on every frame from the list of what is visible on that output only (one entry, no colour transform → try the client buffer with a TEST_ONLY commit, else compose). Outputs are independent, so two outputs can be direct at once and a change on one output never affects another. yserver takes the *idea* (per-output decision, only what is visible on that output counts) but not the per-frame TEST_ONLY (yserver's probe is per buffer, cached) nor the stateless model (X11 Present needs the pending/current/unflip/fallback machinery for CompleteNotify, IdleNotify and a correct replacement image).

## Design

1. **Direct state per output.** The direct-scanout state becomes one instance per output (today's `ScanoutM2State` contents, unchanged in meaning: pending, queued successor, current, unflip request/reason, fallback, probation streak, cursor refresh bookkeeping). Each output enters, holds and leaves direct scanout on its own, with atomic commits on its own CRTC only.
2. **Coverage decides the outputs.**
   - `Output(i)`: the window rect and source extent equal output *i* → candidate for output *i* only.
   - `Root`: as today → candidate for every output, submitted as today's grouped transaction over all outputs, and retired/unflipped per output as today. On a single-monitor setup `Root` and `Output(0)` are the same window; behaviour there must be identical to today.
   - A `Root` candidate while some output holds an `Output` frame, or an `Output` candidate while a `Root` frame is held: the new candidate replaces what it overlaps through the normal unflip/entry path (no mixed state where one transaction mixes both).
3. **The M1 probe admits `Output` coverage.** The probe's TEST_ONLY commit for an `Output(i)` buffer tests only CRTC *i*'s primary plane with that buffer at source (0,0) and the output's size. The "outputs tile the root" requirement applies to `Root` only. The probe cache key includes the coverage, so the same buffer probed as `Root` and as `Output(i)` are distinct entries.
4. **Other outputs keep composing.** The compose pass skips only the outputs that are direct (pending, held or awaiting an unflip replacement on that output); every other output composes and flips as if no direct frame existed. A composed commit on output *j* never waits for, nor races with, a direct transaction on output *i ≠ j* (different CRTCs).
5. **Unflip triggers are scoped to what is visible on the direct output.** Each trigger asks whether the change can alter what output *i* shows:
   - restack, map/unmap, configure of a window other than the direct window: only if the window (its new or old extent) intersects output *i*'s rectangle and is above the direct window;
   - shape changes: only on the direct window's chain (as today) — the chain is the direct window's;
   - root overlay (software cursor sprites, overlay windows): only if it intersects output *i*;
   - authoritative Presents (`ineligible_authoritative_root_present`): a Present is authoritative for output *i* if it covers output *i*; a Present that covers another output only concerns that output;
   - per-CRTC causes (cursor commit failures, CRTC transform, gamma, DPMS) apply to that output;
   - global causes (RANDR topology change, storage reallocation, VT switch, server teardown) unflip every direct output, as today.
   The rule errs towards unflipping: when intersection cannot be established cheaply, the output unflips (today's behaviour).
6. **Present semantics unchanged.** CompleteNotify/IdleNotify for a direct frame use that frame's CRTC clock (`completion_output_idx`), as today; for an `Output(i)` frame that is CRTC *i*. MSC and UST for windows on composed outputs are unaffected.
7. **Cursor.** The direct-mode cursor rules (hardware cursor required, cursor-only commits while a direct frame is held, the VRR cursor deadline) apply per output: the cursor on a composed output behaves exactly as with no direct frame.
8. **VRR follows the per-output state with no new rule** (`live_direct_vrr_wanted_by_output` already reads per output). Making the VRR policy correct with several outputs on (composed outputs always carry `VRR_ENABLED=0`, the cursor deadline only on the VRR output) is PR B.
9. **Tearing** (async flips) stays bound to one direct frame: the tearing owner is the direct frame of one output, and its rules apply to that output's CRTC only. (Controller's call; a second tearing owner is not needed for the user's case.)

### Reuse

- The whole per-frame machinery of `ScanoutM2State` (entry probation, successor slot, composed/fresh unflip, fallback source, retirement by CRTC, pins) is reused per output; the change is where it lives and which outputs a transaction touches, not what it does.
- `ScanoutM0Coverage::Output(idx)` classification already exists.
- `direct_frame_slot_on_output`, the per-output awaiting sets and `completion_output_idx` already model per-CRTC retirement.
- The per-output compose path and pools already exist; only the early return changes.
- VRR's per-output decision already exists.

## Non-goals

- **PR B:** VRR policy and validation with several outputs on (cursor deadline per output, composed outputs never carrying VRR, MSI + LG hardware run with VRR).
- A window spanning parts of two outputs (neither `Root` nor `Output`): composed, as today.
- Mirrored or overlapping outputs, transformed outputs (page flipping stays off while any CRTC is transformed, as today), outputs whose mode size differs from their RANDR size.
- Overlay planes, scaling, a direct buffer smaller than the output.
- Two tearing owners at once.
- Any change to single-monitor behaviour.

## Must keep working

- Single monitor: everything as today (mpv/CS2 direct, VRR, tearing, Cinnamon unredirect, lock screen, notifications, COW cases fixed in #299).
- Two monitors with a window covering the whole root (`Root`): grouped direct as today.
- Present completion order and MSC per CRTC.
- Hot-plug / RANDR changes while an output is direct: clean unflip, no stale frame.

## To measure before the plan

- **Muffin with two monitors.** Whether muffin unredirects a fullscreen window that covers one of two monitors (the client then presents `Unredirected` with `Output` coverage), or keeps compositing it into its root-sized stage (then the game reaches yserver as a COW Present with `Root` coverage and this PR alone does not give it direct scanout under Cinnamon). Measured with the scanout debug summary (`output_coverage`, target) in a Cinnamon session with both monitors on; behaviour observed only, no muffin source read. Without a compositor (i3, bare) the client window is presented directly and is `Output`.

## Acceptance

- Backend tests (plan): per-output entry/exit, a Present or restack on output *j* leaves output *i*'s direct frame current, composed flips continue on *j* while *i* is direct, `Root` grouped behaviour unchanged, global causes unflip all.
- Hardware, NVIDIA card1, MSI (DP-3) + LG (HDMI-2) both on:
  - mpv fullscreen on the MSI under i3 or bare: direct on the MSI output only, ~100 flips/s on its CRTC, the LG keeps composing (a clock/video on it advances);
  - restacks, a playing video and Discord activity on the LG: no unflip on the MSI;
  - a notification over the game on the MSI: unflip on the MSI only;
  - single-monitor regression run (as #298's runs).
- AMD card0 if two outputs can be connected there.

## Provenance

Xorg (MIT) was read for its flip condition. wlroots (MIT, local checkout `~/Projects/wlroots` bd75ebfe) was read for its per-output direct-scanout behaviour; no wlroots code is copied, and the design keeps yserver's own state machine. No KWin, Mutter or Muffin (GPL) source was read; muffin's multi-monitor unredirect behaviour will be observed through yserver's own logs.
