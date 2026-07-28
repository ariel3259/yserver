# PresentPixmapSynced acquire timeline wait

**Status:** implemented and hardware-validated on bee/Plasma and on silence
dual-head under MATE, XFCE, and Plasma.

## Hardware result

Fullscreen playback is smooth on bee/Plasma after this change. The successful
capture contains 2,221 mpv `PixmapSynced` requests:

- 1,748 acquire points were already ready;
- 473 (21.3%) were deferred and all 473 subsequently signalled;
- deferred waits averaged 0.87 ms, with a 6 ms maximum;
- the kernel DRM-eventfd path was used; no Vulkan-poll fallback warning was
  emitted;
- every request produced a release wake, and the user observed that playback
  now works.

Although the waits are short, the intervention is decisive: before this
change those 473 requests copied a source that the client had not released
for reading. Combined with the visual result, this confirms stale pre-acquire
Copy execution as the bee/Plasma playback cause.

Follow-up validation on silence passed in a dual-head configuration under
MATE, XFCE, and Plasma. Playback remained smooth and both CPU and GPU load
were very low, ruling out a hidden busy-poll or multi-output regression in
those sessions.

## Problem

`PresentPixmapSynced` supplies `acquire_syncobj@acquire_value` to prove that
the client has finished rendering the source pixmap. yserver previously
executed `copy_area` immediately and used only the release syncobj. With an
explicit-sync Vulkan client, the dma-buf reservation object is not a
substitute for that timeline point, so yserver could copy stale content.

Xorg's Copy path checks the acquire point in `present_execute_wait`; if it is
not signalled, Xorg registers a DRM syncobj eventfd and resumes execution from
that notification. The server thread does not block.

## Design

- When DRI3 imports a syncobj fd, duplicate the fd before Vulkan consumes it.
  Import the duplicate as a process-local DRM syncobj handle and retain that
  handle beside the Vulkan timeline semaphore.
- For `PresentPixmapSynced`, register a non-blocking eventfd for the requested
  acquire timeline point. Pin both the exact source drawable and imported
  syncobj while the request is parked.
- Generalize the existing deferred `PresentPixmap` record so it can carry
  either protocol request. Only after readiness may core record `copy_area`,
  report Damage, enqueue GPU completion, and eventually signal the release
  point.
- If DRM handle import or `DRM_IOCTL_SYNCOBJ_EVENTFD` is unavailable, retain
  correctness with a non-blocking Vulkan timeline-counter poll and the
  existing 1 ms main-loop wake fallback.
- Window destruction and client disconnect purge parked synced requests just
  like implicit producer waits; window destruction signals the release point
  when it is still addressable.

## Telemetry

The `present_pace` stream distinguishes:

```text
stage=acquire_ready syncobj=... value=...
stage=acquire_deferred wait_id=... syncobj=... value=...
stage=acquire_signaled wait_id=...
```

The bee run confirms that acquire points are commonly pending and records the
request-to-acquire latency; see the hardware result above.

## Validation

- A core regression test parks a synced request, verifies no Copy or Damage
  occurs, marks the wait ready, and verifies execution then occurs.
- Existing Present, source-wait, lifecycle, and DRI3 tests must remain green.
- `cargo clippy --all-targets -- -D warnings` must pass.
- Hardware: bee/Plasma fullscreen mpv passed with complete release progress;
  silence dual-head passed under MATE, XFCE, and Plasma with very low CPU/GPU
  load.
