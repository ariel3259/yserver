# Present direct scanout for compositor final-stage buffers

**Status:** M0 measured; M1 atomic TEST_ONLY hardware-validated on silence.
M2a is scoped as an opt-in live flip retaining a shadow COW Copy for immediate
fallback. This supersedes the implementation assumptions in
[`../plans/2026-07-08-direct-scanout-fullscreen-scope.md`](../plans/2026-07-08-direct-scanout-fullscreen-scope.md),
but preserves its capability result: an RX 580 client dma-buf can be imported
as a DRM framebuffer and accepted by an atomic `TEST_ONLY` commit.

**Branch:** `feat/direct-scanout-scope`.

## Problem and measured motivation

Fullscreen Warframe under Cinnamon/Muffin on silence (RX 580, dual head)
settles around 15--20 page flips/s. The same workload is usable under MATE and
smoother under Xorg. Current telemetry rules out the previously suspected
single-threaded Present wait and a growing Vulkan/fence backlog:

- Present request handling remains sub-millisecond;
- submitted Vulkan queue depth remains bounded at 1--6;
- no CPU fence waits occur;
- yserver still records roughly 140--170 paint submits/s plus a full-output
  scene compose for each of the 15--20 displayed frames;
- every displayed frame visits the COW subtree and redraws the full output.

The destination-wait work fixes a real independent 50 ms request-loop stall,
but cannot remove Cinnamon's extra rendering stages. Direct scanout is now
motivated by this measured heavy-game workload, not by the July fullscreen
video observer artefact.

## Which buffer may be scanned out

There are two superficially similar candidates with very different
correctness properties.

### Rejected as an implicit optimization: the redirected game buffer

Muffin owns redirected application windows. Scanning Warframe's buffer while
Muffin still owns that redirect would bypass compositor policy and omit panels,
notifications, effects, transforms, color processing, and any other content
Muffin deliberately included. Yserver must not infer that fullscreen means
the compositor consents to this bypass.

The application buffer becomes eligible only after an explicit Composite
unredirect makes the covering window scene-participating. That remains a
valid secondary path, using the existing COW-suppression predicate, but the
earlier live measurements reported `participating=false` for Cinnamon's
fullscreen application.

### Primary target: Muffin's final-stage Present source

Muffin has already rendered the authoritative final desktop into the DRI3
pixmap it Presents to its full-screen stage/COW destination. Scanning that
pixmap is compositor-safe: yserver displays exactly the compositor-produced
image and does not second-guess its policy.

This path can remove both downstream operations:

1. Present Copy from Muffin's source pixmap into the COW/stage backing;
2. yserver's scene composition of that backing into a private `ScanoutBo`.

The Xorg compatibility rule supports this boundary. Local xserver source
(`present/present_scmd.c::present_check_flip`) rejects Composite-redirected
windows, requires the destination window pixmap to be the screen pixmap (or
the current/pending flip pixmap), requires the window clip to equal the whole
root, and requires zero offsets plus matching pixmap/window dimensions. The
modesetting driver then checks scanout format/modifier support and flips the
client pixmap. Yserver should reproduce the observable rule, adapted to its
explicit COW/stage model, rather than invent a fullscreen-app bypass.

## Current architecture after #117

The July scope is stale in two important ways.

First, the orphaned `PresentScheduler`, `PresentPath`, path selector, and
`last_flipped` map were deliberately deleted. They must not be resurrected.
#117's unified `present_pending_exec` store is now the live scheduler.

Second, several prerequisites now exist:

- every parked Present owns an opaque source pin immune to XID reuse;
- implicit dma-buf producer readiness and explicit syncobj acquire points are
  waited asynchronously;
- Copy execution is deferred toward the effective MSC;
- completion delivery is ordered by `present_id`;
- wake primitives remain pinned until core authorizes their signal;
- page-flip completion carries a distinct display-retirement clock source.

The missing ownership transition is narrow but load-bearing: on a direct
flip, the entry's source pin must transfer from `present_pending_exec` to a
per-scanout retirement record instead of being released immediately after
Copy execution.

