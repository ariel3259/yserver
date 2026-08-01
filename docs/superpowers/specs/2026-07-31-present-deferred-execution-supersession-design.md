# Present deferred Copy execution + same-target supersession (Skip)

**Status:** **approved with changes applied** — revision 5 after four
adversarial review rounds (independent Claude sessions, 2026-07-31/08-01;
round 1 Fable, rounds 2–4 Opus). Round 4 verdict: approve-with-changes,
"with those applied the spec is implementable without further design
decisions"; its exhaustive edit list (F1–F6 + citation drift) is applied
below. Ready for the implementation plan. This is the "Skip-collapse" follow-up that
the vblank-pacing plan
(`../plans/2026-07-27-present-complete-vblank-pacing.md`, Non-blocking notes)
anticipated; the measured evidence below is the data that plan's Task 10
Step 2 asked to record.

## Problem — measured on CS2, nvidia box (open kernel modules, 1920x1080@60)

The 2026-07-27 pacing work ported Xorg's `present_get_target_msc` math and
parks the client wake (idle xshmfence / release syncobj) plus
`IdleNotify`/`CompleteNotify` until the request's effective target MSC
(`run.rs` drain → `present_complete_gate` → `signal_present_wake`). It fixed
ksplashqml free-running at ~500 fps, but it paces **every** synced present —
including a vsync-off game submitting `target_msc=0` from a multi-image
Vulkan swapchain. Such a client's buffers are only returned at vblank
granularity, so its frame rate is capped at `swapchain_images × refresh`.

Hardware capture (`yserver-mate.submit.tsv`, 2026-07-31, CS2 under MATE on
the nvidia box). The 2026-07-30 boot log from the same box records
`DRM_IOCTL_CRTC_QUEUE_SEQUENCE returned EOPNOTSUPP — disabling idle vblank
arming (flip-driven MSC only)`, so this driver exercises the flip-driven
clock arm of the design below. (Whether its flip-driven MSC is a true
vblank counter or the per-flip `software_msc` fallback —
`platform.rs:1490-1503`, kernel `frame == 0` — must be recorded from the
`kernel_frame=` debug line during implementation validation; the
distinction changes how far ahead a future-target present can run there.)

- During gameplay, inter-compose intervals contained exactly 3 or 4
  `copy_area` submits to the game's backing pixmap (60 s window: 2,400
  threes, 1,199 fours, nothing else; full capture: 23,876 threes, 13,508
  fours, other counts only in non-gameplay phases). Average 3.33 × 60 Hz
  ≈ 200 presents/s; the dominant 3-per-vblank regime is the user-visible
  180 fps.
- Copy timestamps are phase-locked to the compose clock (peaks ~2.7 ms
  apart within each vblank period): the game renders a frame in ~2.7 ms
  (≈370 fps capability, consistent with ~400 fps on Xorg/NVIDIA), burns the
  3–4 buffers released at the vblank, then blocks in acquire.
- The NVIDIA Vulkan WSI does not set `PresentOptionAsync` here (yserver
  advertises `QueryCapabilities` without `PresentCapabilityAsync`;
  `PresentCaps::encode` ties Async to `flip_path`,
  `trait_def.rs:139-141`), so the async escape Mesa-on-Xorg uses for
  vsync-off clients never engages.

Xorg does not have this cap even for synced copy presents, because of a
mechanism yserver has not ported yet: **same-window/same-target
supersession**. When a new `PresentPixmap` arrives for a window and an
earlier one is still queued (not yet executed) for the same target MSC,
Xorg scraps the earlier one — idles its pixmap immediately and completes it
with mode `Skip` (`present_scmd.c:797` area). A vsync-off client therefore
recycles buffers at arrival rate, while a vsync client (Mesa spreads
`target_msc` across frames, so targets never collide) stays paced at
refresh. One mechanism serves both populations.

A second divergence makes the straightforward "supersede at the completion
gate" insufficient: yserver executes the GPU copy **eagerly** at request
time (the 2026-07-27 plan: "the GPU copy stays eager; only the client-visible
wake+events are paced"), while Xorg executes the copy at the target vblank.
Eager execution:

1. violates `presentproto`'s "the presentation will occur no earlier than
   target-msc" for any present with a future target (the content lands in
   the window backing immediately and the next compose shows it early —
   only the *events* wait);
2. would make `Skip` a lie: an eagerly-copied present may already have been
   composed to the screen before it is superseded, and no Xorg-tested
   client has ever seen "shown but reported Skip";
3. costs a full-window GPU copy plus the per-present completion chain
   (`enqueue_present_completion`, `backend.rs:17702`: batch flush + open-frame
   close + submit-group flush + semaphore/fence acquire + signal-only
   `vkQueueSubmit` + SYNC_FD export + epoll registration) for every present —
   at CS2's free-run rate that is 300–400 such chains/s on the
   single-threaded server, competing with input dispatch and the ~60 Hz
   compose on the same thread and VkQueue.

