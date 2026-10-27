# Per-output direct scanout

## Intent

A fullscreen client on one monitor of a multi-monitor setup is scanned out directly on that monitor, while the other monitors keep composing at their own rate. The user's case: a game fullscreen on the main monitor (MSI, 48–240 Hz, DisplayPort) and Discord or a browser, sometimes fullscreen, on the secondary monitor (LG, FreeSync 60–75 Hz). Today, with these two monitors on, nothing is ever scanned out directly, so the game is always composited (one copy per frame) and never gets VRR.

**Failure, as the user defines it:** the main monitor losing direct scanout because of something that happens on the other monitor, or direct scanout not being per monitor at all. VRR without direct scanout does not count as success.

This spec is PR A (point 4 of the VRR order agreed on 2026-10-20). PR B (point 3, VRR with several monitors on) builds on it and is out of scope here (see Non-goals).

## Current state (measured in the code, master 3ac369ee; revision 2 after the first review)

- **Direct scanout is root-wide.** `ScanoutM2State` (`kms/state.rs:261`) is one state for the whole screen. `submit_direct_frame` builds one plane state per entry of `platform.outputs` and submits them in one atomic transaction (`kms/scanout.rs` ~946); every CRTC shows its own region of the same root-sized client buffer.
- **Grouped direct needs a homogeneous topology.** `direct_scanout_topology_eligible` (`kms/scanout.rs` ~2211) requires every output on the primary DRM device and refresh rates within 0.5% of each other (one atomic request cannot cross devices, and grouping CRTCs of different refresh makes retirement wait for the slowest). MSI at 240 Hz plus LG at 75 Hz therefore never qualifies, even for a window covering the whole root. Two monitors at the same rate do get grouped `Root` direct today.
- **Only `Root` coverage is admitted.** The Present classification already computes `ScanoutM0Coverage::Output(idx)` when the window rect and the source extent equal one output (`kms/scanout.rs` ~2729), but `scanout_m1_probe_eligible` (`kms/scanout.rs:249`) and the M1 probe (`maybe_probe_scanout_m1`, ~2249, which also requires the outputs to tile the root exactly) admit `Root` only. With two monitors the root is 3840×1080 and a game on one monitor is `Output`, so it never probes.
- **While direct, no output composes** during a pending direct transaction or a steady hold (`kms/scanout.rs` ~3579–3612); the fresh/degraded unflip paths do compose while direct ownership is retained (~1901, ~3645). Admission also treats any pending scene page flip as a reason to fall back (`kms/present.rs` ~924), `scene_wants_compose` is suppressed while a direct/unflip transaction is pending (~3029), and the held-direct wakeup considers cursor work only (~3346). All of these are root-wide today.
- **Unflip triggers are root-wide.** For example, any top-level restack unflips an unredirected direct frame (`portable/windows.rs:1926`), so a restack of a window on the other monitor would drop the game out of direct scanout. There are about 30 distinct unflip reasons (cursor, shapes, overlay, stack, configure/unmap/destroy, authoritative root Present, CRTC transform, storage reallocation, successor failures).
- **VRR is already per CRTC.** `live_direct_vrr_wanted_by_output` and `direct_frame_wants_vrr` (`kms/scanout.rs` ~1100–1187) decide VRR per output from the direct frame that output shows. It prefers the queued/pending frame's policy when one exists. There is no explicit "one output" VRR limit: with the user's mixed-refresh pair there is simply never a direct frame.
- **The M1 probe is cached.** The TEST_ONLY commit runs once per client buffer, cached by `source_id` and invalidated on topology change; entering direct also needs a run of 8 eligible Presents (`SCANOUT_M2_ELIGIBLE_ROOT_PROBATION`). Once direct, neither repeats (eligibility and topology hashing still run per Present).

## Prior art (behaviour only)