## M0: observe the real Present graph before choosing geometry

The retained logs do not contain enough identity/geometry information to
decide whether Muffin Presents one root-sized pixmap, one pixmap per output,
or another layout. M0 therefore adds change-deduplicated telemetry only. It
must perform no DRM import and no ioctl in the per-frame path.

For each distinct `(destination, source drawable identity, geometry shape)`
seen at Present execution, record:

- client id, client source XID, stable backend `DrawableId`, and Present id;
- destination window XID, paint destination host XID, and the resolved paint
  target `DrawableId`;
- whether the target is the COW, a COW descendant/stage, an unredirected
  scene-participating window, or neither;
- source width/height/depth/bpp, imported-vs-server-owned status, DRM fourcc,
  modifier, per-plane offset and pitch;
- destination absolute rectangle, root extent, each active output rectangle,
  zero/non-zero Present offsets, and full/partial update/valid regions;
- whether one source covers the whole root, exactly one output, or neither;
- stable distinct-buffer count and rotation sequence per destination;
- eligibility rejection reason, with counters rather than per-frame logs.

M0 exit gate for the Cinnamon path:

1. identify the final-stage Present destination and prove it is authoritative
   for the whole visible root or for a specific output;
2. observe at least two rotating imported source buffers;
3. derive the exact per-output source rectangles;
4. show that the candidate rate tracks Cinnamon's displayed-frame cadence;
5. confirm that ordinary desktop, MATE, and non-covering Presents remain
   ineligible.

If no authoritative final-stage candidate exists, stop. Direct scanout cannot
fix this workload without compositor cooperation.

The 2026-08-05 silence capture passed the primary gate. Muffin's source is one
linear XRGB8888 5120×1440 dma-buf Presented to a COW descendant, covering two
2560×1440 outputs with source x offsets 0 and 2560. Warframe's separate
output-sized four-buffer stream remains redirected and is excluded. Under the
slow workload the root-stage rate is 6--10/s while the game continues at
22--24/s; per-output flip telemetry is approximately twice the root-stage
rate. Muffin reused a stable stage drawable during steady gameplay, so M2 must
still audit Xorg-compatible idle ownership before enabling a real flip.

## M1: framebuffer import and exact atomic TEST_ONLY

M1 is still no-flip. Retain the DRI3 import metadata currently discarded after
`DrawableImage::from_dmabuf`: fourcc, modifier, plane offsets, and pitches.
Use the already-owned dma-buf fd to build a cached DRM framebuffer record keyed
by stable `DrawableId` plus storage generation.

For each new eligible buffer only:

1. duplicate/import the dma-buf into the KMS DRM file;
2. register it with `AddFB2WithModifiers`, with the established modifier-less
   retry only when the DRI3 metadata is demonstrably incomplete;
3. build the exact primary-plane state needed by the candidate, including
   `SRC_X/Y/W/H` and `CRTC_X/Y/W/H` for every affected output;
4. issue one atomic `TEST_ONLY` transaction covering all affected CRTCs;
5. tear down every FB/GEM handle on rejection, drawable destruction, storage
   replacement, VT loss, DPMS topology change, and shutdown.

No probe ioctl may run once per frame. The deleted July chain initially did
roughly five DRM ioctls per Present and caused choppy cursor/video by itself;
commit `20d5124b` corrected it to once per buffer. The complete recovered chain
is preserved at `archive/direct-scanout-m1-full` (`c8873d3f`) as reference,
not as code to cherry-pick into the post-#117 architecture.

M1 exit gate passed on silence: seven distinct live Cinnamon candidates passed
TEST_ONLY with exact dual-head source cropping, with no rejects/errors or
per-frame reprobes. Eiger remained safely ineligible because apple_drm forced
the cursor to yserver's software-composition path.

## M2: real flip and ownership transfer

Add a backend Present execution result with at least:

- `Copied` -- existing path and lifetime;
- `DirectFlipSubmitted` -- backend consumed the entry pin and owns completion;
- `FallbackCopy` -- eligibility/import/commit failed before ownership transfer.

On direct submit:

