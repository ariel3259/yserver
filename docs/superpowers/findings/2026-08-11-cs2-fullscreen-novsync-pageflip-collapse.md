# CS2 fullscreen no-vsync stutter — root cause (2026-08-11)

Status: **Mechanism confirmed from telemetry + submit trace. Fix direction identified,
not yet implemented.**

## 1. Scope and evidence source

- Live hardware run on tty2 (Cinnamon session, `DISPLAY=:7`, yserver release
  binary built from `dri3-syncobj-drm-signal` incl. the transparency fix).
- Evidence: `yserver-hw-cinnamon.log` (543 loop-telemetry lines,
  `YSERVER_LOOP_TELEMETRY=1`), `yserver-cinnamon.submit.tsv` (4.2 MB submit
  trace), `cinnamon.log`.
- Display: single 1920×1080 (root=(1920,1080)), page_flip baseline ~60/s.
- Mouse: Razer DeathAdder V3 connected at USB **Full-Speed 12 Mbps**
  (`/sys/bus/usb/devices/1-6/speed` = `12`) → `bInterval=1` = **1000 Hz**, NOT
  8000 Hz. The high-polling-flood hypothesis is dead on arrival.
- User's A/B test: **vsync ON = fluid**; **vsync OFF = "demasiados frames"**
  (stutter) with stable in-game FPS counter. Stutter only while in-game
  fullscreen; desktop cursor/Windows smooth.

## 2. Confirmed mechanism

### 2a. The fullscreen game never reaches direct scanout
- The game (client c237) presents **full 1920×1080 swapchain buffers**
  (`scanout_m0 shape … target=Unredirected coverage=Root rect=(0,0,1920,1080)
  source_extent=(1920,1080) … update=0x3a00xxx update_full=false … eligible=false`).
- `scanout_m0_summary` shows `m1_probe_pass=0` for the whole session — the
  atomic TEST_ONLY direct-scanout probe is never even attempted.
- Two eligibility gates reject every present:
  - `!candidate.explicit_sync` (backend.rs:13002) — CS2 presents via
    **PresentPixmapSynced** (opcode 145, minor 5), so `explicit_sync=true`.
  - `update_region_xid == 0 && update_is_full` (backend.rs:13012-13013) —
    `update_is_full = update_rects.is_none()` (process_request.rs:9205), but the
    game's presents carry an update region → `update_full=false`.

### 2b. Composited path cannot sustain refresh under a no-vsync flood
- With vsync ON the game presents ~60/s (== refresh) → the composited path keeps
  up → fluid.
- With vsync OFF the game floods presents (present counter advances ~150 per
  fraction of a second). Because the window is not unredirected, Muffin (c25)
  keeps compositing it via **MIT-SHM GetImage** (`op130.4`, ~60/s, up to
  3.7 ms/request core time) and yserver re-composites the full 1920×1080 scene
  per present.
- **`page_flip/s` collapses from ~60 to 27–47 Hz** during no-vsync gameplay
  (01:42:51→01:43:06; falls to 6 Hz / 1 Hz as the user exits). The display
  literally updates slower than refresh → the visible stutter.
- Input delivery is collateral: `host_input_max_gap` spikes to 715 ms–2.1 s in
  the same window (shared channel + loop contention) → mouse feels stuttery too.

## 3. Why "too many frames" with a stable FPS counter
CS2's in-game FPS counter measures the game's own render loop, which is decoupled
from yserver's presentation. The game renders uncapped; yserver drops frames in
the composited path AND drops page flips below refresh. FPS counter stays high
while the actual displayed motion judders.

## 4. Fix direction (not yet implemented)

Primary: **get fullscreen authoritative-root explicit-sync presents onto direct
scanout** so the game bypasses composition entirely:
- Relax `!explicit_sync` for authoritative-root full-extent presents, handling
  the acquire/release fences (PresentPixmapSynced syncobjs) on the flip path.
- Treat a fullscreen (authoritative-root, full-extent) present as full regardless
  of its update region — the whole buffer replaces scanout anyway; the update
  region only matters for the partial-copy path.

Backstop (independent): make the composited present path **vsync-locked
latest-wins** — compose once per vblank using the newest present instead of
re-composing per present, so a no-vsync flood cannot push flips below refresh.
This is what real Xorg compositors do.