- **Xorg** (MIT, `hw/xfree86/drivers/modesetting/present.c`): a window can flip only if it covers the entire X screen, one window at a time; VRR is set on every CRTC at once. Per-output direct scanout is therefore a capability Xorg does not have; there is no Xorg client contract for it beyond Present's ordinary completion semantics, which stay as today.
- **wlroots** (MIT, `types/scene/wlr_scene.c`): each output decides direct scanout on every frame from the list of what is visible on that output only (one entry, no colour transform → try the client buffer with a TEST_ONLY commit, else compose). Outputs are independent, so two outputs can be direct at once and a change on one output never affects another. yserver takes the *idea* (per-output decision, only what is visible on that output counts) but not the per-frame TEST_ONLY (yserver's probe is per buffer, cached) nor the stateless model (X11 Present needs the pending/current/unflip/fallback machinery for CompleteNotify, IdleNotify and a correct replacement image).

## Design

1. **Owners.** A *direct owner* is today's direct frame record and its bookkeeping (pending, queued successor, current, unflip request/reason, fallback source, probation streak, pins, one completion event, a retirement set). Ownership is either:
   - **one `Root` owner** over all outputs — exactly today's grouped ownership, grouped submission and grouped composed unflip (the grouped unflip stays because independent replacements hit `ENOSPC` on AMD, `kms/scanout.rs` ~1767), with today's topology requirements; or
   - **up to one `Output` owner per output**, each with its own instance of that bookkeeping, a retirement set of exactly its output, and commits on its own CRTC only.
   The two never coexist. A `Root` candidate while any `Output` owner exists, or an `Output` candidate while the `Root` owner exists, first unflips the current owner(s) through the existing composed-unflip path, then enters through normal probation. Invariant kept from today: one Present produces exactly one completion, and its source/fallback pins are released only after every CRTC that scanned it out has been replaced.
2. **Admission of `Output(i)`** (window rect and source extent equal output *i*; target Unredirected/COW/COW descendant as today):
   - output *i*'s CRTC is on the primary DRM device; other outputs' devices and refresh rates do not matter (the homogeneity rule stays for `Root` only);
   - the Present's selected CRTC is output *i*'s CRTC. If the client selected another CRTC, the Present is composed (today's completion domain: the selected CRTC's clock and MSC; mislabelling the clock or completing without a sample from that CRTC are both wrong);
   - the M1 probe tests only CRTC *i*'s primary plane with the buffer at source (0,0) and output *i*'s size, and checks modifier support on that plane only; "outputs tile the root" applies to `Root` only; the probe cache key includes the coverage;
   - the existing M2 geometry/shape checks are generalised from the root rectangle to output *i*'s rectangle (`direct_shape_chain_covers_root` → covers the owner's rectangle).
3. **Scheduling is scoped to the owner's outputs.** Every root-wide gate becomes a gate over the outputs a transaction touches: admission and successor arbitration ignore scene flips pending on other outputs; `scene_wants_compose` readiness, wakeups and the reentry barrier stop only the owner's outputs; the compose pass skips only outputs that are direct (pending, held, or awaiting their unflip replacement) and every other output composes and flips as if no direct frame existed. The existing per-output commit gate and accepted-event identities (`platform/gamma.rs` ~202, `drm/atomic.rs` ~479) decide whether a CRTC is busy; no new scheduler or device-wide serialisation is added.
4. **Unflip causes, by what output *i* shows.** A cause revokes output *i*'s owner only if it can change what output *i* shows. Visibility uses the scene's existing per-output decision (top-levels filtered by output intersection, topmost candidate, COW suppression per output: `yserver-render` `scene/build.rs` ~310–328) and root-overlay clipping per output (`root_overlay.rs` `apply_list_for_output` ~108), not a new policy:
   - **Presents:** an authoritative Present revokes output *i*'s owner only if it is visible on output *i*. The existing hidden-COW rule (#299) is generalised from "hidden over the whole root" to "hidden over output *i*", so a compositor stage shaped away from the game's monitor and drawing on the other monitor does not revoke the game.
   - **Other windows:** restack, map/unmap, configure — only if the window, in the old or the new stacking order and extent, is above the direct window and intersects output *i*.
   - **The direct window's own chain:** configure, unmap, destroy or reparent of the direct window or any ancestor revokes its owner (the ancestor walk of the shape check is reused), as do shape changes on that chain (today). Lifecycle invalidation by drawable/source/fallback reference stays as today.
   - **Root overlay** (software cursor sprites, overlay windows): only where it intersects output *i*.
   - **Per-CRTC causes** (cursor commit/bind/show/hide failures on that CRTC, successor submit failures): that output's owner only.
   - **Global causes, unchanged:** RANDR topology change, any CRTC transform (page flipping stays off while any CRTC is transformed), DPMS (it disables the whole output set today), storage reallocation, VT switch, teardown — every owner. Gamma changes do not unflip (today they are CRTC properties on the next commit; unchanged).
   When visibility on output *i* cannot be established, the cause revokes (today's behaviour).
5. **Present semantics unchanged.** One completion per Present, stamped in the selected CRTC's domain, after every CRTC that scanned it out retired (today's rule; for an `Output` owner that is its one CRTC). MSC/UST of windows on composed outputs are unaffected.
6. **Cursor.** Direct-mode cursor rules apply per output: hardware cursor required on the owner's CRTC, cursor-only commits for the owner's CRTC while held. External cursor synchronisation after binding, its pending-acknowledgement check and the cursor-mode query are scoped to the owner's outputs, so a composed cursor transition or a pending scene frame on output *j* cannot revoke the owner on output *i*. The cursor on a composed output behaves exactly as with no direct frame.
7. **Readback.** GetImage/root reads of a region scanned out by an `Output` owner rebase the source by the output's origin (the direct read route assumes a root-origin source today, `kms/readback.rs` ~26).
8. **VRR follows the owners with no new rule** (`live_direct_vrr_wanted_by_output` reads per output; composed vsync flips already request VRR off, `drm/page_flip.rs` ~291). The VRR policy and validation with several outputs on is PR B.
9. **Tearing has one owner (user decision 2026-10-27).** Tearing (`PresentOptionAsync` served with async flips) stays bound to ONE direct owner at a time, as today. Its existing effects — hardware cursors suppressed, async timing for composed commits — are scoped to the tearing owner's outputs instead of all outputs. A second output's direct client that also asks for tearing while the owner exists stays direct but flips synchronised to its own vblank; it can become the owner once the first stops tearing. **This limitation is documented to users** (see Documentation) and stated in the PR description.

