# Present Deferred Execution + Supersession Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the approved spec
[`2026-07-31-present-deferred-execution-supersession-design.md`](../specs/2026-07-31-present-deferred-execution-supersession-design.md)
(revision 5, approved after 4 adversarial review rounds): defer Present
Copy execution toward the target MSC and supersede covered
same-window/same-target pending presents with `CompleteNotify{Skip}`, so a
vsync-off multi-buffer client (CS2: measured 180–200 fps, capped at
`swapchain_images × refresh`) recycles buffers at arrival rate (~300–370
fps expected on the measured HW) while vsync clients stay paced at refresh
(the 2026-07-27 ksplashqml fix must not regress).

**Branch:** `present-deferred-supersession` (feature branch off `master`
per AGENTS.md; squash merge with confirmation when done).

**Architecture (from the spec — do not re-litigate):**
1. Six new `Backend` trait capabilities (flip-in-flight, display-idle,
   absolute-arm-supported, absolute arm, scanout-blackout, source-pin
   token pair).
2. `run.rs` tail drain hoisted above `maybe_composite` (drain-before-
   compose), extracted as a testable `run_iteration_tail`.
3. One scheduling clock: `eff` and the due rule both use the **general**
   vblank clock (`present_get_ust_msc`); completion stamping/gate release
   keep the completion clock.
4. Core-side unified pending store keyed by `present_id` (+
   `wait_id → present_id` side map); every parked entry pins its source
   via an opaque backend token.
5. msc-due rule: late → execute at arrival; immediate target → execute at
   arrival iff no flip in flight, else park; future target → park +
   absolute sequence arm at `eff-1` (own per-CRTC set, `user_data`
   tagged), with flip-driven / idle-display / blackout / arm-failure
   fallbacks.
6. Supersession gated on update-region coverage (destination coords,
   `i32`), scrap = cancel wait + release pin + by-XID buffer release +
   fence mirror + `IdleNotify` now + parked `CompleteNotify{Skip}`.
7. Ordered delivery: `PendingPresentComplete` gains `mode`/`emit_idle`;
   the due arm routes through the queue; delivery is per-window
   `present_id` order with per-window hold-back across the three
   unresolved states.
8. Dead `present_scheduler` enqueues + path-selector helpers removed.

**Tech stack:** Rust; yserver-core (`backend/trait_def`, `server`,
`present_scheduler`, `core_loop/{run,process_request,process_disconnect}`),
yserver KMS backend (`kms/render/{backend,scene,telemetry}`,
`drm/page_flip`), yserver-protocol (`x11/present`), X11 Present
extension.

---

## Task 0: Xorg source verification items (a)–(d) — DONE 2026-08-01

**Resolved locally** against a shallow reference clone at
`~/Projects/xserver` (gitlab.freedesktop.org/xorg/xserver, commit
`5541a5c8befbb3975f69c1d7666a2ba76a57b682`); the sandbox machine was not
needed. Answers recorded in the spec's "Xorg verification items —
RESOLVED" section.

- [x] **(a)** Scrappable regardless of fence/acquire state
  (`present_scmd.c:802-823` checks only pixmap/queued/crtc/target_msc +
  a TearFree carve-out N/A to yserver); `present_vblank_scrap` signals
  `release_syncobj @ release_point` immediately at scrap
  (`present_vblank.c:218-239`). WAR hazard is Xorg-parity.
- [x] **(b)** Skip is stamped with the executing vblank's
  `(ust, crtc_msc)` — the target clock (`present_execute_post`,
  `present_execute.c:139-159`).
- [x] **(c)** Idle/release at scrap, `CompleteNotify{Skip}` at target,
  in per-window vblank-list order.
- [x] **(d)** Scrap is gated on the **successor having no update
  region** (`if (!update && pixmap)`, `present_scmd.c:802`) and ignores
  predecessor geometry. **Spec §Supersession updated accordingly:**
  scrap requires `B.update_rects == None`; the extent-coverage check
  stays as a conservative extra. Task 8 below reflects this.
- [x] Commit the spec/plan doc updates (branch
  `present-deferred-supersession`, first commit).

---

## Task 1: Wire test — `CompleteNotify` mode `Skip`

**Files:**
- Modify: `crates/yserver-protocol/src/x11/present.rs` (tests only —
  `encode_complete_notify` already takes `mode`; `COMPLETE_MODE_SKIP = 2`
  exists at `:243`).

- [ ] **Step 1:** Add a test beside `query_capabilities_reply_shape`
  asserting a `CompleteNotify` encoded with `COMPLETE_MODE_SKIP` carries
  mode byte 2 at offset 11, in **both** byte orders (mirror the existing
  LE test at `:550-561`).
- [ ] **Step 2:** `cargo test -p yserver-protocol present` → PASS (no
  production change expected; if the encoder hardcodes anything, fix it
  here).