## Design

Defer Copy execution toward the target MSC, Xorg-style, and supersede
covered same-window/same-target pending presents with `CompleteNotify`
mode `Skip`.

### Loop-order and clock contract (load-bearing)

Two structural changes to `run.rs` are **part of this design**, not
implementation detail:

1. **Drain-before-compose reorder.** `drain_present_completions` has two
   call sites: the epfd-driven one under `PRESENT_COMPLETION_TOKEN`
   (`run.rs:1027`), which already runs before compose, and the
   iteration-tail one at `run.rs:1275`, which runs **after**
   `backend.maybe_composite()` at `run.rs:1272` — after the iteration's
   last compose opportunity. A parked present executed in the tail drain
   would therefore miss the compose submitted three lines earlier whenever
   *any* unrelated damage existed (second client, cursor, compositor
   paint), slipping a full period. The reorder applies to the **tail call
   site**: it — and with it the whole `drain_present_completions`,
   including the existing `drain_ready_present_pixmaps` source-wait path,
   which has the same latent property there today — is hoisted **above**
   `maybe_composite()`, so an entry executed in iteration *i* is composed
   by iteration *i*'s scene build. The swap is safe: `maybe_composite` is
   `fn(&mut self)` with no `ServerState` access (`backend.rs:11288`) and
   mutates no state the drain reads.

   **Wakeup liveness (recorded so it is not re-derived):**
   `on_page_flip_ready` (`backend.rs:11123-11202`) retires flips and
   advances the clock but never submits the next flip — the resubmit is
   `maybe_composite`. With the hoist, the sequence on a DRM event is:
   retire + clock advance → drain (executes due entries, `mark_dirty`) →
   `maybe_composite` composes them into *that* flip. No parked entry can
   sleep forever: a park always happens inside an iteration whose drain
   runs later in that same iteration, and every park reason has a
   guaranteed wake (flip retire → DRM fd; absolute sequence arm → DRM fd;
   compose-pending → `next_wakeup`'s scene deadline,
   `backend.rs:11242-11260`). **No core-side `next_wakeup` contribution is
   required** — the 1 ms `present_deadline` at `backend.rs:11266-11272`
   covers backend-side polls only, and nothing here needs it.

   **Testability (budgeted, not hand-waved):** the loop body's tail
   (deferred-input poll → drain → `maybe_composite`) is extracted into a
   testable `run_iteration_tail(state, backend)` function, and
   `RecordingBackend` gains `maybe_composite` call recording plus a
   `mark_dirty` counter (both are no-op defaults today,
   `trait_def.rs:583/:603`), so a unit test can pin "execute →
   same-iteration compose observes the damage". This small run-loop
   restructuring is part of the implementation plan's budget.
2. **One scheduling clock.** `eff` is computed at request time against
   `present_get_ust_msc()` — the **general** vblank clock — exactly as
   today (`process_request.rs:9043-9050`, `:9350-9366`). The due rule
   below uses the **same** clock. It must not use
   `present_get_completion_clock`: the two clocks intentionally diverge
   (`record_completion_clock` accepts sequence samples only when
   completion-idle, `backend.rs:5807-5818`; the general clock accepts all,
   `:5808`), and mixing them reclassifies a present arriving between a
   sequence sample and a flip retire as future-target, parking it an extra
   period on precisely the bee/Plasma hardware the 2026-07-27 fix was
   validated on. Completion **stamping and gate release** keep their
   existing completion-clock provenance (gate compare `run.rs:1302-1307`,
   queue push `:1317-1322`; the
   HW-validated "completion watermark" decision, `docs/status.md`
   2026-07-28) — scheduling and stamping are different concerns and this
   spec changes only the former. Round-3 review walked ksplash-on-bee
   through the split end-to-end and found no systematic added period. A
   unit test pins the no-reclassification property.

### Unified pending-present store

Generalize the existing parked-copy machinery (source waits:
`pending_present_pixmaps` + `arm_present_source_wait` /
`arm_present_syncobj_wait`, proven by the 2026-07-25 source-wait and
2026-07-28 acquire-wait work) into one core-side pending store, **keyed by
`present_id`**, with a **`wait_id → present_id` side map** for the
source-wait drain (`drain_ready_present_source_waits` returns `wait_id`s,
`backend.rs:11645-11665`, and the destroy/disconnect purges filter by them
today). Each entry carries the `PendingPresentPixmap` record (both
`Pixmap` and `PixmapSynced` variants), its `effective_target_msc`, its
source pin token (below), and two independent readiness conditions:

- **source-ready** — the dma-buf READ sync-file / acquire timeline point has
  signalled (existing mechanism, unchanged);
- **msc-due** — defined in the next section.

### New Backend capability surface