1. do not record `copy_area`, COW damage, or a scene compose for affected
   outputs;
2. pass the producer readiness fence to KMS as `IN_FENCE_FD` when available;
   readiness already observed by userspace is not a substitute for preserving
   the explicit fence handoff where the protocol supplies one;
3. atomically bind the imported FB on every affected primary plane;
4. retain the source pin, FB/GEM record, wake pin, and Present event until all
   affected page-flip events retire;
5. on retirement, emit `CompleteNotify { mode: Flip }` for the new frame and
   idle/release the *previous* scanned Present buffer, matching Xorg's
   `present_flip_notify` then `present_flip_idle` ordering;
6. keep the newest scanned buffer pinned until a replacement direct flip or an
   unflip back to yserver's normal `ScanoutBo` completes;
7. retain per-window `present_id` ordering across Copy, Skip, Flip, and unflip.

The existing `PendingAck`/`PageFlipRetirement` model assumes every flip uses a
pool `ScanoutBo`. M2 must generalize the per-output in-flight record to a
tagged source (`ComposedBo` or `DirectPresent`) rather than fabricating a pool
index for an alien buffer.

## Multi-output transaction rule

Xorg's flip eligibility is whole-X-screen, and its modesetting pageflip drives
all active CRTCs for a root-sized pixmap. If M0 observes a root-sized Muffin
stage buffer on dual head, yserver must likewise treat all affected outputs as
one transaction:

- one eligibility decision;
- one atomic request containing every affected primary plane;
- one retained source with an outstanding-retirement count/set;
- completion only after every affected CRTC reports retirement;
- all-or-fallback before commit; never compose one output while directly
  scanning another from the same whole-root Present unless M0 proves Muffin
  supplies independent per-output buffers.

This differs from the July per-output fullscreen-window assumption and is why
M0 geometry is mandatory.

## Fallback and unflip

The existing compose path remains authoritative. Any rejection before a
successful atomic commit falls through to Copy with unchanged client-visible
semantics.

After direct scanout is active, any of the following schedules an unflip to a
freshly composed normal `ScanoutBo`: ineligible successor, destination
unmap/destroy/resize, stacking or shape change, cursor strategy incompatibility,
output topology/mode change, VT/DPMS transition, framebuffer invalidation, or
atomic failure. The directly scanned source is not idled until that unflip
retires.

Cursor policy is part of eligibility. Hardware cursor is compatible when all
affected outputs can maintain their cursor plane. A required software cursor,
root XOR overlay, or other yserver-owned overlay forces compose/unflip.

## Capability advertisement

Keep `PresentCaps.flip_path=false` throughout M0 and M1. Advertise flip/async
capability only after M2 is hardware-validated with correct fallback and
retirement. Capability is a protocol promise, not a telemetry switch.

## Exclusions

- PR #95 (`prime`) is an external, unmerged split-GPU scanout-ownership PR and
  is not infrastructure for this work.
- Top-level occlusion culling does not remove the COW subtree and is not direct
  scanout.
- Partial repaint and hardware overlay planes are separate optimizations.
- Directly scanning a redirected application behind the compositor's back is
  out of scope.

## Validation gates

- Unit tests: pure eligibility/rejection matrix; pin-transfer state machine;
  Copy/Skip/Flip ordered completion; multi-CRTC retirement; every teardown and
  failed-commit rollback.
- Live Vulkan tests: dma-buf metadata retention and FB-cache lifecycle where a
  render node/KMS test fixture permits it.
- Hardware M0/M1: silence RX 580 dual head; ordinary Cinnamon desktop plus
  fullscreen Warframe candidate capture; MATE negative/control run.
- Hardware M2: silence first, then eiger/air and a stronger GPU. Require clean
  fullscreen enter/leave, Alt-Tab, workspace switches, DPMS, VT switch,
  cursor movement, notifications/overlays, and dual-head independent activity.
- Performance exit: during an eligible Cinnamon stage stream,
  `composite_submits/s` and the corresponding full-frame Present Copy collapse
  toward zero while page flips track the compositor cadence and input remains
  responsive.