- [ ] **Step 3:** Commit:
  `test(present): pin CompleteNotify Skip encoding both byte orders`.

---

## Task 2: Backend capability surface (six methods)

**Files:**
- Modify: `crates/yserver-core/src/backend/trait_def.rs` (near the
  present surface, `:694-724`)
- Modify: `crates/yserver/src/kms/render/backend.rs`

- [ ] **Step 1: Failing KMS unit tests** (in `backend.rs` tests, using
  `KmsBackend::for_tests()`): `present_flip_in_flight` mirrors
  `scene.has_pending_page_flips()`; `present_display_idle` is false when
  `scene_wants_compose()` even with no flips; `present_scanout_blackout`
  is true when `kms_outputs_active == false` even while
  `scanout_allowed()` (the DPMS case — round-4 F1a);
  `pin_present_source` returns a token that survives a `by_xid`
  invalidation (pin, drop the xid mapping, `release_present_source`
  still decrefs the original drawable exactly once).
- [ ] **Step 2:** Run to verify fail.
- [ ] **Step 3: Trait methods (default impls)** in `trait_def.rs`:

```rust
    /// A scanout pageflip is submitted and not yet retired. Used ONLY by
    /// the immediate-target arrival rule (spec §msc-due). Default false:
    /// non-KMS backends execute everything at arrival.
    fn present_flip_in_flight(&self) -> bool { false }
    /// No flip in flight AND nothing composing. Used ONLY by the
    /// idle-display fallback. Distinct from present_flip_in_flight: with
    /// the drain hoisted above maybe_composite, flips can be false while
    /// a compose is pending. Default true (nothing ever composes).
    fn present_display_idle(&self) -> bool { true }
    /// Kernel accepts absolute CRTC_QUEUE_SEQUENCE arming. Gates the
    /// idle-display fallback to flip-driven-clock drivers. Default false.
    fn present_absolute_vblank_arm_supported(&self) -> bool { false }
    /// Arm absolute vblank sequences for parked future-target presents.
    /// Own per-CRTC target set; must not consume or suppress the
    /// relative-1 idle arm. Default Ok(0).
    fn arm_present_absolute_vblank(&mut self, _targets: &[u64]) -> std::io::Result<usize> { Ok(0) }
    /// Display cannot scan out at all (VT-away OR DPMS-off). Gates the
    /// blackout flush. Default false.
    fn present_scanout_blackout(&self) -> bool { false }
    /// Pin a present source drawable by xid; resolves the xid ONCE and
    /// holds the DrawableId behind an opaque token (xid reuse / FreePixmap
    /// cannot re-point it). None if the xid does not resolve.
    fn pin_present_source(&mut self, _host_xid: u32) -> Option<u64> { None }
    fn release_present_source(&mut self, _pin_id: u64) {}
```