The design needs **six** new `Backend` trait methods (round-2/3/4
finding: none of these is reachable from core today —
`scene_wants_compose`, `present_completion_is_idle`,
`crtc_queue_sequence_unsupported`, `scanout_allowed`, and
`kms_outputs_active` are all private to `KmsBackend`,
`backend.rs:5711/:5719/:384/:5697/:416`):

- `present_flip_in_flight() -> bool` — KMS impl:
  `scene.has_pending_page_flips()` (`scene.rs:861-865`). Used **only** by
  the immediate-target arrival rule. Non-KMS backends default `false`.
- `present_display_idle() -> bool` — KMS impl:
  `!scene.has_pending_page_flips() && !scene_wants_compose()` (the
  existing private `present_completion_is_idle`, `backend.rs:5719-5721`,
  exposed under this name). Used **only** by the idle-display fallback.
  The two predicates must not be conflated: with the drain hoisted above
  `maybe_composite`, the drain runs at the one point in the iteration
  where a pending compose has *not yet been submitted* — flips are `false`
  while `scene_wants_compose()` is `true` — so an idle-fallback gated on
  `present_flip_in_flight()` alone would fire on essentially every
  iteration with pending damage and collapse future-target pacing back to
  eager on all drivers.
- `present_absolute_vblank_arm_supported() -> bool` — mirrors the
  `crtc_queue_sequence_unsupported` latch. Gates the idle-display
  fallback to flip-driven-clock drivers (on sequence-capable drivers an
  idle display can still tick via the absolute arm, and applying the
  fallback there would reintroduce the early-frame bug for mpv).
- `arm_present_absolute_vblank(&[u64]) -> io::Result<usize>` — the new
  per-target absolute sequence arm (next section).
- `present_scanout_blackout() -> bool` — true while the display cannot
  scan out at all: KMS impl `!(scanout_allowed() && kms_outputs_active)`,
  which is literally `next_wakeup`'s `allow_kms_timers` complement
  (`backend.rs:11244`). Round-4 finding: `scanout_allowed()` alone is
  **VT-only** (`backend.rs:5697-5699`); DPMS-off instead toggles
  `kms_outputs_active` (`set_dpms_power`, `backend.rs:18603/:18636`), so a
  DPMS blackout rule keyed on `scanout_allowed()` would never fire, and
  `present_display_idle()` cannot substitute (with damage pending while
  dark, `scene_wants_compose()` is true). Gates the blackout rule in
  Lifecycle.
- `pin_present_source(host_xid) -> Option<u64>` /
  `release_present_source(pin_id)` — **opaque backend-issued token**, not
  an xid-keyed pair: `store.incref` / `store_decref_with_invalidate` take
  a `DrawableId` (`store.rs:900-904`), and the `host_xid → DrawableId`
  `by_xid` mapping is exactly what a `FreePixmap` can invalidate and what
  xid reuse can re-point (`store.rs:977-981/:1000-1002/:824-826`). The KMS
  impl resolves the xid once at pin time and stores the `DrawableId`
  behind the token, mirroring how `PendingPresentSourceWait` captures
  `source_id: DrawableId` (`backend.rs:11543-11570`) and
  `finish_present_source_wait` decrefs that captured id, never a fresh
  lookup (`backend.rs:11667-11680`).

**Every parked entry takes the source pin at park time** and releases it
at execution, scrap, or purge. The existing wait-path incref
(`backend.rs:11558/:11627`) is not sufficient: both arm functions return
`Ready` *without* increffing whenever the source is already idle (the
~79% common case per the 2026-07-28 capture), and
`drain_ready_present_pixmaps` drops the wait pin unconditionally
(`finish_present_source_wait` at `process_request.rs:8712`) while the
entry may remain msc-parked. The two pins are distinct (no double-decref:
the wait pin is created only on the `Deferred` arm and released only by
`finish_present_source_wait`; the entry pin is created at park and
released exactly once by execute/scrap/purge). Without the entry pin, a
`FreePixmap` between park and due — legal immediately after
`PresentPixmap`; Xorg refs the vblank's pixmap for this reason — leaves
the copy targeting a dead drawable.

If `execute_present_pixmap_copy` fails, the entry's wake is still released
by XID immediately (like scrap: fence/syncobj + fence mirror +
`IdleNotify`), and its `CompleteNotify` (mode `Copy`,
`emit_idle = false`) is **routed through the ordered delivery queue**
rather than fired on the spot — a copy failure can never strand the
client's WSI on an unsignalled release point (today the `?`-return at
`process_request.rs:8614-8642` skips both the gate insert at `:8670` and
the enqueue at `:8680`), and the error path cannot invert per-window
serial order either (round-4 F5).

