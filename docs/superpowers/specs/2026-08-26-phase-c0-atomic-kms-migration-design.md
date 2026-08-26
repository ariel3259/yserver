# Phase C.0 — complete atomic KMS migration and device-local commit ownership

**Date:** 2026-08-26
**Status:** Draft for review
**Branch:** `feat/phase-c0-atomic-kms-migration`
**Depends on:** PR #129 (`fix/fullscreen-novsync-stutter`)
**Successor:** Phase C.1, on-demand async page-flip tearing

## 1. Summary

Phase C.0 completes yserver's migration to atomic KMS for every operation that
mutates live display state. It replaces legacy hardware-cursor ioctls with
universal cursor-plane state, replaces legacy CRTC gamma with atomic
`GAMMA_LUT` property blobs, and introduces one device-local owner that orders
primary-plane, cursor-plane, color, modeset, unflip, DPMS, and topology commits.

This phase does not implement tearing, advertise
`PresentCapabilityAsyncMayTear`, or submit `PAGE_FLIP_ASYNC`. Its purpose is to
remove the unsupported legacy/atomic mixture and provide a complete atomic KMS
foundation before Phase C.1 raises primary-plane commit cadence above vblank.

## 2. Problem

yserver currently controls primary planes, direct scanout, composed unflips,
modesets, and DPMS with atomic commits, but still mutates KMS state through two
legacy families:

- hardware cursor through `SETCURSOR`/`SETCURSOR2` and `MOVECURSOR`;
- RANDR gamma through the legacy CRTC `set_gamma` ioctl.

Linux DRM deprecates the cursor entry points in favor of universal cursor
planes. Atomic-capable CRTCs expose color management through the `GAMMA_LUT`
blob property. Keeping either legacy family means yserver still has multiple
state-management paths that bypass the atomic transaction and its ordering.

The existing combination already has a measured failure history:

- the 2026-05 `bundle-cursor-atomic` experiment bundled cursor properties only
  with scene page flips, so cursor motion on an idle scene reached only about
  5–9 Hz;
- flushing cursor-only atomic state every loop iteration generated roughly 200
  `EBUSY` failures per second because the kernel had an earlier nonblocking
  commit pending;
- the production workaround moved cursor load/move back to legacy ioctls,
  restoring responsiveness but leaving two state-management models active on
  the same CRTC.

Phase C.1 will submit immediate primary-plane flips at high cadence. It must not
be built on this mixture. Hidden-only cursor support is insufficient, and
silently leaving gamma on the legacy path would make C.0's commit-owner
invariant false.

## 3. Goals

1. Express cursor image, framebuffer, CRTC binding, position, size, hotspot,
   visibility, animation, and detach through universal cursor-plane properties.
2. Express RANDR CRTC gamma through atomic `GAMMA_LUT` property blobs.
3. Eliminate legacy cursor and legacy gamma state-changing ioctls from the KMS
   backend on every device that reports C.0 ready.
4. Preserve visible cursor behavior on idle and animating desktops, during
   composition, and during fullscreen direct scanout.
5. Maintain responsive motion under high-rate input without blocking the X11
   core or generating an `EBUSY` storm.
6. Introduce one commit owner per DRM device that serializes every atomic
   state-changing operation affecting that device.
7. Coalesce pending cursor motion/image changes latest-wins while preserving
   cursor framebuffer and completion lifetimes.
8. Preserve Phase B direct scanout, synchronized page-flip pacing, cursor,
   cursor visibility, RANDR gamma, unflip, VT, DPMS, hotplug, and multi-output
   behavior.
9. Provide an explicit `atomic_kms_pipeline_ready` capability for Phase C.1.

## 4. Non-goals

- `PAGE_FLIP_ASYNC` or visible tearing.
- Present scheduling, async-option parsing, or Present capability changes.
- Variable refresh rate.
- Software-cursor composition redesign.
- Providing hardware cursor or programmable gamma on devices that expose no
  corresponding atomic property. Those devices use software cursor and report
  gamma unavailable; they cannot report C.0 readiness to C.1.
- Removing `DRM_IOCTL_CRTC_QUEUE_SEQUENCE`; it arms an observation event and
  does not mutate KMS display state.

## 5. Global invariant

For the KMS backend as a whole:

```text
all live KMS state mutation uses atomic commits
&& each mutation is ordered by the owning device-local commit owner
&& no legacy cursor/modeset/page-flip/gamma ioctl is reachable
```

`DRM_IOCTL_CRTC_QUEUE_SEQUENCE`, event reads, capability queries, framebuffer
allocation/import, and read-only property discovery are not state mutations and
remain outside the commit owner.