- [ ] **Step 4: KMS impls.** `present_flip_in_flight` →
  `self.scene.has_pending_page_flips()` (`scene.rs:861-865`);
  `present_display_idle` → the existing private
  `present_completion_is_idle()` (`backend.rs:5719-5721`);
  `present_absolute_vblank_arm_supported` →
  `!self.crtc_queue_sequence_unsupported` (`:384`);
  `present_scanout_blackout` →
  `!(self.scanout_allowed() && self.kms_outputs_active)` (`:5697/:416` —
  the complement of `next_wakeup`'s `allow_kms_timers`, `:11244`);
  pin/release → a `HashMap<u64, DrawableId>` + monotonic token counter,
  `store.incref` at pin, `store_decref_with_invalidate` at release
  (model: `PendingPresentSourceWait.source_id`, `:11544-11551/:11678`).
  `arm_present_absolute_vblank` stays `Ok(0)` until Task 3.
- [ ] **Step 5:** Run tests → PASS. `cargo clippy --all-targets -- -D
  warnings`.
- [ ] **Step 6:** Commit:
  `feat(present): backend capability surface for deferred execution`.

---

## Task 3: KMS absolute vblank arm (own set + `user_data` tag)

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs`
  (`arm_idle_vblanks_with` `:5844-5882`, `on_crtc_sequence_event`
  `:5783`, `clear_all_armed_vblank_targets` `:5742`)
- Modify: `crates/yserver/src/drm/page_flip.rs` — the event decode
  truncates `user_data` to `u32` (`:284-286`,
  `let crtc_id_raw = ev.user_data as u32`) before the tag bit could be
  observed; the `on_sequence` closure type (`FnMut(u32, i64, u64)`) must
  carry the untruncated `u64 user_data` (doc comment `:240-245`).
- Modify: `crates/yserver/src/kms/render/platform.rs` —
  `SequenceCompletion` (`:499-507`) gains a `user_data: u64` field,
  populated in `drain_page_flip_events`'s closure (`:1461-1469`).
  `on_crtc_sequence_event`'s signature and both call sites
  (`backend.rs:11147/:11173`) change accordingly.
  (Scope expanded 2026-08-01: the original "Reference-only" listing of
  `page_flip.rs` was wrong — implementation stop-report confirmed the
  full `user_data` never reaches `backend.rs` without this plumbing.)

- [ ] **Step 1: Failing tests:** (i) an absolute-arm sequence event does
  **not** clear the relative `armed_vblank_targets` slot; (ii) a
  relative-arm event does not retire absolute targets; (iii) absolute
  targets with `target <= sequence` retire on a tagged event; (iv)
  `clear_all_armed_vblank_targets` clears both sets; (v) arming a target
  already passed still fires (`NEXT_ON_MISS` is unconditional,
  `page_flip.rs:124-128`).
- [ ] **Step 2:** Run to verify fail.
- [ ] **Step 3: Implement.**

```rust
/// user_data tag for absolute (per-target) sequence arms. The kernel
/// event carries no CRTC; the low 32 bits stay the crtc_id (matching the
/// relative arm at backend.rs:5885), the high bit discriminates the arm
/// kind so on_crtc_sequence_event can tell them apart (spec round-4 F2).
const ABSOLUTE_SEQ_TAG: u64 = 1 << 63;
```

  - New field `absolute_vblank_targets: HashMap<u32 /*crtc*/, BTreeSet<u64>>`.
  - `arm_present_absolute_vblank(&[u64])`: for each connected CRTC with a
    flip-driven... no — arm on the **selected output's CRTC set** exactly
    like `arm_idle_vblanks_with` iterates; skip targets already armed;
    `queue_crtc_sequence(dev, crtc, /*relative*/ false, target, ABSOLUTE_SEQ_TAG | u64::from(crtc_id))`
    (the `-1` is core-side per the spec — `arm_present_absolute_vblank`
    arms exactly the values it receives).
    On `EOPNOTSUPP`: latch `crtc_queue_sequence_unsupported` (same latch
    as `:5899-5906`) and return the error — core's caller handles the
    execute-immediately fallback (Task 7).
  - `on_crtc_sequence_event`: step 1 (relative-slot clear, `:5772-5776`)
    applies **only** when `ev.user_data & ABSOLUTE_SEQ_TAG == 0`; tagged
    events remove retired entries (`target <= sequence`) from
    `absolute_vblank_targets`. Clock recording (`:5807-5818`) stays
    unconditional for both.
  - `clear_all_armed_vblank_targets` clears both maps.
- [ ] **Step 4:** Run tests → PASS; clippy.
- [ ] **Step 5:** Commit:
  `feat(kms): per-target absolute vblank arm with user_data tag`.

---

## Task 4: `run.rs` tail extraction + drain-before-compose reorder

**Files:**
- Modify: `crates/yserver-core/src/core_loop/run.rs` (tail
  `:1265-1275`; harness `:3198-3310`)
- Modify: `crates/yserver-core/src/backend/recording.rs` (instrumentation;
  `mark_dirty`/`maybe_composite` defaults are no-ops,
  `trait_def.rs:583/:603`)

- [ ] **Step 1:** Extract the loop-body tail (deferred-input poll →
  `maybe_composite` → `drain_present_completions`) into
  `fn run_iteration_tail(state: &mut ServerState, backend: &mut dyn Backend)`
  — mechanical, no order change yet. Existing tests stay green.
- [ ] **Step 2:** `RecordingBackend` gains `maybe_composite_calls: usize`
  and `mark_dirty_calls: usize` (recorded overrides) plus a hook to make
  `drain_completed_present_events` return a canned completion.
- [ ] **Step 3: Failing test:** a completion drained in the tail whose
  execution marks dirty must be observed by the **same** tail's
  `maybe_composite` call (assert call order: drain's `mark_dirty`
  precedes the `maybe_composite` invocation within one
  `run_iteration_tail`). Fails against the current order
  (`maybe_composite` at `:1272` before drain at `:1275`).
- [ ] **Step 4:** Reorder inside `run_iteration_tail`:
  `drain_present_completions` **before** `maybe_composite`. (The epfd
  call site at `run.rs:1027` is already pre-compose — untouched. Safety:
  `maybe_composite` is `fn(&mut self)` with no `ServerState` access,
  `backend.rs:11288`.) Test → PASS.
- [ ] **Step 5:** Full `cargo test -p yserver-core` + clippy (watch for
  order-sensitive tests; fix only true order assumptions, none are
  expected per round-4 review).
- [ ] **Step 6:** Commit:
  `feat(core-loop): drain present completions before compose (testable tail)`.

---

## Task 5: Unified pending-present store (no behavior change yet)

**Files:**
- Modify: `crates/yserver-core/src/server.rs` (`pending_present_pixmaps`
  is `HashMap<u64 /*wait_id*/, PendingPresentPixmap>` today, `:1011`)
- Modify: `crates/yserver-core/src/core_loop/process_request.rs`
  (`drain_ready_present_pixmaps` `:8695-8719`, arm sites, teardown
  `:1374-1454`)
- Modify: `crates/yserver-core/src/core_loop/process_disconnect.rs`
  (`:299-330`)

- [ ] **Step 1: Types.** `PendingPresentEntry { pending: PendingPresentPixmap,
  source_ready: bool, wait_id: Option<u64>, pin: Option<u64> }`;
  `ServerState.present_pending_exec: BTreeMap<u64 /*present_id*/, PendingPresentEntry>`
  (BTreeMap: per-window smallest-id scans in Task 6 iterate in id order);
  `ServerState.present_wait_to_id: HashMap<u64, u64>`.
- [ ] **Step 2: Re-route the existing source-wait parking** through the
  store: on `Deferred`, insert the entry (`source_ready: false`,
  `wait_id: Some(..)`, `pin: backend.pin_present_source(src_host_xid)`)
  and the side-map row; `drain_ready_present_pixmaps` resolves
  `wait_id → present_id`, sets `source_ready`, and — since msc-due does
  not exist yet — executes immediately, releasing the entry pin after
  `execute_present_pixmap_copy` (the wait pin is dropped by
  `finish_present_source_wait` at `:8712` exactly as today; the two pins
  are distinct — spec §store).
- [ ] **Step 3: Purges.** Window-destroy (`:1374-1454`), disconnect
  (`process_disconnect.rs:299-330`), and shutdown (before sockets close,
  next to the `signal_all_retained_present_wakes` call, `lib.rs:603`)
  walk the store: release by XID (idle fence trigger / syncobj signal) +
  `sync_fences` mirror + release pin + drop entry + side-map row.
  Failing tests first: parked entry × {window destroy, disconnect,
  shutdown} → exactly one release each; `FreePixmap` between park and
  drain → copy still executes against the pinned drawable (or purges
  cleanly), never a dead-xid copy.
- [ ] **Step 4:** `cargo test -p yserver-core present` + existing
  source-wait suites green; clippy.
- [ ] **Step 5:** Commit:
  `refactor(present): unified pending store keyed by present_id (+ source pins)`.

---

## Task 6: Ordered completion delivery

**Files:**
- Modify: `crates/yserver-core/src/server.rs` (`PendingPresentComplete`
  `:819-822`)
- Modify: `crates/yserver-core/src/core_loop/process_request.rs`
  (`fire_present_completion_events_at` — mode hardcoded at `:9680`,
  unconditional IdleNotify `:9650-9670`; `fire_due_present_completions`
  `:9893-9917`)
- Modify: `crates/yserver-core/src/core_loop/run.rs` (due arm
  `:1324-1339`)

- [ ] **Step 1: Failing tests** (harness + `RecordingBackend`):
  - **P1/P2/P3 vector:** P1 executes at arrival (gate inserted), P2
    parks, P3 scraps P2 → `Skip(P2)` parked; P1's completion drains
    late; delivery must be `Copy(P1)` then `Skip(P2)` (today's code
    would invert).
  - **Uncovered-survivor vector:** uncovered A executes at `T`, covered
    B scrapped → `Skip(B)` never before `Copy(A)`.
  - Parked Skip emits **no second** `IdleNotify` (`emit_idle: false`).
  - Per-window hold-back only: window X's stalled copy does not delay
    window Y's due completions.
  - Async present (`eff == None`) completes immediately even with an
    earlier parked synced present for the same window (documents the
    exemption — spec round-4 F6).

  (Until Task 8 exists, drive the "scrap" inputs by directly inserting
  parked-Skip rows; Task 8 re-runs these end-to-end.)
- [ ] **Step 2:** Extend `PendingPresentComplete` with `mode: u8` and
  `emit_idle: bool`; thread through `fire_present_completion_events_at`
  (skip the IdleNotify block when `emit_idle == false`; use `mode`
  instead of `COMPLETE_MODE_COPY`). All existing constructors set
  `mode: COMPLETE_MODE_COPY, emit_idle: true`.
- [ ] **Step 3:** Route the due arm through the queue: `run.rs:1324-1339`
  pushes into `present_pending_complete` instead of firing;
  `fire_due_present_completions` runs in the same drain pass (already
  does, `:1365`).
- [ ] **Step 4:** Rewrite the sweep (spec round-4 F4): per window,
  deliver the smallest unblocked `present_id` first. Blocked = a smaller
  `present_id` for the same window exists in `present_pending_exec`
  (unexecuted), `present_complete_gate` (executed-but-undrained; it
  carries `dst_window_xid`, `server.rs:824-828`), or in the queue
  not-yet-due. Implementation: group due entries by window, deliver in id
  order, stop a window's group at its first blocker; O(n log n) worst
  case is fine at these rates.
- [ ] **Step 5:** Tests → PASS; clippy.
- [ ] **Step 6:** Commit:
  `feat(present): per-window ordered completion delivery (Skip-capable queue)`.

---

## Task 7: msc-due classification + deferral

**Files:**
- Modify: `crates/yserver-core/src/present_scheduler.rs` (pure
  classification fn + tests)
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` (both
  Pixmap handlers; eff sites `:9043-9050/:9350-9366`)
- Modify: `crates/yserver-core/src/core_loop/run.rs` (due-pass in the
  drain; third arming call site beside `:1378/:1399`)

- [ ] **Step 1: Pure classifier + failing tests:**

```rust
pub enum MscDue { ExecuteNow, Park }
pub fn classify_msc_due(eff: Option<u64>, clock_msc: u64, flip_in_flight: bool) -> MscDue
```

  Vectors: `None` (async/no-clock) → ExecuteNow; `eff <= clock` →
  ExecuteNow; `eff == clock+1 && !flip_in_flight` → ExecuteNow;
  `eff == clock+1 && flip_in_flight` → Park; `eff > clock+1` → Park;
  wrap-safe via `msc_is_after`.
- [ ] **Step 2: Arrival evaluation** (in the request handlers, at the
  point today's code calls `execute_present_pixmap_copy` /
  arms the source wait): source-ready entries classified `Park` go into
  the store (pin taken) instead of executing; `ExecuteNow` keeps today's
  path. **The arrival evaluation happens at request-processing time**
  (request drain runs before the tail — a lone present on an idle display
  executes at arrival, it never waits for the tail).
- [ ] **Step 3: Due-pass** at the top of `drain_present_completions`
  (which Task 4 moved pre-compose): re-classify parked source-ready
  entries against the fresh general clock; execute due ones
  (release entry pin after copy). Fallback ladder for still-parked
  future targets, exactly per spec:
  - `present_absolute_vblank_arm_supported()` → third arming call site:
    `backend.arm_present_absolute_vblank(&targets_minus_one)` — **not**
    routed through `arm_present_completion_idle_vblanks` (its idle-only
    gate at `backend.rs:17885-17893` would suppress it). If the arm
    returns `Err` or `Ok(0)` (including the latch-trip iteration), the
    affected entries execute immediately (`trigger=idle_fallback`).
  - Flip-driven drivers: entries become due as flips advance the clock;
    if `present_display_idle()` (both terms), execute immediately.
  - `present_scanout_blackout()`: execute parked entries **and** deliver
    everything in `present_pending_complete` with the current (frozen)
    clock stamp (`trigger=blackout`).
- [ ] **Step 4: Failing tests:** future-target present produces no copy
  and no damage until due; parked immediate-target executes on the
  flip-retirement wakeup and lands in that iteration's compose (uses
  Task 4 instrumentation); idle-display rule requires both predicate
  terms and only on `!arm_supported` backends; blackout flushes both
  halves; arm failure → same-drain execution; no-reclassification test
  (present arriving between an active sequence sample and a flip retire
  keeps `eff == clock+1` — one-clock contract); combined
  source-wait + msc-due executes only when both hold.
