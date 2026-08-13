# Direct-Level Latest-Wins Supersession Spec (delta)

Date: 2026-08-12. Status: **Approved for implementation.**
Related findings: `docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md`
(§4b, §6). Extends the Phase B direct-scanout work on branch
`fix/fullscreen-novsync-stutter` (spec
`2026-08-11-async-present-defer-supersession.md` was the Phase A flood fix).

## Problem

Phase B hardware validation on the nvidia box (with the `YSERVER_HW_CURSOR_NVIDIA=1`
A/B override) showed direct scanout engages without crash (1,984 `live direct submit`,
0 `request_exit`), but **thrashes**: ~3.7 composed unflips/s during the whole
session (553 in ~150 s, 407 of them after a single direct submit), and
`page_flip/s` drops from ~60 to 54-56. Root cause: a no-vsync game (CS2, synced
`options=0x8`, ~380 presents/s) floods `try_present_direct`; every present that
arrives while a direct flip is in flight hits the
`pending.is_some() || has_pending_page_flips()` gate (backend.rs:13212),
calls `request_direct_unflip()`, falls to the Copy path, and tears the direct
frame down — then re-enters on the next present. Direct/composed thrash.

### Why the existing supersession core does not coalesce the flood

`present_flip_in_flight()` (backend.rs:13627) returns
`scene.has_pending_page_flips()`, which reads the **scene's** `pending_acks`
(scene.rs:907-909). The direct frame is submitted via `submit_direct_scanout`
and retires via `retire_direct_output` (backend.rs:12681) — it **never touches
the scene's `pending_acks`**. So while a direct flip is in flight,
`flip_in_flight == false`; `classify_msc_due` classifies arriving synced
presents as `ExecuteNow` (no `Park`), they reach `try_present_direct`, and the
13212 gate shreds them. The core supersession machinery never gets the window
to park-and-scrap them.

## Design

Two pieces, both scoped to the direct-scanout path. Synced-present behavior on
the composed path is unchanged.

### 1. `present_flip_in_flight()` — include the in-flight direct flip

```
present_flip_in_flight() = scene.has_pending_page_flips() || scanout_m2.pending.is_some()
```

With this, a present arriving while a direct flip is in flight is parked
(`classify_msc_due` already returns `Park` for `eff == clock+1 &&
flip_in_flight`) and the existing synced same-target supersession
(`supersede_covered_pending_presents`, successor gate already relaxed to
full-extent in 2026-08-01) scrap-pools it before it reaches the direct path.
Effect: the ~380 presents/s flood coalesces to ~1 present/flip before
`try_present_direct`.

Side effects to audit (expected, verify in review):
- `present_display_idle()` = `!has_pending_page_flips() && !scene_wants_compose()`
  (backend.rs:13631, :7070): while a direct flip is pending, it returns
  `false` — correct, the idle-display fallback must not arm while a direct
  frame is in flight.
- `due_pass` re-classification after retire: when `pending` empties
  (`retire_direct_output`), `flip_in_flight` returns `false`, the parked
  entry's `classify_msc_due` re-evaluates to `ExecuteNow`, and the newest
  present executes — correct latest-wins on the composed path.

### 2. `scanout_m2.queued` slot — direct-level latest-wins

New field `queued: Option<DirectPresentFrame>` on `ScanoutM2State`
(backend.rs:317), alongside `pending`/`current`. A `queued` frame is fully
prepared (pins taken, fb handle retained, completion event captured) but not
yet submitted to KMS. Invariant: **at most one direct flip in flight per
CRTC** — `pending` is the in-flight frame; `queued` is never submitted while
`pending` exists.

`DirectPresentFrame` (backend.rs:301) gains a retained `fb_handle: u32`
(the m1 probe framebuffer handle; the probe cache entry can be dropped if the
topology changes while the frame waits). `plane_states` are rebuilt at submit
time — they depend only on `platform.outputs`, which is stable.

**`try_present_direct` (backend.rs:13212 gate):** replace the
`request_direct_unflip(); return Ok(false)` for a present arriving while
`pending.is_some()` with:

- if a `queued` frame exists, complete it as Skip (release pins, `Skip` +
  IdleNotify in `present_id` order via the existing completion machinery) and
  replace it;
- prepare and store the new frame in `queued`;
- return `Ok(true)` (no Copy, no unflip).

The `has_pending_page_flips()` (scene flip) branch stays a fall-to-Copy/ unflip
case: during direct scanout the scene should not be flipping, so this is a
defensive residual, not the common path.

**`retire_direct_output` (backend.rs:1507, when `pending.awaiting_outputs`
empties):** if `queued` is `Some`, move it to `pending` (chain-flip pending);
else the current `pending → current` transition. Do NOT submit here — the
chain-flip is deferred to `maybe_composite` (next tick).

**`maybe_composite`:** when `pending.is_none() && queued chain pending &&
!unflip_requested`, submit the retained frame (`submit_direct_scanout`),
promote to `pending`, set `hold_direct`/`unflip_requested=false` (mirroring
the current submit block). If `unflip_requested` (cursor/overlay changed while
the frame waited), complete `queued` as Skip and proceed with the normal
composed unflip.

**Teardown:** `stop_direct_after_scanout_replaced` (backend.rs:1305) clears
`queued` — complete as Skip or Copy per the path, always releasing its pins.

## Out of scope

- The `YSERVER_HW_CURSOR_NVIDIA` override itself (scene.rs:639-644) — an
  A/B lever for validation; default behavior on nvidia-drm stays SW cursor.
- Getting direct scanout to engage when the cursor is not HW (the m1 guard
  requires `cursor_hw`; the plan's known software-cursor blocker).
- A multi-frame direct queue (N > 1) — the single `queued` slot is latest-wins
  for a flood; a deeper queue adds latency for no observable benefit.

## Acceptance

1. With `YSERVER_HW_CURSOR_NVIDIA=1` on the nvidia box, a no-vsync fullscreen
   CS2 session keeps `page_flip/s` at ~60 (not 54-56) during gameplay.
2. Composed unflips in steady state drop to ~0/s (was ~3.7/s); the
   direct→unflip→direct cycle is gone. Copy fallbacks during the game are
   near zero.
3. No `request_exit`, no released-while-scanned dma-buf (the C1 degradation
   path is untouched).
4. Synced presents on the composed path are bit-for-bit unchanged: the
   existing `classify_msc_due`, supersession, and msc-due unit suites pass
   unchanged; `present_flip_in_flight`'s new conjunct is additive.
5. CI: `cargo clippy --all-targets -- -D warnings` clean.

## Reference points

- `crates/yserver/src/kms/render/backend.rs:13627` — `present_flip_in_flight`.
- `crates/yserver/src/kms/render/backend.rs:13212` — the in-flight gate to replace.
- `crates/yserver/src/kms/render/backend.rs:1474` — `retire_direct_output`.
- `crates/yserver/src/kms/render/backend.rs:317` — `ScanoutM2State`.
- `crates/yserver/src/kms/render/backend.rs:301` — `DirectPresentFrame`.
- `crates/yserver-core/src/core_loop/process_request.rs:8602` —
  `successor_presents_full_extent` (already relaxed).
