# Present completion clock provenance — design

**Status:** implemented in the working tree and software-verified. Bee hardware
validation produced zero visible improvement in fullscreen mpv; retain as a
clock-provenance correctness change, but reject it as the playback fix.

**Goal:** prevent `PresentPixmap` / `PresentPixmapSynced` Copy completions
from being released by a standalone CRTC sequence event while visible
scanout work is active. A completion must use either a real pageflip
retirement or a CRTC sequence event that was observed while the display was
genuinely idle.

This is a corrective follow-up to the target-MSC completion pacing on
`fix/present-complete-vblank-pacing`. It preserves that branch's
`present_id`, retained-wake, target-MSC math, and lifecycle machinery.

## Hardware result

The post-implementation bee capture proves that the split was active without
improving playback. Across mpv's 25.93-second request interval:

- mpv submitted 651 `PixmapSynced` requests (25.07/s);
- 648 mpv completions were released by `PageFlip`, and three by a sequence
  observed during a momentarily idle scene;
- 180 sequence events observed while active were excluded from the completion
  clock;
- mpv's request cadence remained concentrated around 39--41 ms, while its
  completion cadence was quantized mostly to 33--34 ms and 49--51 ms;
- the user observed zero difference in fullscreen choppiness.

This falsifies lost clock provenance as the primary cause. It also raised the
existing `PixmapSynced` acquire-syncobj omission to the leading next
hypothesis. That follow-up is now implemented separately; see
[`2026-07-28-present-synced-acquire-wait-design.md`](2026-07-28-present-synced-acquire-wait-design.md).

## Evidence and corrected model

The bee capture identifies mpv as client 46 and records 664
`PixmapSynced` requests over 26.46 seconds (25.09 fps). During the same
interval:

- the shared Present MSC advances by 1,587 fields (59.98 Hz);
- only 1,049 real pageflip retirements occur (39.60 Hz);
- 104 of 660 mpv completions fire at an MSC for which the capture contains
  no pageflip retirement;
- another four completions fire before the matching pageflip retirement.

The integers and timestamps delivered by `DRM_IOCTL_CRTC_QUEUE_SEQUENCE`
are not invented: they are kernel CRTC vblank samples. What is invalid is
the inference currently made by core: because a sequence event and a
pageflip event overwrite the same `PlatformBackend::ust_msc` entry, any
advance of that entry is treated as evidence that an eagerly copied frame
may be completed. The clock has lost its provenance.

Xorg makes the distinction differently. Its Copy path queues a per-Present
vblank event and executes the copy when that event fires
(`present_scmd.c:865`, `present_execute.c:102`). Its flip path completes from
the driver's flip notification. yserver currently performs its GPU copy
eagerly and only defers the client wake and events, so it cannot claim exact
Xorg execution ordering. This design supplies the missing safety property:
a standalone vblank sample cannot masquerade as active scanout progress.

## Goals

- Preserve the source of every clock sample used by Present.
- Keep `NotifyMSC` driven by real CRTC vblank time, whether that time comes
  from a pageflip event or an explicitly queued sequence event.
- During active scanout, release paced Pixmap completions only from a
  pageflip-retirement sample.
- When there is no pending or requested visible scanout work, allow an idle
  sequence sample to release completions so obscured/offscreen presents and
  static desktops cannot deadlock.
- Stop issuing a completion-driven `CRTC_QUEUE_SEQUENCE` ioctl on every
  iteration while a flip or visible damage is pending.
- Preserve exactly-once signalling and all disconnect/window-destroy
  behavior already implemented on the branch.
- Make the chosen clock source visible in telemetry.

## Non-goals

- Delaying the GPU copy itself until target MSC. That would match Xorg's Copy
  execution ordering more literally, but is a separate and larger change.
- Implementing per-window CRTC selection. The current Present clock is global
  and selects the most advanced output. This design remains conservative and
  global when deciding whether the server is idle.
