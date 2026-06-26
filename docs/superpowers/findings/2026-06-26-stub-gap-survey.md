# X11 stub / gap survey — 2026-06-26

Real, currently-unimplemented X11 behaviour in the tree. Verified against
`master` @ `8c9bf61d` by reading each cited site (a raw survey was seeded by
an opencode/GLM-5.2 pass, then each claim re-checked; only confirmed gaps are
listed here). Deliberately-dropped legacy (non-TrueColor visuals, indirect
GLX, endian-swapped clients, DDX ABI) is excluded.

## Priority

Ranked by impact × likelihood-of-being-hit × effort. Tier tags (`[T1]`/`[T2]`/`[T3]`)
are repeated on each item below.

**Tier 1 — highest ROI**
1. **RENDER `ClipByChildren` on Composite/CompositeGlyphs/Trapezoids** — highest-traffic
   path (every glyph run + icon composite); proven bug class (xfce decoration buttons,
   systray icons); medium effort, reuses the existing clip-by-children helper.
2. **GLX FBConfig synthesis → real `__DRIconfig`** — biggest single app-unblock (native
   Chromium/Electron GL, currently needs `--use-angle=vulkan`). Highest effort + most
   fragile (must match mesa `driConfigEqual` exactly) and a workaround exists → schedule
   when native Chromium-GL is wanted, not urgent.

**Tier 2 — solid, mostly contained**
3. **MapSubwindows grandchild Expose** — ~10 LoC, kills a blank-region class. Cheapest win.
4. **SetPictureFilter bilinear** — quality bump for scaled/transformed content (nearest today).
5. **GrabServer/UngrabServer atomicity** — correctness for WM atomic restack/reparent.
6. **RRCreateMode** — unblocks `xrandr --newmode`/custom modes (today: loud BadImplementation).

**Tier 3 — low / niche / by-design** (leave unless something demands them)
PictFormat-aware sampling · GLX TFP indirect sampling · RRSetPanning · RRCreateLease ·
SetScreenConfig 1.0 · GetMotionEvents · RecolorCursor · UnmapNotify.from_configure ·
deep-chain crossings · DRI3 SetDRMDeviceInUse · pointer-barrier edge-runaway.

> Out of scope for this list but **outranks most of Tier 2** in real-world impact: the
> **MATE 0×0 RandR-reconfigure black-screen wedge** (`cinnamon-monitors.xml` is a latent
> trigger). Slot it alongside Tier 1 when picking next work. See
> [[project_mate_black_screen_monitors_xml]].

## RANDR

- `[T2]` **RRCreateMode** (minor 16) — `BadImplementation` stopgap; no custom modes.
  `crates/yserver-core/src/core_loop/process_request.rs:2970` (`16 | 29 | 45 =>` arm).
- `[T3]` **RRSetPanning** (minor 29) — `BadImplementation`; CRTC panning unsupported.
  Same arm, `process_request.rs:2970`. (read-only `RRGetPanning` at :2392 is real.)
- `[T3]` **RRCreateLease** (minor 45) — `BadImplementation`; no DRM-lease handoff (VR).
  Same arm, `process_request.rs:2970`.
- `[T3]` **SetScreenConfig** (legacy RANDR 1.0, minor 2) — no-op accept: replies
  `status=0` without any modeset. `process_request.rs:2766` ("no-op accept").
  (The RANDR 1.2 `SetCrtcConfig`/`SetScreenSize` paths *are* real modesets —
  `backend.rs:10777`, `process_request.rs:2677` — not stubs.)

## Core protocol

- `[T2]` **GrabServer (36) / UngrabServer (37)** — pure log no-op; no server-grab
  state exists, so clients relying on server grabs for atomic restacking get
  no atomicity. `process_request.rs:202-203` → `log_void` (`:16544`).
- `[T3]` **RecolorCursor (96)** — no-op log stub. `process_request.rs:204`.
- `[T3]` **GetMotionEvents (39)** — always returns `nevents=0`; pointer motion
  history is never recorded. `handle_get_motion_events` `process_request.rs:16882`.