`execute_present_pixmap_copy` runs only when **both** conditions hold, and
is otherwise unchanged (copy → damage → gate insert →
`enqueue_present_completion`). Async presents
(`PRESENT_ALL_ASYNC_OPTIONS`) and unpaced environments (no MSC clock:
nested/headless — `HostX11Backend`/`RecordingBackend` return `(0,0)` from
`present_get_ust_msc`, so `effective_target_msc` is `None` and the whole
due rule collapses to today's arrival execution; pre-first-flip on KMS
likewise) keep today's immediate execution and completion. The
completion-side machinery (gate, parked completions,
`signal_present_wake` at target MSC) is untouched except for the ordered
delivery and Skip-carrying changes below.

### msc-due

Compose is damage-gated (`maybe_composite`, `backend.rs:11349-11350`) and
a parked present produces no damage until executed, so the due rule is
driven by arrival and by clock advance — evaluated in the (reordered)
core-side drain, **not** in the backend (`maybe_composite` has no
`ServerState` access). Let `clock` be the general vblank clock per the
contract above:

- **Late or due (`eff <= clock.msc`):** execute at arrival (Xorg
  late-present behavior: completes at the actual MSC).
- **Immediate target (`eff == clock.msc + 1`):** execute at arrival **iff
  `!present_flip_in_flight()`** — content can still make the compose whose
  flip retires at `eff`. If a flip is already in flight (the compose for
  `eff` has been submitted; this present cannot make it), park. A parked
  immediate-target entry executes in the drain of the iteration woken by
  the flip retirement — which, with the reorder, feeds that same
  iteration's compose.
- **Future target (`eff > clock.msc + 1`):** park. Where
  `CRTC_QUEUE_SEQUENCE` works, core arms
  `arm_present_absolute_vblank(&[eff - 1])` from a **third arming call
  site** in `run.rs` (alongside `present_pending_msc` at `:1378` and
  `present_pending_complete` at `:1399` — and **not** routed through
  `arm_present_completion_idle_vblanks`, whose idle-only gate
  (`backend.rs:17885-17893`) would suppress it during activity). The
  general clock accepts active-display sequence samples
  (`backend.rs:5808`), so the arm's event makes the entry due under the
  scheduling-clock contract, and the (reordered) drain of that wakeup
  executes it in time for the compose whose flip lands at `eff`. The
  absolute arm **keeps its own per-CRTC target set** and neither consumes
  nor suppresses the existing single-slot relative-1 idle arm
  (`armed_vblank_targets` admits one in-flight sequence per CRTC,
  `backend.rs:5832-5870`, cleared on any sequence event `:5784-5787` —
  sharing it would either starve the absolute arm forever under a
  compositor, which parks NotifyMSC every iteration, or let a distant
  target suppress the idle arm and re-open the compositor frame-clock
  deadlock it exists to prevent). Multiple in-flight `CRTC_SEQUENCE`s per
  CRTC are legal. `clear_all_armed_vblank_targets` (`backend.rs:5730`,
  VT suspend) clears **both** sets. **The two arms are discriminated via
  `user_data` tagging** (round-4 finding: `drm_event_crtc_sequence`
  carries no CRTC; yserver already encodes it as
  `user_data = u64::from(crtc_id)`, `backend.rs:5885`, recovered at
  `page_flip.rs:285`, and `on_crtc_sequence_event` clears the relative
  slot on *any* event, `backend.rs:5772-5776`): the absolute arm sets
  `user_data = ABSOLUTE_TAG | crtc_id` (upper 32 bits are free), the
  relative-slot clear applies only to untagged events, and tagged events
  retire absolute entries with `target <= sequence`. Clock recording
  (`backend.rs:5807-5818`) stays unconditional for both. If
  `arm_present_absolute_vblank` returns `Err` or `Ok(0)` — including the
  iteration where the `EOPNOTSUPP` latch first trips, before
  `present_absolute_vblank_arm_supported()` reflects it — the entry
  executes immediately in that same drain (`trigger=idle_fallback`), so
  the first-arm-on-an-unlatched-driver case cannot park with no wake.
  Mitigating note: whenever a NotifyMSC is parked, the relative-1 arm
  already ticks the general clock every vblank, so the absolute arm is
  only strictly needed when no NotifyMSC is outstanding. Where the ioctl is unsupported (the nvidia box's
  `EOPNOTSUPP` latch, `backend.rs:5899-5906`), the clock is flip-driven
  only: the entry executes when flips advance the clock to `eff - 1`, and
  if `present_display_idle()` holds (no flip in flight **and** nothing
  composing — the clock cannot advance at all), it executes immediately.
  **Documented concession:** on such drivers a future-target present on an
  otherwise idle display shows early; this is bounded by the driver
  capability, does not affect immediate-target clients (the CS2/ksplash
  populations), and is strictly better than the deadlock (park forever: no
  damage → no compose → no flip → clock frozen) that unconditional parking
  would produce there. The idle-display fallback applies **only** on
  `!present_absolute_vblank_arm_supported()` drivers, per the capability
  section above.