- [ ] **Step 5:** Tests → PASS; clippy. `just yserver-mate-hw-telemetry
  "info,present_pace=debug"` smoke on this box: desktop still paints,
  vsync clients paced.
- [ ] **Step 6:** Commit:
  `feat(present): defer Copy execution to target MSC (arrival/due rule)`.

---

## Task 8: Supersession (coverage-gated scrap + parked Skip)

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` (both
  Pixmap handlers, after eff computation; coverage helper + tests)

- [x] **Step 1: Pure coverage predicate + failing tests** (destination
  coords, `i32`; on the **resolved** `update_rects:
  Option<Vec<RegionRect>>`, `server.rs:1687` — not raw `req.update`,
  which yields `None` for unresolvable regions too,
  `:9024-9034/:9340-9349`). **Successor gate first (Task 0 item (d),
  Xorg `present_scmd.c:802`): scrap is attempted only when
  `B.update_rects == None`** — a successor carrying rects (or the
  zero-pixel `Some(empty)`) never scraps. Then predecessor coverage by
  `B`'s full-extent rect `(x_off, y_off, src_w, src_h)` (source-extent
  bounded, `:8630-8642`):
  - predecessor `None`: covered iff its own extent rect fits;
  - predecessor `Some(rects)`: covered iff every dest rect fits;
  - predecessor `Some(empty)`: trivially covered.

  Vectors: full-over-full; full-over-sliver (scraps — Xorg semantics);
  sliver successor (declines regardless of coverage — the marco
  pattern); negative offsets; full-frame successor from a smaller
  mid-resize source not covering a larger predecessor (declines — the
  conservative extra); zero-pixel both roles.
- [x] **Step 2: Scrap at arrival** (after classification, before
  park/execute of `B`): when the successor gate passes, scan
  `present_pending_exec` for same-window, same-`eff`, unexecuted,
  covered entries `A`:
  1. `finish_present_source_wait(wait_id)` if armed; release `A`'s pin;
  2. by-XID release: `dri3_trigger_fence(idle_fence)` /
     `dri3_signal_syncobj(release_syncobj, release_value)` +
     `state.sync_fences[..].triggered = true` (cf. `:9880-9887`) +
     `IdleNotify` now;
  3. park `CompleteNotify` `{mode: Skip, emit_idle: false}` keyed to `T`
     in `present_pending_complete`;
  4. drop the entry + side-map row.
- [x] **Step 3: Copy-failure reroute** (round-4 F5): the
  `execute_present_pixmap_copy` error path releases by XID immediately
  (like scrap) and parks `{mode: Copy, emit_idle: false}` in the ordered
  queue instead of firing on the spot. Failing test: a failing copy
  neither strands the client nor inverts per-window order.
- [x] **Step 4:** Re-run the Task 6 vectors end-to-end (real scrap
  driving them now); uncovered same-target entries execute in arrival
  order; distinct targets never supersede; scrap × window-destroy races
  release exactly once.
- [x] **Step 5:** Tests → PASS; clippy.
- [x] **Step 6:** Commit:
  `feat(present): same-target supersession with CompleteNotify Skip`.

---

## Task 9: Dead scheduler removal

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs`
  (enqueues `:9120/:9424`; teardown drain `:1443`; helpers
  `present_path_for` `:9493`, `present_path_for_synced` `:9510`,
  `build_path_selector_inputs` `:9531`)