## Window lifecycle / event semantics

- `[T3]` **UnmapNotify.from_configure** — always emitted `false`; the configure-driven
  implicit-unmap case is never wired. All 6 callsites pass `false` literally
  (`process_request.rs:1163,1166,18791,18795,18856,18859`,
  `process_disconnect.rs:52,57`); encoder accepts the byte (`x11/mod.rs:3236`).
- `[T3]` **Crossing (Enter/Leave) events** — fire on top-level transitions only, not
  the full deepest-descendant→ancestor chain (spec-divergent; works in practice
  for MATE/xfce/marco). `pointer_fanout.rs:941-957`.
- `[T2]` **MapSubwindows** — re-Exposes only direct children, not deep grandchildren
  promoted Unviewable→Viewable by the viewability cascade (contrast
  `handle_map_window`, which walks the subtree). `handle_map_subwindows`
  `process_request.rs:18655-18731`; known-issues.md:426-440 (~10 LoC fix unapplied).

## RENDER

- `[T1]` **ClipByChildren** — applied only on the FillRectangles/Clear path
  (`process_request.rs:1785`). Composite (`:1557`), CompositeGlyphs, and
  Trapezoids/Triangles/TriStrip/TriFan (`:1631`) still use
  `accumulate_damage_full_to_state` — latent over-damage of the same class that
  caused the xfce-button and systray bugs.
- `[T2]` **SetPictureFilter** — honoured only as `Nearest`. Bilinear/Convolution are
  parsed and stored but ignored at draw time (engine builds a fixed NEAREST
  sampler). `render_pipeline.rs:343-352`, `backend.rs:16149-16171`;
  known-issues.md:414-424.
- `[T3]` **Per-picture PictFormat / ARGB intent** — tracked but not applied: a
  `pict_format` + `force_opaque` flag exist per picture, but the engine still
  decides force-opaque from drawable *depth*, not the declared PictFormat.
  `crates/yserver/src/kms/core.rs:1366-1371` ("instrumentation-only … lands as
  a follow-on").

## GLX / DRI3

- `[T1]` **GLX FBConfigs are synthesised**, not mirrored from the real driver
  `__DRIconfig`s. `synthesise_glx_fb_configs` hand-builds 4 configs with
  hardcoded attributes (`process_request.rs:9096`); the inline comment warns
  these must match mesa's `driConfigEqual` exactly or dri3 screen creation
  fails — the path implicated in Chromium/ANGLE "failed to create drawable" on
  Mesa 26 (`:9105-9159`).
- `[T3]` **GLX_EXT_texture_from_pixmap** — `bind` succeeds (no protocol error) but
  indirect texture *sampling* of non-resident bindings is unimplemented, so
  bound texture content is never updated. `process_request.rs:9760-9765`
  (`TODO(glx-tfp)`).
- `[T3]` **DRI3 SetDRMDeviceInUse** — acknowledged but ignored (single-GPU design).
  `process_request.rs:8959-8965`.

## XFIXES / XInput2

- `[T3]` **Pointer barriers** — implemented and HW-verified, but a sustained hard push
  into a barrier makes the cursor jump to the far screen edge on release. Root
  cause is the core↔input-thread cursor model (the input thread integrates
  relative libinput deltas with no knowledge of barriers), not the barrier code.
  known-issues.md:199-216. Low priority (GNOME 49 dropped its X11 pressure-barrier consumer).

---

## Doc hygiene (separate, actionable)

Several `docs/known-issues.md` entries describe gaps that are now **fixed** and
should be reconciled with the code: RRSelectInput mask storage + ScreenChangeNotify
delivery (entry ~:132), AllowEvents(ReplayPointer) replay (~:135), SendEvent
parent-tree propagation (~:141), input-shape hit-testing (~:145), and "Real RANDR
SetCrtcConfig" (~:800, now a real modeset). Worth a pass to tick these off so the
doc stops over-reporting.