The modifier-less `ADDFB2` fallback is also outside this prohibition: it creates
a framebuffer object but does not change live CRTC/plane state.

## 6. Universal cursor-plane representation

Each active CRTC is assigned a distinct compatible universal cursor plane. The
existing per-card plane matching remains responsible for proving simultaneous
coverage; a single plane cannot satisfy two active CRTCs.

The desired cursor state contains at least:

```rust
struct AtomicCursorDesired {
    visible: bool,
    framebuffer: Option<CursorFramebuffer>,
    crtc: crtc::Handle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    hotspot_x: i32,
    hotspot_y: i32,
    generation: u64,
}
```

The KMS plane properties derive from the desired state:

- visible: `FB_ID`, `CRTC_ID`, `SRC_*`, and `CRTC_*` describe the cursor;
- hidden/detached: `FB_ID=0` and `CRTC_ID=0`;
- `CRTC_X/Y` use hotspot-adjusted CRTC coordinates;
- clipping and negative positions preserve partially off-screen cursors;
- the source rectangle remains in framebuffer coordinates and must not encode
  the hotspot twice.

The desired state is separate from submitted and retired state. Updating the
desired cursor never releases the framebuffer referenced by an in-flight or
currently scanned atomic state.

## 7. Atomic gamma LUT

Each CRTC discovers and retains the `GAMMA_LUT` property handle and reads
`GAMMA_LUT_SIZE`. The property is usable only when both exist and the size is
non-zero.

RANDR supplies three equally sized `u16` arrays. C.0 validates them against the
advertised size and encodes one array of DRM color-LUT entries:

```rust
#[repr(C)]
struct DrmColorLut {
    red: u16,
    green: u16,
    blue: u16,
    reserved: u16,
}
```

`reserved` is always zero. The exact kernel ABI layout is pinned by size,
alignment, and byte-level tests on supported targets. No Linux-specific libc
request alias is introduced.

The device creates a property blob from the complete LUT and submits its id as
the CRTC's `GAMMA_LUT` value through the device-local atomic owner. A gamma
change may be combined with a compatible pending modeset/primary/cursor intent,
or submitted alone when no such commit is imminent. It must not wait
indefinitely for scene damage.

### 7.1. Gamma blob lifetime

The owner tracks desired, pending, and current gamma blob ids per CRTC:

- failure before successful submission destroys the new unreferenced blob;
- a submitted blob remains alive through commit completion;
- the previously current blob is destroyed only after a newer commit proves it
  has been replaced;
- queued gamma changes coalesce latest-wins, destroying only blobs never
  submitted and no longer desired;
- VT, DPMS, hotplug, CRTC removal, shutdown, and device loss drain or retain
  blobs according to the same generation/ownership rules as planes.

Identity gamma is represented explicitly by an identity LUT blob unless the
driver documents and tests `GAMMA_LUT=0` as equivalent for that CRTC. Reset and
resume restore yserver's cached desired LUT; they do not silently reset client
gamma.

### 7.2. Unsupported color management

A CRTC without usable atomic `GAMMA_LUT` cannot report complete C.0 readiness
and must expose gamma as unavailable using Xorg-compatible RANDR semantics
(including a zero/absent gamma size or the corresponding protocol error chosen
by the implementation plan after checking Xorg). It must not fall back to
`set_gamma`. C.0 removes that call site from production KMS entirely.

`DEGAMMA_LUT`, `CTM`, HDR metadata, and color-pipeline extensions are separate
features. They are not required to preserve the existing RANDR gamma contract.

## 8. Device-local atomic commit owner

Every opened KMS device owns one scheduler. All state-changing call sites submit
intent to it rather than issuing `atomic_commit` directly.

The owner tracks:

- last retired/displayed state per CRTC and plane;
- at most one nonblocking atomic commit pending per affected CRTC;
- newest queued cursor intent per CRTC;
- desired/pending/current gamma blob and generation per CRTC;
- primary-plane/direct/unflip intents that must not be reordered;
- cursor framebuffer references for current, pending, and queued generations;
- topology/seat generation used to reject stale work.

### 8.1. Ordering classes

1. **Topology and ownership:** modeset, connector routing, disable, DPMS, VT
   suspend/resume, hotplug reconstruction.
2. **Primary replacement:** composed page flip, direct scanout, composed
   unflip.
3. **Cursor-only:** image, movement, animation, show/hide.
4. **Color-only:** RANDR gamma LUT replacement or identity restoration.

Topology changes invalidate queued intents from earlier generations. Primary
replacement, cursor, and color state may be combined when ready, but cursor or
gamma must not wait indefinitely for future scene damage.

