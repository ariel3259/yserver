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

Side effects — **both must be fixed as part of Piece 1** (adversarial review
2026-08-12, finding #4):

- `present_completion_is_idle()` (backend.rs:7096) = `!has_pending_page_flips()
  && !scene_wants_compose()`. It does NOT consult `scanout_m2.pending`, and
  during direct scanout the scene is quiescent, so it commonly returns `true`
  while a direct flip is in flight. On flip-driven drivers without absolute
  vblank arming (`present_absolute_vblank_arm_supported() == false`), the
  due-pass idle-display fallback (process_request.rs:9097-9105) force-executes
  every parked `source_ready` entry each tick — re-introducing the flood at the
  13212 gate and un-parking exactly the presents Piece 1 must keep parked.
  **Fix: `present_completion_is_idle()` (and therefore
  `present_display_idle()`) must also return `false` while
  `scanout_m2.pending.is_some()`.** Audit the impact on
  `arm_present_completion_idle_vblanks` (backend.rs:19907): with the direct
  flip the only active scanout work, idle-sequence arms are suppressed and
  completions for vsync-on compositor presents pace via page-flip retire —
  which is the correct clock while direct holds.
- `due_pass` re-classification after retire: when `pending` empties
  (`retire_direct_output`), `flip_in_flight` returns `false`, the parked
  entry's `classify_msc_due` re-evaluates to `ExecuteNow`, and the newest
  present executes — correct latest-wins (into the direct path, via the
  re-entry point; adversarial review #11).

### 2. `scanout_m2.queued` slot — direct-level latest-wins

New field `queued: Option<DirectPresentFrame>` on `ScanoutM2State`
(backend.rs:317), alongside `pending`/`current`. A `queued` frame is fully
prepared (pins taken, completion event captured) but not yet submitted to
KMS. Invariant: **at most one direct flip in flight per CRTC** — `pending` is
either the in-flight frame OR a not-yet-submitted chain frame that will become
in-flight next tick; `queued` is never submitted while `pending` exists.

`DirectPresentFrame` (backend.rs:301) gains a retained
`fb: Arc<DirectScanoutProbeFramebuffer>` — **not a raw `u32` handle**.
`DirectScanoutProbeFramebuffer::Drop` calls `rm_fb` + GEM close
(modeset.rs:866-875); the m1 cache owns the entry it holds (`_framebuffer`
retained solely for lifetime), so a plain handle would dangle if the cache is
cleared while the frame waits (adversarial review #3). The `Arc` keeps the fb
alive across any `scanout_m1.clear` (including the topology-signature clear at
backend.rs:1599, which has no preceding `stop_direct`), and the kernel's own
fb refcount covers the frame once it is actually submitted. `plane_states` are
rebuilt at submit time — they depend only on `platform.outputs`, which is
stable.

**`try_present_direct` (backend.rs:13212 gate):** replace the
`request_direct_unflip(); return Ok(false)` for a present arriving while
`pending.is_some()` with the queued-store branch. **The queued-store branch
runs only when `pending.is_some() && !self.scene.has_pending_page_flips()`**
(adversarial review #7): a scene flip in flight takes precedence and falls to
the existing unflip/Copy path — during direct scanout the scene should not be
flipping, so the scene-flip arm is a defensive residual, and the degraded
composed-unflip window (both a direct pending and scene flips, backend.rs:12932)
must not enter the queued-store.

Queued-store branch:
- if a `queued` frame exists, complete it as Skip (release pins, `Skip` +
  IdleNotify; ordering per the synced-only guarantee below) and replace it;
- prepare and store the new frame in `queued`;
- **restore the pre-gate state exactly as the submit block does**
  (adversarial review #2): `unflip_requested = false`, `hold_direct = true`,
  and clear `unflip_fallback_source` / `unflip_shadow_ready` — the
  unconditional `request_direct_unflip()` at backend.rs:13201 ran before the
  gate and must not leave a stale unflip pending or an armed fallback marker
  that `note_present_pixmap` (13102-13113) would misread as "shadow already
  materialized";
- return `Ok(true)` (no Copy, no unflip).

**`retire_direct_output` (backend.rs:1507, when `pending.awaiting_outputs`
empties):** if `queued` is `Some` AND `!unflip_requested`, move `queued` to
`pending` (promoted, chain-flip pending); else the current `pending → current`
transition. Do NOT submit here. If `unflip_requested` became true while the
frame waited (cursor/overlay change), complete `queued` as Skip at retire and
let the composed unflip own the slot (adversarial review #6).

**`maybe_composite` chain-flip:** the early-return gate at backend.rs:12911-12914
must be amended so the chain submit is reachable (adversarial review #1 — as
written, `pending.is_some()` or `hold_direct` returns before any submit, so a
deferred chain-flip would never fire and the clock would freeze). Introduce a
distinct `scanout_m2.pending_is_submitted: bool` (or equivalent) separating
"in flight" from "promoted but unsubmitted": the gate admits the chain submit
when `pending.is_some() && !pending_is_submitted && !unflip_requested && no
scene flips`. On that path, submit the retained frame (`submit_direct_scanout`),
set `pending_is_submitted = true`, and mirror the current submit block's
state (`hold_direct = true`, `unflip_requested = false`). If
`unflip_requested` (cursor/overlay changed while the frame waited), complete
`queued`/promoted frame as Skip and proceed with the normal composed unflip.

**Chain-flip submit failure** (backend.rs:13241-13247 analogue — adversarial
review #8): the chain submit must have the same failure contract as the direct
submit — release the frame's pins, complete as Copy/Skip, reset the eligible
root probation, set `reentry_blocked_until_composed`, and fall through to the
composed path. Never `request_exit()`.

**Ordering guarantee (adversarial review #5):** the Skip completion for a
superseded queued victim is ordered per-window `present_id` only for **synced**
victims (the gated arm, run.rs:1496 → `present_pending_complete` hold-back).
An async victim (`eff=None`, ungated) completes immediately, outside the
hold-back — mirroring the Phase A spec's own acceptance note. CS2 is synced so
the acceptance path is ordered; a mixed sync/async fullscreen client can see an
async Skip overtake a held-back synced FLIP (pre-existing round-4 F6
behavior, not introduced here). Route the Skip through `scanout_m2.completed`
with `completion_mode=SKIP`, `emit_idle=true`.

**Wake-pin registration (adversarial review #13):** insert `retained_present_wakes`
at prepare time for queued frames; the chain submit must not re-insert the same
`present_id` (the existing insert is keyed by `present_id` and would silently
overwrite). Specify a single registration point.

**Teardown:** every `stop_direct_after_scanout_replaced` call site
(backend.rs:6783, 7447, 7619, 20656 — VT suspend, DPMS off, topology change,
shutdown) clears `queued` — complete as Skip or Copy per the path, always
releasing its pins and completing its event. Enumerate these alongside each
`scanout_m1.clear` in the implementation plan; the `Arc<DirectScanoutProbeFramebuffer>`
makes a queued frame safe across a clear, but its completion event must still
fire.

## State-transition table (adversarial review — missing section)

`P` = pending, `Q` = queued, `C` = current, `U` = unflip_requested,
`S` = pending_is_submitted.

| Event | Pre-state | Action | Post-state |
|---|---|---|---|
| eligible present arrives, `P.is_some() && !scene_flips` | P in flight, Q empty, U=false | prepare frame, store Q | P in flight, Q=new, U=false, hold=true |
| eligible present arrives, P in flight, Q exists | P in flight, Q=old | complete Q=old as Skip; store Q=new | P in flight, Q=new |
| eligible present arrives, scene flip in flight | P in flight (degraded window) | fall to Copy + unflip (no Q) | existing behavior |
| P flip retires, Q exists, U=false | P in flight, Q=frame | promote Q→P, S=false | P=promoted (unsubmitted), Q empty |
| P flip retires, Q exists, U=true | P in flight, Q=frame, U=true | complete Q as Skip; P→C | P empty, C=retired, unflip proceeds |
| P flip retires, Q empty | P in flight | P→C | C=retired |
| maybe_composite, P=promoted, S=false, U=false | P unsubmitted | submit P, S=true | P in flight |
| maybe_composite, P=promoted, S=false, U=true | P unsubmitted, U=true | complete P as Skip; composed unflip | P empty |
| cursor/overlay change | any with P in flight | U=true | U=true (retire decides Q fate) |
| chain submit fails | P unsubmitted | release pins, complete as Copy/Skip, reentry-blocked | P empty, composed path |
| stop_direct (teardown) | P or Q | complete Q as Skip/Copy, release pins | P/Q empty |

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

Unit coverage (fixture has no DRM — predicate-level, per the plan constraint):
- `present_flip_in_flight()` returns `true` with `scanout_m2.pending.is_some()`
  (extend `present_flip_in_flight_mirrors_scene_state`, backend.rs:32060).
- `present_completion_is_idle()` returns `false` while
  `scanout_m2.pending.is_some()` (new conjunct).
- Pure predicate for the queued-store decision:
  `direct_queued_store_eligible(pending_in_flight, scene_flip_in_flight) ->
  bool` = `pending_in_flight && !scene_flip_in_flight`.
- Pure predicate for the retire hand-off:
  `direct_chain_promote_eligible(queued_some, unflip_requested) -> bool` =
  `queued_some && !unflip_requested`.
- Pure predicate for the maybe_composite submit:
  `direct_chain_submit_eligible(pending_promoted, scene_flip_in_flight,
  unflip_requested) -> bool`.
- Core (yserver-core): a queued victim's Skip respects per-window `present_id`
  order for synced entries (reuse the existing ordered-delivery test
  machinery); async Skip is out of hold-back (already covered).

## Reference points

- `crates/yserver/src/kms/render/backend.rs:13627` — `present_flip_in_flight`.
- `crates/yserver/src/kms/render/backend.rs:13212` — the in-flight gate to replace.
- `crates/yserver/src/kms/render/backend.rs:1474` — `retire_direct_output`.
- `crates/yserver/src/kms/render/backend.rs:317` — `ScanoutM2State`.
- `crates/yserver/src/kms/render/backend.rs:301` — `DirectPresentFrame`.
- `crates/yserver-core/src/core_loop/process_request.rs:8602` —
  `successor_presents_full_extent` (already relaxed).