The busy-loop over-composition defect (findings
`2026-08-11-yserver-leak-cinnamon-regression-and-transparency-bug.md` §2) is
related but separate; it is unchanged by this fix.

### 4a. vsync ↔ explicit sync — clarification (resume 2026-08-11)
The user asked whether toggling vsync forces explicit sync to follow its state.
**Data says no — they are orthogonal and both are constant:**
- The game (client c237) sends `PresentPixmapSynced` (opcode 145, minor 5 =
  explicit sync) **and** `PresentOptionAsync` (`options=0x8`) in BOTH the vsync-on
  and vsync-off windows. Neither changes with the toggle. There is no "stuck"
  explicit-sync state.
- `effective_target_msc` is `None` for async presents
  (present_scheduler.rs:70) → `classify_msc_due` returns `ExecuteNow` **always**
  (present_scheduler.rs:94-95) → presents never park → `supersede_covered_pending_presents`
  early-returns on `effective_target_msc == None` (process_request.rs:8810-8812)
  → **zero supersessions** (Hollow Knight in #117 had 87.8%). Every async present
  marks full damage and re-composes.
- Why direct scanout "worked before": the `!candidate.explicit_sync` gate
  (backend.rs:13002) was introduced **in the same commit that created direct
  scanout** (`9c4ef8f0 feat(scanout): add direct compositor scanout`). Direct
  scanout has never supported explicit-sync presents. #117 (`ab1d5462`) validated
  the vsync-off present pipeline only on the **implicit** Pixmap path (Hollow
  Knight, mpv — commit note: "NVIDIA WSI never takes PixmapSynced, so that path
  stays unverified on hardware"). CS2 on RADV **takes the unvalidated explicit
  path**, which is exactly where the page-flip collapse happens.
- Aggregate telemetry vsync vs no-vsync: `page_flip/s` 59-60 → **38.6**,
  `iter/s` 1100-1400 → 656, `iter_wall_max` 1.0ms → 4.9ms, `drain_max` 3 → 12.
  Same present rate (~200-300/s, `145.5`) in both windows — the present rate is
  NOT the differentiator; the loop saturation under no-vsync is.

### 4b. Adversarial review of the direct-scanout plan (2026-08-11) — flood sheds onto Copy
Review of the implementation plan found the direct-scanout gate relaxation
ALONE does not fix the collapse. Under a no-vsync flood with direct scanout
engaged, the in-flight gates shed every excess present onto the Copy path and
tear down direct scanout:
- `try_present_direct` (backend.rs:13040 `unflip_requested`,
  13059 `pending.is_some() || has_pending_page_flips()`) returns `Ok(false)`
  for every present arriving while a flip is pending → falls to
  `execute_present_pixmap_copy` (source→COW copy + damage→recompose at the
  game's uncapped rate).
- Worse: 13059 calls `request_direct_unflip()` on the way out → the flood
  actively tears down the direct scanout it just entered.
- `supersede_covered_pending_presents` early-returns on
  `successor.effective_target_msc == None` (process_request.rs:8810) and
  `classify_msc_due(eff=None)` = always `ExecuteNow` (present_scheduler.rs:94-95)
  → **async presents never park and never supersede** → the flood is
  un-coalesced.
- **Therefore the PRIMARY fix is async present defer+supersede** (treat async
  `eff=None` as `clock_msc+1` so they park when a flip is in flight and get
  superseded — what Xorg's scrap logic does). The direct-scanout gates are a
  SECONDARY efficiency layer, and require an additional direct-level latest-wins
  supersession (replace the pending `DirectPresentFrame` instead of falling to
  Copy) to survive the flood.
- Test-fixture caveats found by review: `for_tests()` has cursor `Sw`
  (scene.rs:896-899) and an empty `dri3_syncobjs` map (backend.rs:3251), so
  scanout-m1 and release-wake tests cannot pass on the fixture — predicate-level
  tests required, hardware validation for the rest.



## 6. Hardware validation result — Phase A (2026-08-12, nvidia box, CS2)

Session: Cinnamon, `DISPLAY=:7`, NVIDIA_PROPRIETARY (card0, `driver_id=NVIDIA_PROPRIETARY`),
`YSERVER_LOOP_TELEMETRY=1` + `YSERVER_SUBMIT_TRACE`, single 1920×1080 output, ~60 Hz.
Logs: `yserver-hw-cinnamon.log` (37 MB), `yserver-cinnamon.submit.tsv`.

### Result: the no-vsync stutter is FIXED on hardware

- `page_flip/s` histogram over the session: dominant values 59.8–61.0
  (851× 60.0, 369× 59.9, 57× 59.8, 55× 61.0…); the only sub-40 values (1–4) are
  the session exit. The 27–47 Hz collapse window is gone.
- Supersession is active and coalesces the flood: `present_skips/s` runs
  140–535 with median 266 during gameplay (vs 0 during idle desktop). The game
  floods ~323 presents/s (op145.5 PresentPixmapSynced) and the synced
  supersession absorbs them down to one per flip.
- `deferred=0/2`–`0/4` in loop telemetry — no presents parked on source waits.

### Critical correction to §4a (the findings' async premise was WRONG)

§4a claimed the game sends `PresentOptionAsync (options=0x8)`. **That is a
misread of the constant**: the Present protocol defines `PresentOptionAsync =
0x1`, `PresentOptionAsyncMayTear = 0x10`, and `PresentOptionSuboptimal = 0x8`.
All 50,127 game shape lines carry `options=0x8`, which is `Suboptimal`, and
`masked_options & PRESENT_ALL_ASYNC_OPTIONS (0x11) = 0` → **synced**. So:

- These presents get `effective_target_msc = Some(…)` and are coalesced by the
  **pre-existing synced same-target supersession** (present-deferred-supersession,
  PR #117) — NOT by the Phase A async defer+supersession (spec
  2026-08-11-async-present-defer-supersession), which only fires for async
  presents.
- The observed improvement on this box therefore comes primarily from (a) the
  synced supersession already merged in master and (b) the merged DRI3 syncobj
  work + maintainer fixes (PR #122, `5b730f75`..`8a88a336`) making the
  PresentPixmapSynced acquire/source path functional here. Phase A remains
  correct and necessary for a genuinely-async flood (the spec's target case,
  e.g. a vsync-off game sending `PresentOptionAsync`), but CS2-on-this-box is
  not that case.
- The `effective_target_msc = None` claim in §4a is likewise unproven by this
  session: synced presents materialize `Some(eff)` whenever the target is
  future. The original §4a reasoning (that async presents never park/supersede
  because eff=None) was built on the 0x8=Async misread and should be read as
  hypothesis, not mechanism, for the actual CS2 session.

### Phase B (direct scanout) did NOT engage on this box

`m1_probe_pass=0`, `m1_probe_reject=0`, `m1_probe_error=0` for the entire
session — the m1 pre-probe was never attempted. The game's presents pass the
target (`Unredirected`), coverage (`Root`), offsets, and valid-region gates;
the blocker is upstream of the probe in `scanout_m1_probe_eligible`
(backend.rs:1551): `scanout_allowed`, `kms_outputs_active`, `cursor_hw`, or
`root_overlay.is_empty()`. On this NVIDIA proprietary box the hardware cursor
plane is initialised (`CursorWidth=256`), but `cursor_mode()` returns `Hw`
only when EVERY output's last-frame mode is `Hw`; a software or hidden cursor
(CS2 hides the OS cursor in fullscreen) leaves it `Sw`, which the m1 guard
requires to be `Hw`. So Phase B was correctly inert here — no direct scanout
should engage without the Hw-cursor precondition. Recording as the plan's
Task-8 "software cursor / overlay" known blocker, not a Phase B regression.
Further Phase B hardware validation needs a session where the cursor stays in
Hw mode.

### I2 (final-review finding) resolved

The final whole-branch review flagged the `options=0x8` / async contradiction
before this run; the hardware data confirms the review's reading. The findings
§4a text above should be corrected accordingly (this section supersedes it).

### Phase B gate diagnosis (2026-08-12, second session)

The m1 guard now logs its decline reason once per session (commit b09ca26b).
First decline fires at desktop startup: `scanout_allowed=true
kms_outputs_active=true cursor_hw=false root_overlay_empty=true` — **the only
blocking gate is `cursor_hw`**. Root cause is structural on this box:
`scene.rs:639-644` deliberately disables the HW cursor strategy on nvidia-drm
(`hw_cursor_strategy_enabled() && !platform.is_nvidia_drm()`), because the
legacy cursor-move ioctl stalls ~1 vblank per drag and the atomic cursor path
regressed rendering. So `cursor_mode()` is always `Sw` on NVIDIA, and Phase B's
`cursor_hw` precondition can never hold there. The m1 probe is behaving
correctly; Phase B simply requires a non-nvidia-drm (AMD/Intel) host for
hardware validation. The predicate-level scanout tests pass; the live direct
scanout flip remains hardware-unvalidated (pending an AMD session).

### Direct-scanout latest-wins supersession — hardware validation (2026-08-13, nvidia box, CS2)

Task 6 of spec `2026-08-12-direct-scanout-latest-wins-supersession` on branch
`feat/direct-scanout-latest-wins`, build `7c2079567aef`, harness
`tools/yserver-cinnamon-hw-cs2.sh`. Session: Cinnamon `DISPLAY=:7`,
NVIDIA_PROPRIETARY, single 1920×1080@60, `YSERVER_LOOP_TELEMETRY=1` +
`YSERVER_SUBMIT_TRACE`, **`YSERVER_HW_CURSOR_NVIDIA=1`** (the Phase-B A/B
override). Logs: `yserver-hw-cinnamon.log` (427 loop-telemetry lines),
`yserver-cinnamon.submit.tsv`, `cinnamon.log`. Run 00:00:44Z→00:07:52Z
(~7 min; the first of the two re-runs was discarded — contaminated by a Marvel
Rivals session sharing the VT). **Result: PASS — the thrash is gone.**

- `page_flip/s` sustained **60.0** during gameplay (bucket means: min 0-1 = 25.4
  startup/load ramp, min 1-2 = 36.3, min 2-3 = 50.2, min 3-4 = 58.7, min 4-7 =
  60.0/60.0/60.0; the 54-56 collapse is gone). The only sub-40 samples (down to
  1-2) are the session exit teardown (signal 15 → shutdown), not gameplay.
- `composed unflip retired` = **0** (was 553/session pre-fix); direct scanout
  stays engaged through the flood.
- `scanout_m2: live direct submit` = **7835**; `m1_probe_pass=11`,
  `m1_probe_reject=0`, `m1_probe_error=0` — **Phase B engaged on the nvidia box
  this time** (the earlier §6 session saw `m1_probe_pass=0` because `cursor_hw`
  was false; the `YSERVER_HW_CURSOR_NVIDIA=1` override satisfies it).
- Chain-flip path exercised: `chain-flip promoted` / `chain direct submit`
  lines present (e.g. 00:03:00, source 5730/5728/5729/5732); `chain direct
  submit failed` = **0**.
- Queued-slot Skip path: `scanout_m2: queued frame skipped` = **21** (sources
  58/59, a handful of the ~72k presents) — small/non-zero as allowed by the
  plan; Piece 1 coalescing keeps the slot mostly empty.
- `request_exit` = none; `missed_pageflips/s` = 0; `present_skips/s` mean 106
  (426 samples) — the synced supersession keeps coalescing the flood.
- EOPNOTSUPP appears **once**, at startup: `DRM_IOCTL_CRTC_QUEUE_SEQUENCE`
  (commit `f948352b` probe) is unsupported by the NVIDIA absolute-vblank arm and
  falls back to flip-driven MSC. Benign and pre-existing; not a direct-scanout
  regression. The harness's `-- EOPNOTSUPP -- expected absent` grep should be
  relabeled (fallback is expected on this box).

## 5. Reference points

- `crates/yserver/src/kms/render/backend.rs:13000-13013` — direct-scanout
  `eligible` predicate (explicit_sync, update_is_full, root_overlay…).
- `crates/yserver-core/src/core_loop/process_request.rs:9205-9206` —
  `update_is_full`/`explicit_sync` from the Present request.
- `crates/yserver/src/kms/render/backend.rs:12945` — `note_present_pixmap`,
  scanout_m2 / direct-scanout machinery.
- `crates/yserver/src/kms/render/backend.rs:1593` — `probe_direct_scanout_test_only`.
- Telemetry: `YSERVER_LOOP_TELEMETRY=1` → `page_flip/s`, `gap_max` (cursor-lag
  proxy), `host_input/s`; `YSERVER_SUBMIT_TRACE` → per-submit kinds.