### 8.2. Cursor and gamma progress on idle scenes

A cursor-only intent schedules its own prompt atomic flush when no compatible
primary commit is imminent. It is not gated on `scene_wants_compose`, damage,
or the next vblank-driven scene tick.

Gamma-only intent follows the same progress rule, while multiple unsubmitted
gamma updates coalesce to the newest complete LUT.

If a nonblocking commit is already pending, new motion overwrites the queued
cursor position. Image/show/hide changes replace older queued cursor state while
retaining any framebuffer still referenced by current or pending state. The
retirement/event wake immediately attempts the newest queued cursor intent.

The queue is bounded:

```text
per CRTC: one submitted atomic state + one latest desired cursor state
```

Gamma has the same bounded desired-state rule; a newer unsent LUT replaces and
destroys the older unsent blob.

### 8.3. `EBUSY`

`EBUSY` is a scheduling signal, not a reason to retry-spin, block the event
loop, or fall back to a legacy ioctl. The owner retains the newest desired
state, records telemetry, and retries after the relevant commit completion.

Repeated `EBUSY` without an owner-tracked pending commit is an invariant failure
and must be diagnosed separately rather than hidden by a busy loop.

## 9. Cursor lifecycle

### Load/change

- Upload/import a cursor framebuffer.
- Record a new desired generation.
- Retain the old framebuffer until every commit that references it retires.
- Coalesce multiple image/animation updates before submission to the newest
  complete state.

### Move

- Convert root coordinates to the owning CRTC and subtract hotspot exactly
  once.
- Deduplicate identical effective positions.
- Coalesce unsent motion latest-wins.
- Crossing outputs detaches the old CRTC plane and attaches the destination
  plane as one ordered device transaction when both CRTCs share a device.

### Hide/show

- Hide atomically detaches the plane; its completion retires the last scanned
  cursor framebuffer reference.
- Show restores the newest image, hotspot, and position even when they changed
  while hidden.
- XFixes hide/show and `Cursor=None` use the same state machine.

### Animation

- Animation deadlines update desired image state without forcing scene
  composition.
- Slow or pending KMS retains the newest animation frame; it never builds an
  unbounded queue.

## 10. Interaction with Phase B direct scanout

Phase B's primary-plane eligibility and synchronized flip behavior remain
unchanged. C.0 changes only how cursor state reaches KMS and how atomic commits
are ordered.

Direct entry must attach the current cursor state atomically or prove that the
already-submitted cursor plane state remains valid. Direct exit/unflip must not
drop or flash the cursor. A primary flip event cannot retire a newer cursor
generation merely because both share a CRTC.

Phase B must preserve the current gamma blob across direct entry, direct frame
replacement, and composed unflip. A primary-plane event must not retire a newer
cursor generation or gamma blob merely because they share a CRTC.

The existing hardware-cursor requirement becomes part of
`atomic_kms_pipeline_ready`. A device without atomic cursor coverage uses the
software cursor; one without atomic gamma exposes gamma unavailable. Neither
device can enable Phase C.1.

## 11. Multi-device and multi-output

- Commit ownership is per DRM device, never global and never based on device
  enumeration order.
- Every active CRTC maps to its owning device and a distinct cursor plane.
- Cross-device cursor movement submits independent ordered detach/attach
  operations; failure must leave at most one visible sprite and preserve a
  retryable desired state.
- Topology epochs invalidate queued cursor intents and framebuffer associations.
- A device without universal cursor-plane coverage or atomic `GAMMA_LUT`
  reports C.0 unavailable only for its own outputs.
- Unsupported devices never regain a legacy state-mutating ioctl path.

## 12. Telemetry

Provide counters or structured logs for:

- atomic cursor desired/submitted/retired;
- moves coalesced and identical moves deduplicated;
- cursor-only commits versus cursor bundled with primary updates;
- pending/queued high-water marks;
- `EBUSY`, stale-generation drops, and retry latency;
- cursor image/framebuffer generations current, pending, and queued;
- legacy cursor ioctl calls, which must remain zero on a C.0-ready device;
- input-to-cursor-submit and input-to-retirement latency;
- gamma desired/submitted/retired/coalesced;
- gamma blob created/destroyed/current/pending high-water;
- legacy gamma ioctl calls, which must remain zero on a C.0-ready device;
- RANDR-gamma-to-submit and submit-to-retirement latency.

Per-motion logs remain debug/trace only.

## 13. Verification

### Unit and state-machine tests

1. Plane-property encoding for visible, hidden, partially clipped, negative,
   hotspot-adjusted, and cross-output cursor states.