- [x] **Step 1:** Delete the two enqueues, the teardown
  `present_scheduler.drain_window` signalling remnants that the
  2026-07-27 plan already gutted, and the three now-unreferenced
  path-selector helpers (`present_scheduler::choose_path` is `pub` with
  its own tests — survives).
- [x] **Step 2:** `cargo clippy --all-targets -- -D warnings` (this is
  the step that catches any other newly-dead code) + full core tests.
- [x] **Step 3:** Commit:
  `chore(present): remove dead informational scheduler enqueues`
  (closes the 2026-07-25 1,676-syncobj teardown finding's source).

---

## Task 10: Telemetry

**Files:**
- Modify: `crates/yserver-core/src/core_loop/{process_request,run}.rs`
  (`present_pace` target lines)
- Modify: `crates/yserver-core/src/backend/trait_def.rs` +
  `crates/yserver/src/kms/render/telemetry.rs` (`present_skips/s`)

- [x] **Step 1:** `present_pace` debug lines per spec:
  `stage=parked_msc … reason=flip_in_flight|future`,
  `stage=exec_due … trigger=arrival|drain|sequence|idle_fallback|blackout`,
  `stage=superseded`, `stage=supersede_declined`.
- [x] **Step 2:** `Backend::note_present_skip()` (default no-op); KMS
  increments a telemetry counter emitted as `present_skips/s` in the
  `render_telemetry` line. Core calls it at scrap.
- [x] **Step 3:** Build + clippy; commit:
  `feat(present): pacing/supersession telemetry`.

---

## Task 11: Full workspace verification

- [ ] `cargo +nightly fmt`
- [ ] `cargo clippy --all-targets -- -D warnings` (CI-exact — catches
  test-code lints)
- [ ] `cargo test --workspace`
- [ ] Commit any fmt churn: `chore(present): fmt`.

---

## Task 12: Hardware verification (the real gate)

Unit-green is not proof (memory: `feedback_tests_are_not_visible_evidence`).

**Scope decision 2026-08-01 (user):** the PR is gated on the
**software-behaviour** half of this matrix — client compatibility,
compositor regressions, and pacing on machines we have. The items that
need hardware we do not control are explicitly deferred and called out
in the PR body rather than silently skipped:

- *deferred* — **NVIDIA legacy 1050 Ti**: the maintainer's box; ask in
  the PR for a run there.
- *deferred* — **dual-head**: needs a second output.

Everything else below is in scope for this PR.

- [x] **CS2 / this box (nvidia):** DONE 2026-08-01 12:54 (Task 13
  gate-relaxation build). Cap collapsed; copies-per-compose mode 4 → 1;
  `copy_area` 299,307 → 31,945 while presents only fell 378,872 →
  150,111; executed copies ~64/s vs 60 Hz flip (~1.06×);
  `present_skips/s` sustained 140–200; zero backward serials in 150,110
  deliveries; accounting balances exactly (83,120 skip + 63,529
  exec_due + 3,461 source_ready = 150,110 fired); no stalls (mean
  inter-present gap 4.46 ms). marco = 0 skips / 100 % copies, confirming
  the successor gate declines partial-region presents.
- [x] **Hollow Knight / this box — unplanned, and the strongest run so
  far** (2026-08-01 22:20, ~8 min). A vsync-OFF client free-running at
  **~1000 presents/s** against a 60 Hz output, with marco compositing:
  - 480,998 presents, **422,195 superseded (87.8 %)**;
  - executed copies land at **120–121/s = exactly 2× the flip rate**,
    the number this plan predicted ("→ ~2× flip rate"): 60 from marco
    (which never scraps — partial regions) + ~60 from the game, one per
    vblank after supersession collapses each burst;
  - **marco itself stays at exactly 60/s every single minute** — the
    vsync-paced population is NOT accelerated by the deferral, which is
    direct evidence against the ksplashqml regression this matrix
    worries about (same mechanism: a synced client must stay at
    refresh);
  - **zero backward serials in 480,997 deliveries** across both clients;
  - accounting balances exactly (480,998 requests / 480,997 fired = one
    in flight at cutoff) — no leaked or double-delivered completions;
  - survived the monitor being physically powered off and back on
    mid-game with no instability.
- [ ] **ksplashqml / bee (Plasma):** stays ~60 fps — the 2026-07-27
  fix's original target; up to one period added scanout latency for
  flip-in-flight arrivals is accepted (Xorg parity). *Partially covered
  by the marco-at-60/s result above (same synced-present path); still
  worth the specific client. Plasma is not installed on this box and
  building it on Gentoo costs the whole Qt6+KF6 stack, so this is a good
  candidate to ask the maintainer for.*
- [ ] **mpv / bee + silence:** Pixmap and PixmapSynced, windowed +
  fullscreen, compositing on/off — smooth, no early-frame fast-forward,
  complete release progress.
- [ ] **marco/MATE dogfood** with `YSERVER_PRESENT_TRACE` +
  `supersede_declined`: no drag-smear regression; record whether slivers
  ever collide on effective target (item (d) relaxation data).
- [ ] **Dual-head / silence:** window on the idle output presents
  correctly while the other plays video; no anomaly beyond documented
  global-clock behavior.
- [ ] **DPMS-off blackout** with an actively presenting client: both
  halves flush (`trigger=blackout`), buffers cycle, clean resume.
- [ ] **NVIDIA legacy (maintainer's 1050 Ti, closed proprietary
  driver):** same WSI synced-present behavior and same `nvidia-drm`
  QUEUE_SEQUENCE limitation as the open-modules box, so the fix should
  apply identically — verify a vsync-off Vulkan client free-runs and
  the desktop stays correct on whichever Present path that driver
  branch uses (xshmfence `Pixmap` vs explicit-sync `PixmapSynced`
  differs by driver version; both paths must behave).
- [ ] **Warframe + Steam** regression set from the 2026-07-27 plan Task
  10 (cursor lag, pbuffer-render, black-until-damaged).
- [ ] **DE dogfood** (MATE/XFCE/Cinnamon): no interactivity/input-lag
  regression.
- [ ] Update `docs/status.md` "Where we are" + refresh the
  `present-supersession-spec-status` memory; then squash-merge with user
  confirmation (AGENTS.md).

---

## Task 13: Successor-gate relaxation (spec amendment 2026-08-01)

Added after the first Task 12 CS2 capture: `present_skips/s` was zero
for the whole session because NVIDIA's WSI attaches a full-extent
single-rect update region (`(0,0,1920,1080)` on every one of 95,119
game presents — `YSERVER_PRESENT_TRACE` capture, 2026-08-01 12:11) and
the Xorg-literal successor gate declines any region. Spec §"Amendment
2026-08-01 — successor-gate relaxation" is the contract: the gate now
also accepts `Some(rects)` where **one single rect contains the full
source extent** (`r.x <= 0 && r.y <= 0 && r.x + r.width >= src_width
&& r.y + r.height >= src_height`, `i32`, pixmap coordinates). No union
coverage. Everything downstream (predecessor coverage vs the successor
extent, scrap mechanics, Skip delivery, telemetry) unchanged.

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs`
  (`present_supersession_covers`'s successor gate, the damage arm of
  `execute_present_pixmap_copy`, `supersede_covered_pending_presents`'s
  decline-telemetry guard + comment, tests)

- [x] **Step 0: Refit two existing Task 8 tests whose fixtures are
  accidentally full-extent** — their *assertions* must be preserved
  (they are the only regression coverage for "a partial-region
  successor never scraps", the property the amendment's safety argument
  rests on). `coverage_sliver_successor_never_scraps_regardless_of_coverage`
  and `supersede_successor_with_update_rects_never_scraps` both use a
  successor region `(0,0,100,100)` on a `100x100` source — post-
  amendment that passes the gate and the asserts go red. Change the
  fixtures to a genuinely partial rect (e.g. `(10,10,5,5)`), rename to
  `..._partial_region_successor_...`, KEEP the assertions. Do NOT flip
  assertions on the old fixtures.
- [x] **Step 1: Failing tests** (beside the existing coverage vectors),
  per spec §Validation amendment bullet: single-rect full-extent region
  scraps like `None`; over-large rect passes; superset region
  (`[full-extent, sliver]`) passes; partial single rect declines;
  multi-rect union-but-no-single-rect declines; region full-extent in
  one axis on a taller pixmap declines; `Some([zero-area rect])`
  declines; `Some(empty)` declines. The `SupersessionFixture` builder
  hardcodes `update: 0` in the request even when `update_rects` is
  `Some` — add an `update` field so damage-arm tests can exercise the
  real branch.
- [x] **Step 2: Gate implementation.** Factor the successor gate into a
  named helper (`fn successor_presents_full_extent(&PendingPresentPixmap)
  -> bool` or similar) — MANDATORY, not optional: it must be the single
  source of truth used by BOTH `present_supersession_covers` and the
  `supersede_declined` telemetry guard, whose current guard
  (`update_rects.is_none()`) and comment ("the successor cleared the
  Xorg gate (no update region)") both become wrong otherwise. Update
  that comment. Rule exactly as the spec amendment writes it (explicit
  `i32::from` on all four terms).
- [x] **Step 3: Damage-arm fix** (spec amendment §"Damage-arm
  dependency"): in `execute_present_pixmap_copy`'s damage accumulation,
  translate per-rect damage by `x_off`/`y_off`
  (`x_off.saturating_add(rect.x)`, matching the copy arm), and re-key
  the branch off `update_rects.is_none()` instead of the raw
  `update != 0` (unresolvable region → full-extent copy must damage
  full-extent, not nothing). Failing test first: a full-extent-region
  present with `x_off/y_off != 0` accumulates damage at the translated
  position; an `update != 0` + unresolvable-region present accumulates
  full-extent damage.
- [x] **Step 4:** Re-run the Task 8 e2e vectors + full
  `cargo test -p yserver-core`; clippy CI-exact.
- [x] **Step 5:** Annotate sub-hypothesis #2 of
  `docs/superpowers/findings/2026-07-08-mate-compositor-drag-smear-diagnosis.md`
  as DISPROVEN (region is pixmap-relative; Xorg `present.c:76-92` clip
  origin) with a pointer to the spec amendment.
- [x] **Step 6:** Commit:
  `feat(present): accept full-extent update regions in the supersession successor gate`.
- [ ] **Step 7 (user): repeat the CS2 capture.** Acceptance criteria
  (spec amendment): `present_skips/s > 0` sustained and copies/compose
  → 1–2 (NOT a fixed 4/5 skip ratio — mid-burst vblanks legitimately
  split targets); multi-minute session with no swapchain stall/hitch,
  no stale/torn content, `IdleNotify` count == present count, and
  per-window serial monotonicity. The gate relaxation is one predicate,
  revertible independently of Tasks 0-11 if the closed WSI's
  swap accounting misbehaves on first-ever Skip completions.

---

## Non-blocking notes (tracked, not silently dropped)

- **Coverage relaxation** to Xorg's (possibly region-insensitive) scrap
  waits on Task 0 item (d) + the marco collision data — follow-up, not
  this branch.
- **Flip path** (`PresentCaps.flip_path`) remains the route to full ~400
  fps Xorg parity — separate program ("alien-BO scanout integration").
- **Per-output clocks / window→output mapping** stay out of scope; the
  global most-advanced-output semantics are documented in the spec.
- **NVIDIA future-target early-show concession** (no absolute arm there)
  is by design; if the `kernel_frame=` capture shows `software_msc`
  (per-flip counting), append the observed value to the spec.

## Self-Review

- Spec §Loop-order → Task 4 (reorder + testable tail) and Task 7 Step 4
  (no-reclassification, one-clock). §Capability surface → Task 2 (six
  methods, token pin). §Absolute arm → Task 3 (own set, tag, clear-both)
  + Task 7 (call site, failure fallback). §Store → Task 5 (present_id
  key, side map, pins, purges). §msc-due → Task 7. §Supersession →
  Task 8 (coverage, scrap, fence mirror, copy-failure reroute).
  §Ordered delivery → Task 6 (mode/emit_idle, due-arm reroute,
  per-window sweep). §Dead scheduler → Task 9. §Lifecycle → Tasks 5/7/8
  (purges, blackout). §Telemetry → Task 10. §Validation unit bullets all
  appear as failing-test steps; HW bullets → Task 12.
- **Release-exactly-once populations:** never-executed parked entries
  release by XID only (scrap/purge — no gate, no PinnedWake);
  executed entries release via `signal_present_wake` at delivery
  (unknown-id no-op guards the boundary, `backend.rs:17855-17857`);
  copy-failure releases by XID then delivers `emit_idle: false`. Entry
  pin vs wait pin are distinct creations with distinct releasers.
- **Order of tasks is always-green:** Tasks 1–6 change no client-visible
  pacing (store still executes at source-ready; queue delivers same
  events in corrected order); Task 7 flips on deferral with the
  fallbacks in the same commit; Task 8 adds scrap on top of an
  already-ordered queue.