- Correlating a Present copy with the exact scanout BO generation containing
  its pixels. A pageflip retirement is the active-display pacing boundary,
  not proof of direct scanout of that individual window.
- Fixing the existing `PixmapSynced` acquire-syncobj wait omission. Xorg waits
  for the acquire point before Copy; yserver does not. That correctness bug
  should be designed and fixed separately, even if later testing shows that
  it contributes to stale mpv frames.
- Present flip mode / alien client BO direct scanout. yserver continues to
  advertise and emit Copy mode.

## Invariants

1. **Clock provenance is never discarded.** General vblank time and
   completion-eligible time are separate state.
2. **Active sequence events cannot release Pixmap completions.** They may
   satisfy `NotifyMSC`, but do not advance the completion clock.
3. **Every pageflip retirement advances both clocks.** It is valid general
   vblank time and valid active-display completion time.
4. **An idle sequence advances the completion clock only if idle is true when
   the event is consumed**, not merely when its ioctl was queued.
5. **Idle is conservative:** no scene pageflip is in flight and no scene or
   drawable presentation damage is waiting to be composed.
6. **A clock never moves backwards.** All updates use the existing
   wrap-safe MSC comparison; a late event cannot replace a newer sample.
7. **Wake signalling remains exactly once.** This design changes only when a
   parked completion is selected; `signal_present_wake(present_id)` remains
   the sole post-copy wake path.

## Architecture

### Two clocks, one kernel timeline

The backend exposes a general `(msc, ust)` watermark and a typed completion
watermark:

```rust
/// Latest real vblank sample, from either a pageflip retirement or a CRTC
/// sequence event. Used for target calculation and NotifyMSC.
fn present_get_ust_msc(&self) -> (u64, u64);

/// Latest sample eligible to release a paced Pixmap completion. Its source
/// remains attached all the way through completion event encoding.
fn present_get_completion_clock(&self) -> PresentClockSample {
    let (msc, ust) = self.present_get_ust_msc();
    PresentClockSample {
        msc,
        ust,
        source: PresentClockSource::BackendVblank,
    }
}
```

The default keeps nested, recording, and other non-KMS backends behaviorally
unchanged. KMS overrides the second method.

`PlatformBackend` stores per-output samples:

```rust
ust_msc: HashMap<usize, (u64, u64)>,
completion_clocks: HashMap<usize, PresentClockSample>,
```

The existing `ust_msc` remains the general clock map. Both getters retain the
current "most advanced output" behavior until per-window CRTC routing exists.

### Sample update rules

Pageflip event processing writes the kernel `(frame, ust)` to both maps
before returning the output index to the scene retirement path. The Asahi
`frame == 0` software-MSC fallback remains flip-driven and therefore also
updates both maps.

Sequence processing always updates `ust_msc`. It updates
`completion_clocks` only when the backend's idle predicate is true at event
consumption:

```rust
fn present_completion_is_idle(&self) -> bool {
    !self.scene.has_pending_page_flips() && !self.scene_wants_compose()
}
```

`scene_wants_compose()` already covers structural dirtiness and pending
drawable presentation damage. `SceneCompositor` needs a small aggregate
`has_pending_page_flips()` query over its per-output `pending_acks` queues.

The event-time check closes the important race:

1. core queues an idle sequence;
2. a client damages a visible window and yserver submits a flip;
3. the previously queued sequence arrives;
4. the backend observes active scanout and updates only the general clock;
5. the later pageflip retirement advances the completion clock and releases
   the parked frame.

Checking only at ioctl submission would incorrectly release at step 3.

### Core drain

`drain_present_completions` performs two independent drains:

```text
general vblank clock
  -> state.present_kernel_msc/ust
  -> fire_due_present_notify_msc

completion-eligible clock
  -> backend PresentClockSample (kept separate from ServerState)
  -> fire_due_present_completions
```