2. Identical position deduplication.
3. Multiple unsent moves coalesce to the newest coordinates.
4. Image replacement retains current/pending framebuffer lifetimes.
5. Hide followed by Show during a pending commit restores the newest sprite.
6. Cursor-only work progresses without scene damage or a primary flip.
7. `EBUSY` queues exactly one desired state and retries only after completion.
8. Primary and cursor intents preserve ordering when combined or separated.
9. Stale VT/DPMS/hotplug/topology generations cannot submit.
10. Cross-device/output movement never leaves duplicate visible cursor planes.
11. Direct entry/unflip preserves cursor visibility and framebuffer ownership.
12. Animated cursor coalesces without unbounded state.
13. C.0 readiness is false without complete universal-plane coverage.
14. No C.0 cursor operation reaches a legacy ioctl call site.
15. `GAMMA_LUT_SIZE` validation accepts exact arrays and rejects mismatch.
16. DRM color-LUT encoding preserves all RGB entries and zeroes `reserved`.
17. Gamma replacement/coalescing preserves desired/pending/current blob
    lifetime and destroys every superseded unsubmitted blob exactly once.
18. Gamma-only work progresses without scene damage.
19. Modeset, DPMS, VT, hotplug, direct entry/unflip, and shutdown preserve or
    safely retire the desired LUT.
20. C.0 readiness is false if any owned active CRTC lacks atomic cursor-plane
    coverage or atomic `GAMMA_LUT`.
21. No C.0 gamma operation reaches `set_gamma`.

### Hardware validation

At minimum on NVIDIA proprietary and AMDGPU/RADV:

- idle desktop with continuous cursor motion;
- 1000 Hz mouse motion and circular/diagonal movement;
- visible cursor over composed desktop and fullscreen direct scanout;
- animated cursors, image/name changes, hotspot changes, XFixes hide/show, and
  `Cursor=None` followed by restore;
- drag operations under MATE/Cinnamon without window lag;
- multi-output crossing, including different device owners where available;
- Alt-Tab/direct unflip/re-entry;
- VT switch, DPMS cycle, hotplug, and shutdown;
- zero legacy cursor ioctl calls on C.0-ready devices;
- zero sustained `EBUSY` storm and no cursor freeze, trail, flash, duplicate, or
  vblank-rate motion on an idle scene;
- `xrandr --gamma`/RANDR SetCrtcGamma and GetCrtcGamma round trips, identity and
  non-identity ramps, repeated rapid changes, DPMS/VT/hotplug persistence, and
  zero legacy gamma ioctl calls on C.0-ready devices.

Compare cursor latency and smoothness against the current legacy implementation.
C.0 cannot regress the workload that caused the legacy path to land.

## 14. Acceptance criteria

C.0 is complete when:

- the complete visible cursor lifecycle uses universal cursor-plane atomic
  state on supported devices;
- one device-local owner orders all atomic state mutation;
- an idle cursor is responsive independently of scene damage;
- high-rate motion is bounded/coalesced without retry spinning or an `EBUSY`
  storm;
- current and pending cursor framebuffer lifetimes are correct;
- RANDR gamma uses atomic `GAMMA_LUT`, round-trips correctly, progresses without
  scene damage, and retains/destroys property blobs with correct lifetime;
- Phase B direct scanout, unflip, synchronized pacing, VT, DPMS, hotplug,
  multi-output, and shutdown remain correct;
- supported devices report `atomic_kms_pipeline_ready=true` only after cursor
  plane coverage, atomic gamma discovery, and scheduler initialization succeed;
- unsupported cursor devices use software composition; unsupported gamma CRTCs
  expose gamma unavailable; neither path uses a legacy state-changing ioctl or
  enables Phase C.1;
- `cargo +nightly fmt -- --check`,
  `cargo clippy --all-targets -- -D warnings`, and
  `cargo test --all-targets --locked` pass.

## 15. Implementation boundary

C.0 is its own PR stacked on #129. It must land before C.1. Cursor conversion,
gamma conversion, and device-local commit ownership form one merge boundary:
switching call sites to atomic without bounded progress recreates the old cursor
failure, while retaining production legacy cursor or gamma calls does not
complete the migration invariant.

## 16. References

- `docs/status.md`, "HW cursor drag-lag fix" (2026-05-29)
- Phase B direct-scanout implementation and validation in PR #129
- Linux DRM/KMS documentation for universal cursor planes, atomic commits, and
  deprecated legacy cursor entry points
- Linux DRM color-management properties `GAMMA_LUT` and `GAMMA_LUT_SIZE`
- Successor spec (separate branch): Phase C.1 on-demand async direct-scanout
  tearing