### Supersession

On arrival of present `B` for window `W` with effective target `T`
(computed against the same clock sample), **where `B.update_rects ==
None`** (the Xorg gate, verification item (d): `present_scmd.c:802`
attempts scrap only for a successor with no update region — a successor
carrying rects never scraps), scan the pending store for entries `A`
with the same window and same effective target that have not executed,
**and whose update region is covered by `B`'s full-extent rect**
(yserver's strictly conservative addition on top of Xorg, which ignores
geometry entirely; it only bites on mid-resize offset/size mismatches).
Coverage is defined on the resolved `update_rects:
Option<Vec<RegionRect>>` (`server.rs:1685-1687`) — not on the raw
`req.update` xid, because `update != 0` with an unresolvable region also
yields `None` and a full-extent copy
(`process_request.rs:9026-9034/:9341-9349`) — in destination coordinates
with `i32` arithmetic (offsets are `i16` and may be negative; today's
copy math saturates):

- `update_rects == None` covers the rectangle
  `(x_off, y_off, src_width, src_height)` — bounded by the **source
  pixmap**, not the window (`process_request.rs:8632-8642`), which matters
  mid-resize when Mesa reallocates swapchain pixmaps on
  `PresentConfigureNotify`;
- `update_rects == Some(rects)` covers
  `∪ (x_off + r.x, y_off + r.y, r.width, r.height)`;
- `update_rects == Some(empty)` (the documented zero-pixel present) covers
  nothing as a successor (never scraps others) and is trivially covered as
  a predecessor (always scrappable).

The full-frame game population always satisfies coverage. Each covered
`A` is scrapped:

- cancel its source/acquire wait if armed (`finish_present_source_wait` —
  drops the wait pin) and release the entry's source pin token;
- release its buffer **by XID, immediately** — trigger `idle_fence` via
  `dri3_trigger_fence` / signal `release_syncobj@release_value` via
  `dri3_signal_syncobj` (the teardown path's mechanism; `A` never reached
  `enqueue_present_completion`, so no backend `PinnedWake` or gate entry
  exists for it — the by-XID release is its sole signal path, keeping the
  "each population released exactly once" invariant from the 2026-07-27
  plan's self-review), set the X11 fence mirror
  (`state.sync_fences[idle_fence_xid].triggered = true`, as
  `complete_present_with_clock` does at `process_request.rs:9880-9887` —
  without it a client's `XSyncQueryFence` disagrees with its own unblocked
  wait for up to a period), and fire `IdleNotify` now;
- park the `CompleteNotify` mode `Skip` for ordered delivery at the target
  clock (next section).

The **successor gate + coverage condition** together protect partial
updates: a sliver present (marco/picom presenting drag slivers into the
COW — the documented 2026-07-08 pattern, `process_request.rs:8975-9022`
diagnostic) never scraps as a successor (it carries an update region —
the Xorg gate declines), and is only ever scrapped by a full-frame
successor that overwrites everything, so no subrect content is lost.
Same-target entries whose successor declines all execute at their due
point, in arrival order. Verification item (d) below is resolved: this
is exactly Xorg's semantics plus one strictly conservative extent check,
and the only possible future relaxation is dropping that extent check to
match Xorg's geometry-blindness (worth nothing unless mid-resize
telemetry shows it declining real scraps).

Supersession is not gated on redirection (Xorg picks copy-vs-flip only;
the 2026-07-27 plan explicitly warns against redirect-gating), so presents
to a compositor's COW behave identically — subject to the same coverage
condition.

### Ordered completion delivery (per-window `present_id` order)

Per-window `CompleteNotify` serial order must be non-decreasing; Mesa's
`loader_dri3` regenerates its swap accounting from the latest event's
serial, so a backward serial is a real client hazard. Xorg cannot emit
one: `present_vblank_scrap` idles the pixmap immediately but leaves the
scrapped vblank *in the ordered queue* (`pixmap = NULL`) and notifies at
the target MSC in queue order — its ordering property comes from that
single ordered list, with no async GPU-completion stage that can reorder
against it. yserver has exactly such a stage: a Copy enters the delivery
queue at **GPU-fence-retirement** time (`run.rs:1306-1322`) while a Skip
would enter at **scrap (request-arrival)** time, so raw insertion order is
not serial order (round-3 blocker: P1 executes at arrival, P2 parks, P3
scraps P2 → `Skip(P2)` enqueued before P1's fence retires → backward
serial). Three changes make yserver match:

