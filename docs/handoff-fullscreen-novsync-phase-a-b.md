# PR handoff: async Present pacing and fullscreen direct scanout

## Suggested title

`fix(present): stabilize no-vsync pacing and enable fullscreen direct scanout`

## Summary

This PR builds the safe foundation for fullscreen no-vsync presentation in two
layers:

- **Phase A — async Present defer and supersession.** While a display flip is
  in flight, genuinely asynchronous Presents are parked instead of immediately
  recomposing every request. A newer compatible async Present scraps the parked
  predecessor using Xorg-style latest-wins semantics. Synced Present behavior is
  unchanged.
- **Phase B — fullscreen direct scanout.** A fullscreen Unredirected window may
  act as the authoritative root and, when all scanout predicates pass, its
  buffer is submitted directly to KMS. Explicit-sync sources are supported and
  fullscreen candidates are pre-probed before admission.

The result is a stable VBlank-synchronized direct path. This PR does **not**
implement tearing or DRM async page flips.

## Problem

A fullscreen no-vsync workload can produce Presents much faster than display
refresh. Sending every request through composition wastes GPU/CPU work and can
reduce the actual display flip cadence. Direct scanout also previously rejected
fullscreen Unredirected windows before reaching the KMS eligibility probe, so
eligible game buffers could not bypass composition.

Phase A controls an actual async flood at the Present scheduler. Phase B then
removes composition when the newest frame is a safe fullscreen scanout
candidate. These mechanisms are complementary but independent: Phase A is the
safe fallback when direct scanout cannot engage; Phase B is the efficient KMS
path when it can.

## Implementation

### Phase A

- Classify an async Present behind an in-flight flip as deferred until the next
  display boundary.
- Allow a compatible successor with the same CRTC and known effective target
  MSC to supersede a covered parked predecessor.
- Complete the discarded predecessor as `Skip`, preserving ordering and Present
  completion semantics.
- Leave synced Presents on their existing scheduling path.

### Phase B

- Treat eligible fullscreen Unredirected windows as authoritative-root
  candidates.
- Admit fullscreen explicit-sync Presents to direct scanout.
- Pre-probe the source framebuffer before committing to the direct path.
- Track direct and composed-unflip transactions per output.
- Materialize a composed fallback into the frame's pinned paint target rather
  than assuming that target is always the Composite Overlay Window.
- If a synchronized atomic unflip fails, retain the still-scanned buffer and
  degrade to composed per-output flips instead of terminating the server.

The branch also retains deferred store references for Pictures whose backing
materializes late. This prevents a live Picture from losing its drawable during
game startup.

## Hardware validation

Testing used Cinnamon with fullscreen CS2/no-vsync on the NVIDIA KMS system.
The direct path was exercised with the opt-in NVIDIA hardware-cursor validation
lever.

Initial validation established that:

- the earlier 27–47 Hz display-flip collapse was absent and page flips held near
  the 60 Hz display refresh;
- Present supersession absorbed the request flood;
- no panic or fatal server error occurred;
- composed-unflip failure handling recovered safely.

The capture also corrected an important premise: CS2's observed `options=0x8`
is `PresentOptionSuboptimal`, not `PresentOptionAsync`. Therefore that specific
session was stabilized primarily by the already-merged synced supersession and
DRI3 syncobj fixes. Phase A remains necessary for clients that really send
`PresentOptionAsync` or `PresentOptionAsyncMayTear`, but it must not be credited
as the mechanism for that `0x8` capture.

### Post-#95 pacing investigation

#95 made direct and composed-unflip transactions visible to the per-output
Present scheduler. A diagnostic selector compared that production `post95`
behavior with the earlier scheduler boundary using the same commit and binary.

The counterbalanced `post95-1 -> pre95 -> post95-2` run did not give `pre95` a
telemetry advantage: average `page_flip/s` was 56.6 / 55.3 / 55.9, and samples
at or above 55 Hz were 87.0% / 84.3% / 86.7%. Phase B engaged in all three
cases, retiring 21,217 / 18,195 / 27,660 direct frames without a panic or fatal
server error. Perceived quality improved with run order rather than with the
selector.

Follow-up repeatability runs kept production `post95` behavior, the same binary
and duration, and recorded workload markers, NVIDIA load, Present stages, and
KMS page-flip jitter. None of those post95 runs reproduced perceived-Hz lag.
The evidence therefore rejects the post-#95 visibility change as a sufficient
cause and does not support reverting #95.

## Scope boundary: no tearing in this PR

This PR intentionally uses VBlank-synchronized KMS flips. A user disabling
VSync is represented by effective Present option bits, not inferred from request
rate or game settings. The follow-up Phase C may select tearing only when all of
the following hold:

```text
fullscreen_direct_scanout_eligible
&& effective_options contains (Async | AsyncMayTear)
&& DRM async page flip is supported
```

That follow-up will advertise `async_may_tear`, submit `PAGE_FLIP_ASYNC`, and
own the additional buffer-retirement, fence, fallback, cursor, and multi-output
validation. Keeping it separate means async tearing can be reviewed or reverted
without losing the safe Phase A/B infrastructure in this PR.

## Review notes

- Direct and composed-unflip transactions are always visible to Present pacing;
  there is no runtime feature gate.
- NVIDIA retains its existing software-cursor policy.
- Direct scanout still requires all existing format, modifier, geometry,
  coverage, cursor, overlay, synchronization, and output eligibility checks.
- The repeatability harness is retained for later pacing comparisons.