`complete_present_now` must stamp `CompleteNotify` with the sample that
actually released it. Therefore `fire_due_present_completions` passes its
eligible `(msc, ust)` through to `complete_present_now` /
`fire_present_completion_events`; it must not indirectly read the possibly
newer general clock from `ServerState`.

Immediate async/no-clock completions continue to use the latest general
sample, matching current behavior. A small value object avoids accidentally
mixing the two call sites:

```rust
#[derive(Clone, Copy)]
pub struct PresentClockSample {
    pub msc: u64,
    pub ust: u64,
    pub source: PresentClockSource,
}

pub enum PresentClockSource {
    PageFlip,
    IdleSequence,
    /// Default for a backend that supplies a real clock but does not expose
    /// finer provenance (nested/recording compatibility).
    BackendVblank,
    Immediate,
}
```

The source is server-internal telemetry; it is not encoded on the X11 wire.

### Idle vblank arming

`NotifyMSC` and completion fallback have different arming policy and should
no longer be combined into one unqualified target vector.

- `NotifyMSC` targets may call the existing `arm_idle_vblanks` regardless of
  scanout activity. Their sequence events advance only the general clock.
- Completion targets request an idle arm only while
  `present_completion_is_idle()` is true.
- The existing per-CRTC `armed_vblank_targets` map continues to deduplicate
  the actual ioctl. If a NotifyMSC arm already exists, no second ioctl is
  required.

Add a backend method with a default no-op:

```rust
fn arm_present_completion_idle_vblanks(
    &mut self,
    _target_mscs: &[u64],
) -> io::Result<usize> {
    Ok(0)
}
```

The KMS implementation returns `Ok(0)` while active and otherwise reuses the
existing queue helper. Core calls the two arming paths separately. Nested and
headless backends preserve the current pre-first-clock immediate behavior and
cannot park forever.

### Ordering within one DRM drain

One DRM read can contain both pageflip and sequence events. Preserve the
current order:

1. decode all events;
2. update both clocks for pageflip samples;
3. retire scene `pending_acks` for pageflips;
4. consume sequence samples and evaluate the now-current idle predicate;
5. return to core, which drains the two watermarks.

If a pageflip retirement leaves new presentation damage pending, the idle
predicate remains false. If it truly leaves the display idle, allowing a
same-batch sequence sample into the completion clock is harmless: both are
real samples and no visible scanout work is waiting.

## State transitions

```text
                       visible damage / flip submitted
        +------------------------------------------------+
        |                                                v
   IDLE DISPLAY                                   ACTIVE DISPLAY
   no pending_acks                                pending_acks or
   no compose demand                              compose demand
        |                                                |
        | sequence: general + completion                 | sequence: general only
        | pageflip: general + completion                  | pageflip: general + completion
        |                                                |
        +<-----------------------------------------------+
                     last flip retires and no damage remains
```

The state is derived, not latched. VT suspend, DPMS off, hotplug, and failed
atomic commits already clear pending scanout state; no new independent mode
bit can become stale across those lifecycle edges.

## Multi-output behavior

The existing global-max clock is already not sufficient for strict Present
CRTC semantics. Until windows are routed to CRTCs, this design uses a
conservative global idle predicate: activity on any output prevents sequence
events from advancing the completion clock on every output. Consequences:

- correctness is favored over latency;
- an obscured present on one output may wait for a real retirement while
  another output is continuously active;
- it cannot be released early by an unrelated standalone sequence sample.

Per-output gates require adding an output/CRTC identity to
`PresentCompleteGate` and selecting the CRTC from window coverage. That is a
future design, not a hidden extension of this fix.

## Telemetry

Replace the temporary ambiguous `PACE-INSTR ... fired msc=` line with source
information sufficient to falsify the design:

```text
present_clock sample source=pageflip output=0 msc=... ust=...
present_clock sample source=sequence output=0 msc=... ust=... completion_eligible=false reason=active
present_pace fired pid=... source=pageflip msc=... target=...
present_pace fired pid=... source=idle_sequence msc=... target=...
```