1. **`PendingPresentComplete` gains `mode: u8` and `emit_idle: bool`**
   (`server.rs:819-822` today carries only `{event, effective_target_msc}`),
   and `fire_present_completion_events_at` threads them through instead of
   hardcoding `COMPLETE_MODE_COPY` (`process_request.rs:9680`) and
   unconditionally emitting `IdleNotify` (`:9650-9670`). A parked Skip is
   enqueued with `mode = Skip, emit_idle = false` — its `IdleNotify`
   already fired at scrap time, and a duplicate would mark a live,
   re-submitted buffer free in Mesa's per-pixmap busy tracking.
   `complete_present_with_clock`'s `signal_present_wake(present_id)` on an
   id the backend never saw is a verified no-op
   (`backend.rs:17855-17857`); the `sync_fences` re-trigger is a harmless
   duplicate.
2. **The due arm of the drain routes through the queue too** (today it
   fires immediately at `run.rs:1324-1339` while parked entries fire at
   `:1365`): every gated completion is pushed into
   `present_pending_complete` and delivered by
   `fire_due_present_completions`. Its `mem::take`-and-rebuild sweep
   (`process_request.rs:9893-9917`) preserves *insertion* order and is
   O(n); **the sweep is rewritten** to select, per window, the smallest
   unblocked `present_id` first — insertion order alone is exactly what
   bullet 3 replaces (round-4 F4).
3. **Delivery order is per-window `present_id` order with per-window
   hold-back**, not raw insertion order. `present_id` is monotonic and
   allocated at request time (`server.rs`, `next_present_id`), so it *is*
   per-window serial order. A due entry for window `W` is held back while
   any smaller `present_id` for `W` is still unresolved in **any** of the
   three states: msc-parked-unexecuted (the pending store),
   executed-but-undrained (`present_complete_gate`, which already carries
   `dst_window_xid`), or parked-not-yet-due. The hold-back is
   **per-window** so a stalled window's GPU copy cannot head-of-line
   block another window's completions. Async presents sit outside the
   hold-back by construction (`effective_target_msc` is `None` only for
   async, so they complete immediately, `run.rs:1340-1349`, possibly
   ahead of an earlier parked synced present for the same window) — this
   is Xorg-parity and pre-existing, noted so an implementer does not
   chase it (round-4 F6).

Unit tests pin the two round-3 inversion vectors: (i) P1
executes-at-arrival / P2 parks / P3 scraps P2 / P1's fence retires late —
`Skip(P2)` must not be delivered before `Copy(P1)`; (ii) an uncovered
partial-update survivor `A` executing at `T` with a covered later `B`
scrapped — `Skip(B)` must not be delivered before `Copy(A)`.

### Multi-output honesty

`present_get_completion_clock` / `present_get_ust_msc` return the
**most-advanced output's** sample (`platform.rs:1538-1556`; pinned by
`present_get_ust_msc_returns_most_advanced_output`), and core has no
window→output mapping. `present_flip_in_flight()` is likewise global
(`scene.rs:861-865` is `outputs.iter().any(..)`), so on dual-head with
output A flipping continuously (video), an immediate-target present for a
window on idle output B parks at arrival and executes in the drain woken
by A's next flip retirement (~one fast-output period, well under its own
refresh) — pacing follows the fast output, as it already does for the
shipped completion gates. On mixed-refresh dual-head, `eff` for a
slow-output window is computed against the fast output's clock with the
same consequence. This is a pre-existing property — not a regression
introduced here — and per-output clock plumbing is out of scope. The spec
makes no per-output claims; a dual-head hardware check is part of
validation.

### Dead scheduler removal

The informational `present_scheduler` enqueues
(`process_request.rs:9120, 9424`, drained only at teardown `:1443`; noted
as dead in the 2026-07-27 plan and implicated in the 2026-07-25 teardown
finding of 1,676 failed idle-syncobj signals on destroyed surfaces) are
removed, **together with the then-unreferenced path-selector helpers**
`present_path_for` / `present_path_for_synced` /
`build_path_selector_inputs` (`process_request.rs:9494/:9511/:9535`) —
leaving them would fail CI-exact `cargo clippy --all-targets -- -D
warnings` as dead code (`present_scheduler::choose_path` keeps its own
unit tests and survives). The unified pending store **is** the scheduler.

### Lifecycle

Window-destroy and disconnect purges extend the existing
`pending_present_pixmaps` teardown paths (`process_request.rs:1374-1454`,
`process_disconnect.rs:299-330`), now resolving entries via the
`wait_id → present_id` map where the trigger is a wait: release by XID +
fence mirror + IdleNotify (owner alive) + release the entry source pin +
drop, exactly one release per entry; parked Skip/Copy completions for a
destroyed window are dropped from `present_pending_complete` by the
existing purge. **Shutdown:** parked core-side entries are released by XID
in the shutdown sequence before client sockets close
(`signal_all_retained_present_wakes`, `lib.rs:603`, covers backend pins
only — it never sees never-executed entries). **VT switch:** suspend
clears both armed-vblank sets (`clear_all_armed_vblank_targets`,
`backend.rs:5730`, extended to the absolute-arm set); parked entries
survive and become due again via post-resume flips or the idle-display
rule. **DPMS-off / VT-away blackout (`present_scanout_blackout()`):** no flips
occur, no sequence samples arrive (`arm_idle_vblanks_with` bails at
`backend.rs:5845`; a disabled CRTC has no vblank), the completion clock
freezes, and `next_wakeup`'s scene deadline is gated off
(`backend.rs:11244-11246`) — so parked entries would otherwise hold their
source pins and the client's buffers for the whole blackout, and executing
the copy alone would not help (the release fires only through the clock
test in `fire_due_present_completions`, which a frozen clock never
satisfies — round-4 F1c). While `present_scanout_blackout()` holds, both
halves flush: parked entries execute immediately
(`trigger=blackout`), **and** parked completions in
`present_pending_complete` are delivered immediately with the current
(frozen) clock stamp — buffers genuinely keep cycling while dark,
matching the existing convention that a dark display does not pace
clients.