### Reuse

- The direct owner record and its machinery (probation, successor slot, composed/fresh unflip, fallback, pins, per-CRTC retirement, `completion_output_idx`) are reused unchanged inside each owner; `Root` keeps today's single grouped owner.
- `ScanoutM0Coverage::Output(idx)` classification; the M1 probe and its cache; the lower-level direct submission that already takes a plane slice (`drm/modeset.rs` ~2167).
- The scene's per-output top-level/COW visibility (`scene/build.rs` ~310–328) and `apply_list_for_output` for overlay clipping.
- Per-output compose readiness and submission (`scene/tick.rs` ~109, `platform/scene_sink.rs` ~91) and the per-output commit gate.
- The shape-chain ancestor walk (`kms/scanout.rs` ~75) for chain invalidation and for the hidden-on-output rule.
- Per-CRTC cursor movement and VRR cursor deadlines (`platform/cursor.rs` ~469, ~588).

## Non-goals

- **PR B:** VRR policy and validation with several outputs on (cursor deadline per output, composed outputs never carrying VRR, MSI + LG hardware run with VRR).
- A window spanning parts of two outputs (neither `Root` nor `Output`): composed, as today.
- Mirrored or overlapping outputs, transformed outputs (page flipping stays off while any CRTC is transformed, as today), outputs whose mode size differs from their RANDR size.
- `Output` direct on a secondary DRM device (multi-GPU); a Present whose selected CRTC is not the owner output's CRTC (composed).
- Per-output DPMS or gamma-driven unflips.
- Overlay planes, scaling, a direct buffer smaller than the output.
- Two tearing owners at once (documented limitation, design item 9).
- Any change to single-monitor behaviour.

## Documentation

- `docs/status.md` (and `docs/high-level-design.md` where it describes direct scanout): direct scanout is per output; tearing serves one direct output at a time — a second fullscreen client on another monitor that disables V-Sync gets synchronised flips while the first one tears.
- The PR description repeats the limitation.

## Must keep working

- Single monitor: everything as today (mpv/CS2 direct, VRR, tearing, Cinnamon unredirect, lock screen, notifications, COW cases fixed in #299).
- Two monitors at the same refresh with a window covering the whole root (`Root`): grouped direct, grouped unflip, as today.
- Present completion order and MSC per CRTC.
- Hot-plug / RANDR changes while an output is direct: clean unflip, no stale frame.

## To measure before the plan

- **Muffin with two monitors.** Whether muffin unredirects a fullscreen window that covers one of two monitors (the client then presents `Unredirected` with `Output` coverage), or keeps compositing it into its root-sized stage (then the game reaches yserver as a COW Present with `Root` coverage and this PR alone does not give it direct scanout under Cinnamon). Measured with the scanout debug summary (`output_coverage`, target) in a Cinnamon session with both monitors on; behaviour observed only, no muffin source read. Without a compositor (i3, bare) the client window is presented directly and is `Output`.

## Acceptance

- Backend tests (plan): per-output entry/exit; a Present, restack or scene flip on output *j* leaves output *i*'s owner current (including a shaped-away compositor stage drawing on *j*); composed flips continue on *j* while *i* is direct and while *i* enters, takes successors and unflips; `Root`↔`Output` transitions with exactly one completion and pins released after the last consumer; a Present selecting another CRTC is composed; direct-window ancestor changes revoke without another Present; GetImage on a nonzero-origin output; cursor crossing outputs and image changes while *j* composes; `Root` grouped behaviour unchanged; global causes revoke every owner.
- Hardware, NVIDIA card1, MSI (DP-3) + LG (HDMI-2) both on:
  - mpv fullscreen on the MSI under i3 or bare: direct on the MSI output only, ~100 flips/s on its CRTC, the LG keeps composing (a clock/video on it advances);
  - restacks, a playing video and Discord activity on the LG: no unflip on the MSI;
  - a notification over the game on the MSI: unflip on the MSI only;
  - single-monitor regression run (as #298's runs).
- AMD card0 if two outputs can be connected there.

## Provenance

Xorg (MIT) was read for its flip condition. wlroots (MIT, local checkout `~/Projects/wlroots` bd75ebfe) was read for its per-output direct-scanout behaviour; no wlroots code is copied, and the design keeps yserver's own state machine. No KWin, Mutter or Muffin (GPL) source was read; muffin's multi-monitor unredirect behaviour will be observed through yserver's own logs.