The initial implementation uses source-tagged diagnostic logs. If a capture's
volume makes those hard to aggregate, promote these same events to one-second
counters:

- `present_complete_pageflip/s`
- `present_complete_idle_sequence/s`
- `present_sequence_suppressed_active/s`
- `present_completion_idle_arm/s`

For fullscreen mpv, `present_complete_idle_sequence/s` must remain zero while
the flip/compose path is active. A nonzero value is a regression regardless
of whether playback happens to look smooth.

## Tests

### Backend clock tests

- Pageflip sample advances both maps.
- Asahi software-MSC pageflip advances both maps.
- Idle sequence advances both maps.
- Sequence observed with a pending scene ack advances only the general map.
- Sequence observed with compose demand advances only the general map.
- Sequence queued while idle but consumed after activity begins is
  suppressed for completion.
- A late/lower-MSC event does not move either map backwards.
- Suspend/DPMS arm clearing remains intact.

### Core tests

- A parked Pixmap completion is not released when only the general clock
  reaches its target.
- The same completion releases exactly once when the completion clock reaches
  the target.
- The emitted CompleteNotify carries the releasing clock's UST/MSC, not a
  newer general-clock sample.
- NotifyMSC still releases from a sequence-only general-clock advance.
- An idle-sequence completion releases and signals its retained wake once.
- Disconnect and window destroy still release/purge gated and parked entries
  exactly once.

### Hardware acceptance (result: playback criterion failed)

On bee, repeat the captured windowed -> fullscreen -> windowed mpv sequence:

- mpv request cadence remains approximately the media cadence;
- fullscreen playback has no 1–2 second visual stalls or bursts;
- active fullscreen completions report `source=pageflip` only;
- no completion MSC precedes or lacks its source pageflip log entry;
- sequence events may continue for NotifyMSC but are logged as suppressed for
  active completion;
- Plasma splash remains refresh-paced and login does not regress;
- `mpv --x11-bypass-compositor=no` remains smooth;
- drag interaction remains responsive.

Retest on air. Because `apple_drm` rejects CRTC sequence ioctls, behavior
should remain flip-driven and unchanged. Test one multi-output machine to
measure the conservative global-idle latency before considering per-CRTC
routing.

## Risks and follow-ups

- **No pageflip for a visible copy:** incorrect damage propagation could now
  cause a client-visible stall instead of being hidden by sequence releases.
  That is desirable diagnostically but must be watched during hardware tests.
- **Acquire synchronization:** if fullscreen mpv still repeats stale content
  while completion provenance is correct, the next investigation is the
  missing `PixmapSynced` acquire wait. Steady requests and correct completion
  timestamps cannot prove the copied source was ready.
- **Exact Xorg Copy semantics:** the long-term faithful model is to schedule
  copy execution itself at the target vblank and wait for explicit acquire
  synchronization before copying. That would remove the eager-copy timing
  difference rather than only constraining completion.
- **Per-CRTC Present:** required for strict multi-output timing and avoiding
  conservative cross-output blocking.

## Expected implementation surface

- `crates/yserver-core/src/backend/trait_def.rs` — completion-clock getter and
  idle-completion arming method.
- `crates/yserver-core/src/server.rs` — separate completion watermark/sample.
- `crates/yserver-core/src/core_loop/run.rs` — split clock drains and arming.
- `crates/yserver-core/src/core_loop/process_request.rs` — pass the releasing
  sample into completion event encoding.
- `crates/yserver/src/kms/render/platform.rs` — separate per-output maps and
  monotonic update helper.
- `crates/yserver/src/kms/render/scene.rs` — aggregate pending-flip query.
- `crates/yserver/src/kms/render/backend.rs` — event-source update rules, idle
  predicate, KMS arming policy, tests, and telemetry.

Estimated implementation size is 150–300 lines plus tests. It does not alter
the retained-wake ownership model or request protocol parsing.