## What this does and does not fix

- CS2-class clients (synced, `target_msc=0`, multi-buffer, full-frame
  updates): buffers release at arrival rate via supersession → free-run
  bounded by render + at most ~2 executed copies per vblank per window
  (the immediate-target arrival execution plus the parked survivor),
  instead of one copy per present. Round-2/3 review traced full periods:
  ~3 arrival-time supersession releases plus 1 executed survivor per
  vblank, the game never dropping below 2 free images of 4, no
  cross-period scrap, no starvation at flip-retire boundaries. Expected
  ceiling on the measured HW: ~300–370 fps (2.7 ms frame time), up from
  180–200. Full Xorg parity (~400) additionally needs the flip path
  (`PresentCaps.flip_path`, tracked separately — "until alien-BO scanout
  integration lands").
- Vsync clients (distinct targets): paced at refresh as today. Latency is
  Xorg-parity, **not always identical to today's eager copy**: a present
  arriving while a flip is in flight parks and its pixels land one period
  later than eager would have shown them (Xorg copies *at* the target
  vblank; eager showed them early). Presents arriving with no flip in
  flight — the common just-after-vblank case — execute at arrival exactly
  as today.
- Future-target clients (mpv `target_msc` scheduling): frames no longer
  show early on sequence-capable drivers (via the new absolute arm); on
  NVIDIA-class drivers the idle-display concession above applies.
- `PresentCapabilityAsync` advertisement stays tied to `flip_path`
  (unchanged); supersession makes the fix independent of whether the
  client's WSI ever sets async options.

## Xorg verification items — RESOLVED 2026-08-01

Verified against a local reference clone
(`~/Projects/xserver`, gitlab.freedesktop.org/xorg/xserver, commit
`5541a5c8befbb3975f69c1d7666a2ba76a57b682`):

- **(a) CONFIRMED — scrappable regardless of fence/acquire state.** The
  scrap loop (`present_scmd.c:802-823`) keys only on `vblank->pixmap !=
  NULL`, `vblank->queued`, same `crtc` + same `target_msc`, plus a
  TearFree carve-out (`vblank->reason >= PRESENT_FLIP_REASON_DRIVER_TEARFREE
  && exec_msc == target_msc` — not applicable to yserver, which has no
  TearFree path). No `wait_fence` or acquire check. And
  `present_vblank_scrap` (`present_vblank.c:218-239`) signals
  `release_syncobj @ release_point` **immediately at scrap** on the
  explicit-sync path (else `present_pixmap_idle`). The client-side WAR
  hazard is therefore Xorg-parity, not a yserver invention.
- **(b) CONFIRMED — target-clock stamp.** The scrapped vblank stays
  queued (`pixmap = NULL`); at its target vblank `present_execute_post`
  computes `mode = PresentCompleteModeSkip` because `!vblank->pixmap`
  and notifies with the **executing vblank's** `(ust, crtc_msc)`
  (`present_execute.c:139-159`). The parked-delivery design matches.
- **(c) CONFIRMED — idle-at-scrap, complete-at-target,** in the ordered
  per-window vblank list (see (a)/(b) mechanics).
- **(d) RESOLVED — Xorg's scrap is gated on the SUCCESSOR having no
  update region** (`if (!update && pixmap)`, `present_scmd.c:802`) and
  ignores the predecessor's region entirely. It is neither
  region-insensitive nor predecessor-sensitive. Consequence adopted in
  §Supersession: scrap requires `B.update_rects == None` (the Xorg
  gate); yserver keeps its extent-coverage check on top as a strictly
  conservative extra (Xorg ignores offset/size geometry; the check only
  bites on mid-resize mismatches). This dissolves the marco-sliver risk
  exactly the way Xorg dissolves it: a sliver present (with an update
  region) never scraps as a successor, and as a predecessor it is only
  scrapped by a full-frame successor that overwrites everything. The
  `supersede_declined` telemetry remains useful to observe how often the
  successor gate declines in compositor dogfood.

## Telemetry

Extend the `present_pace` debug stream:

```text
stage=parked_msc pid=... eff=... clock_msc=... reason=flip_in_flight|future
stage=exec_due pid=... eff=... clock_msc=... trigger=arrival|drain|sequence|idle_fallback|blackout
stage=superseded pid=... by=... window=0x... eff=...
stage=supersede_declined pid=... by=... window=0x... eff=...   (same target, coverage failed)
```

(`trigger=sequence` occurs only on drivers with the new absolute sequence
arm; `reason` disambiguates why an entry parked, which the raw
`flip_in_flight` bit alone would not.) `render_telemetry` gains
`present_skips/s` alongside the existing copy/submit counters so HW
captures can show the copy-rate collapse without debug logging.
`supersede_declined` occurrences in a compositor dogfood session are the
empirical answer to verification item (d)'s relaxation question.

## Validation

- Unit — loop/clock contract: via the extracted `run_iteration_tail` and
  the instrumented `RecordingBackend`, an entry executed by the due-pass
  is observed by the **same iteration's** compose (pins the
  drain-before-compose reorder); a present arriving between an
  active-display sequence sample and a flip retire is not reclassified
  future-target (pins the one-clock contract).
- Unit — supersession: covered same-target supersession releases the
  earlier present (fence/syncobj signalled, fence mirror set,
  `IdleNotify` at scrap, entry dropped, source pin token released);
  uncovered same-target presents both execute in arrival order;
  distinct-target presents never supersede; the zero-pixel-update present
  neither scraps nor survives scrap; coverage handles negative offsets,
  unresolvable update regions (`update_rects == None` despite
  `update != 0`), and mid-resize source-extent changes.
- Unit — ordered delivery: the two round-3 inversion vectors (P1/P2/P3
  with late fence retirement; uncovered survivor + covered scrap) deliver
  per-window `present_id` order with no second `IdleNotify` for a Skip;
  hold-back is per-window (a stalled window does not block another
  window's due completions).
- Unit — due rule: a future-target present produces no copy and no damage
  until due; the immediate-target rule executes at arrival with no flip in
  flight and parks with one; a parked immediate-target entry executes on
  the flip-retirement wakeup; the idle-display rule
  (`!present_absolute_vblank_arm_supported()` drivers only, gated on
  `present_display_idle()` — both terms) executes a source-ready parked
  present; the blackout rule executes parked entries while
  `!scanout_allowed()`; combined source-wait + msc-due only executes when
  both hold; the absolute arm coexists with a parked NotifyMSC (neither
  suppresses the other).
- Unit — lifecycle: a parked entry pins its source across `FreePixmap`
  (token-based; xid reuse cannot re-point the pin) and still executes (or
  is purged with exactly one release); a failing copy still releases the
  wake and completes; window-destroy, disconnect, and shutdown purge
  parked entries with exactly one release each, resolving wait-triggered
  purges through the `wait_id → present_id` map.
- Wire: `CompleteNotify` mode `Skip` encoding (both byte orders).
- Existing Present, source-wait, acquire-wait, lifecycle, and DRI3 suites
  stay green; `cargo clippy --all-targets -- -D warnings` (CI-exact,
  including the removed path-selector helpers) and `cargo +nightly fmt`.
- Hardware (the real gate):
  - CS2 on the nvidia box, repeat of the 2026-07-31 capture: the
    3–4-copies-per-vblank lattice collapses; presents/s rises toward the
    ~370 fps render capability; executed `copy_area`/s to the game pixmap
    drops toward ~2× flip rate; `present_skips/s` absorbs the difference.
    Record the `kernel_frame=` value to settle the software-msc question
    above.
  - ksplashqml on bee/Plasma stays ~60 fps (no free-run regression — the
    2026-07-27 fix's original target), accepting up to one period of
    added scanout latency vs. today's eager copy for presents that arrive
    with a flip in flight (Xorg parity).
  - mpv on bee + silence (Pixmap and PixmapSynced, windowed + fullscreen,
    compositing on/off): smooth, no early-frame fast-forward, release
    progress complete.
  - marco/MATE compositing dogfood with `YSERVER_PRESENT_TRACE` /
    `supersede_declined` telemetry: no drag-smear regression; record
    whether sliver presents ever collide on effective target (verification
    item (d) relaxation data).
  - Dual-head on silence (mixed-refresh if available): no pacing anomaly
    beyond the documented global-clock behavior; a window on the idle
    output still presents correctly while the other output plays video.
  - DPMS-off blackout with an actively presenting client: parked entries
    and parked completions both flush (`trigger=blackout`), buffers keep
    cycling; clean resume with correct pacing restored.
  - Warframe and Steam regression set from the 2026-07-27 plan's Task 10.
  - Desktop DE dogfood (MATE/XFCE/Cinnamon): no interactivity or input-lag
    regression.
