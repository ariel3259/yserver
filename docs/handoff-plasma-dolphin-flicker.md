# Handoff: Plasma/Dolphin alternating stale-frame flicker

## Scope

- GitHub Discussion: <https://github.com/joske/yserver/discussions/100#discussioncomment-17773692>
- Branch: `fix/plasma-present-import-coherency`
- Status: work in progress; copy-time tracing reproduced the issue and exposed
  a dma-buf reservation-fence snapshot race. A re-export/repark experiment was
  laggy and was reverted after the same run ended in an unrelated-client GPU
  VM fault.
- Reproduction machine/display: Plasma at 2560x1440. In Dolphin's detailed
  list view, move the pointer slowly from one item to the next. Previously
  hovered rows flicker on and off; KWin's status overlay can flicker too.

This is probably not a regression. Dolphin had not previously been tested in
this setup.

## Captured evidence

The earlier drawable-snapshot artifacts were `plasma.xtrace` (26 MiB),
`plasma.log`, and `yserver-hw-plasma.log`, captured at 09:39 on 2026-07-31.
They are deliberately not committed. The drawable dump directory also contains
PPMs from older runs; use timestamps and pixmap IDs rather than globbing every
file together. The newer traced reproduction is described below.

For the latest run, KWin alternates these full-screen DRI3 Present sources:

- host pixmap `0x400066` / protocol pixmap `0x00800029`
- host pixmap `0x400073` / protocol pixmap `0x0080001f`

The 16 captured snapshots of each source are internally identical, while the
two sources differ. Cropping the Dolphin window at `760x578+951+454` shows one
source retaining hover fills for Desktop, Documents, and Downloads, and the
other retaining GNUstep. This matches the visible trail. Earlier captures had
the same shape with fewer stale rows.

Important eliminations:

- Dolphin's redirected backing is correct.
- Every Dolphin repaint appears in Damage, translated to the KWin frame by the
  expected `+28` y offset.
- KWin submits full-frame Present requests (`update=0`) and alternates its two
  pixmaps normally.
- Present serials and idle/completion events are monotonic.
- No pointer motion or focus loop explains the stationary flicker.
- The scanout path is not losing partial damage; stale pixels already appear
  when Vulkan reads one of KWin's imported source images.

`x11trace` prints the 64-bit Present MSC/UST halves in a misleading shifted
form. Server `present_pace` logs show the actual MSC values and should be used
for pacing analysis.

## Changes in this branch

The first attempt changed imported image layout tracking from
`VK_IMAGE_LAYOUT_UNDEFINED` to `VK_IMAGE_LAYOUT_GENERAL` and returned imported
images to `GENERAL` after CopyArea. This was necessary because imported dma-bufs
already contain client pixels, but the hardware retest still reproduced the
flicker and hover trail.

The follow-up also enables `VK_EXT_queue_family_foreign` and
wraps CopyArea use of imported images in explicit ownership transfers:

1. acquire `VK_QUEUE_FAMILY_FOREIGN_EXT -> graphics queue family` before the
   transfer;
2. perform the image copy;
3. release `graphics queue family -> VK_QUEUE_FAMILY_FOREIGN_EXT` in
   `VK_IMAGE_LAYOUT_GENERAL`.

Server-owned images retain the existing shader-read terminal layout. Drivers
without the foreign-queue extension retain the layout-only fallback. The test
machine advertises extension revision 1 according to `vulkaninfo`.

Changed implementation files:

- `crates/yserver-core/src/backend/recording.rs`
- `crates/yserver-core/src/backend/trait_def.rs`
- `crates/yserver-core/src/core_loop/process_request.rs`
- `crates/yserver/src/kms/vk/device.rs`
- `crates/yserver/src/kms/render/store.rs`
- `crates/yserver/src/kms/render/frame_builder.rs`
- `crates/yserver/src/kms/render/engine.rs`

`docs/status.md` records the investigation and the failed first retest.

## Validation completed

- `cargo +nightly fmt`
- `cargo test -p yserver copy_returns_imported_images_to_external_general_layout`
- `cargo check --workspace`
- `cargo clippy --all-targets -- -D warnings`
- `cargo build -p yserver`
- `git diff --check`

All passed before this handoff was committed.

## Next steps

1. Fix the invalid no-modifier dma-buf fallback exposed by the validated
   reproduction. When `VK_EXT_image_drm_format_modifier` is absent, probe
   `VK_IMAGE_TILING_LINEAR + DMA_BUF` with the exact import usage instead of
   assuming LINEAR is supported. Do not advertise or import an unsupported
   combination; determine the valid staging/copy fallback needed on this RADV
   device.
2. Fix the scanout-semaphore lifetime VUIDs emitted during the startup RandR
   reconfiguration, then rerun standard validation to get a clean baseline.
3. Hardware-test the new Present source-consumer synchronization described
   below. It exports yserver's exact copy-completion sync-file and imports it
   onto the source dma-buf as a shared READ fence.
4. Keep `just yserver-plasma-hw-trace` for diagnosis; it already enables
   `YSERVER_PRESENT_COPY_TRACE=1`. Do not reintroduce the re-export/repark loop
   without resolving the lag seen in its first hardware run.
5. Use `just yserver-plasma-hw-vkdebug` for the standard-validation retest.
   This trace-disabled recipe enables standard
   Vulkan validation and abort-on-device-loss while preserving the ordinary
   captures. Check `yserver-hw-plasma-vkdebug.log` and `plasma-vkdebug.log` for
   VUIDs, particularly around the `VK_QUEUE_FAMILY_FOREIGN_EXT` ownership
   transfers. Synchronization validation plus `RADV_DEBUG=syncshaders`, and
   then synchronization validation plus `RADV_DEBUG=hang`, were both too slow
   for an interaction repro; the recipe now omits all three and uses a release
   build.
6. Do not merge solely on unit tests: the bug is hardware/coherency-specific.

The foreign ownership fix is intentionally limited to CopyArea, which covers
this Present path. If it succeeds, audit other imported-image consumers before
generalizing the ownership state machine.

## 2026-07-31 foreign-ownership retest

The issue remained after the explicit foreign queue-family transfers. Two
Ctrl-Alt-F12 dumps taken ten seconds apart again showed two distinct hover
histories in KWin's alternating full-screen sources (`0x400081` and
`0x400090`). Every repeated read of one source within a dump was identical,
but that is not a temporal sequence: the stop-the-world dump rereads the same
live drawable while request processing is blocked. Three Dolphin rows were
visibly flickering during the second snapshot. Vulkan validation was requested
but the validation layer was not installed, so the run cannot rule out VUIDs.

For the next run, start with `YSERVER_PRESENT_COPY_TRACE=1`. The diagnostic
performs a full source readback immediately after the producer wait becomes
ready and before the real Present copy, then emits `PRESENT-COPY-SOURCE` with
the Present id/serial, host source/destination, deferred wait id, fresh dma-buf
reservation-fence state, extent/depth, and deterministic FNV-1a content hash.
Correlate those lines with `PACE-INSTR`. This is intentionally timing-invasive;
record whether enabling it changes or suppresses the flicker.

## 2026-07-31 copy-time trace reproduction

The timing-invasive trace eventually reproduced the visible flicker. The new
local artifacts are `yserver-hw-plasma.log` (51 MiB), `plasma.xtrace` (108
MiB), and `plasma.log`, covering 09:24:22--09:26:01 UTC. KWin alternated host
sources `0x40006d` and `0x400088` into destination `0x40006a`. There are 611
full-screen copy-time samples: 306 from the first source and 305 from the
second; 519 copied immediately and 92 woke from a deferred source wait.

Of those 611 fresh pre-copy sync exports, 604 were signaled and seven were
still pending. All seven pending samples followed a deferred wait:

- serials 3, 511, 537, and 581 from `0x40006d`;
- serials 4, 10, and 20 from `0x400088`.

The late serials 511, 537, and 581 occurred close to the observed reproduction.
This is direct evidence of a time-of-check/time-of-use race:
`DMA_BUF_IOCTL_EXPORT_SYNC_FILE` snapshots the reservation object's current
fences, but after that sync-file signals the producer can replace the exclusive
fence with a newer write before yserver copies. Waiting on the old snapshot is
therefore insufficient.

An experiment re-exported implicit synchronization after every deferred wake
and reparked if it found a newer producer fence. It used the exact pinned source
drawable rather than a fresh XID lookup. The first untraced hardware run felt
severely laggy and ended in the GPU reset described below, so the experiment was
reverted. Rechecking a moving reservation object can chase later producer work
and still cannot close the post-check window; the next fix should couple the
consumer completion fence and Present idle lifecycle instead.

## 2026-07-31 revalidation experiment failure

The untraced run started at 09:37:33 UTC. The desktop felt laggy before the
fault. At 09:38:16 the kernel reported two AMDGPU VM protection faults assigned
to `wezterm-gui` PASID 558, followed by a compute-ring timeout and GPU reset.
Starting one second after the first VM fault, yserver logged 115 GLX-TFP
write-wait timeouts; after the reset RADV cancelled yserver's context as
innocent and Vulkan returned `ERROR_DEVICE_LOST`. The ordering means those
timeouts are reset fallout, not evidence that they caused the fault. The log
does not establish whether the revalidation timing contributed to the WezTerm
fault, but its pre-fault lag is enough to reject the implementation.

## 2026-07-31 validated reproduction

Standard Khronos validation was fast enough to reproduce the Dolphin flicker
from 10:07:51--10:09:04 UTC. No GPU fault or device loss occurred, and the
ordinary Present workload emitted no queue-family-ownership VUID. In
particular, validation did not reject the `VK_QUEUE_FAMILY_FOREIGN_EXT`
barriers.

It did expose a more fundamental issue in the imported-image path. This RADV
device does not expose `VK_EXT_image_drm_format_modifier`. Yserver's fallback
therefore advertises `DRM_FORMAT_MOD_LINEAR` unconditionally and creates client
images as `VK_IMAGE_TILING_LINEAR` with `DMA_BUF` external memory. RADV's
capability query returns `VK_ERROR_FORMAT_NOT_SUPPORTED` for that exact format,
tiling, usage, and handle combination. Validation reported ten client
`vkBindImageMemory` imports whose dma-buf handle type was not importable before
hitting its duplicate limit. Continuing after that is undefined Vulkan
behavior and directly covers KWin's alternating Present sources, so this must
be corrected before attributing the flicker solely to reservation fences.

Other actionable but distinct VUIDs were ten in-use semaphore destructions
during the startup RandR reconfiguration, two startup draws expecting
`SHADER_READ_ONLY_OPTIMAL` while validation tracked `UNDEFINED`, four unreset
query reads, and two shader-feature errors. Ctrl-Alt-F12 itself added two
scanout-dump staging-buffer usage VUIDs; nine later negative-scissor VUIDs are
also separate from the imported-source issue. These need cleanup, especially
the semaphore lifetime violation before another GPU-fault investigation, but
none appeared as an ownership error at the moment of the flicker.

## 2026-07-31 Present consumer READ-fence implementation

The branch now publishes each implicit-sync `PresentPixmap` copy as a dma-buf
consumer. Core passes the source host xid to the backend completion hook. The
backend clones the imported source dma-buf fd so it survives client pixmap
destruction, and the pending completion entry carries that fd through both
submission paths. Once Vulkan exports the semaphore signaled either by the
copy-containing COW submission or by a same-queue submit ordered immediately
after a standalone copy, yserver imports that same sync-file onto every source
dma-buf with `DMA_BUF_SYNC_READ`. The kernel duplicates the sync-file;
yserver can therefore continue polling its copy for Present completion while a
future client write waits for the read to finish.

This publishes the consumer dependency without repeatedly snapshotting and
chasing the producer's moving reservation object, but the retest below shows
that it does not close the non-atomic handoff window. `PresentPixmapSynced` remains on its
explicit syncobj path and deliberately does not receive an implicit fence.
Unsupported import-sync-file ioctls retain the old behavior; other ioctl
failures are logged with the Present serial. The next decisive step is an
ordinary hardware reproduction attempt with `just yserver-plasma-hw`. If the
flicker remains, repeat with `just yserver-plasma-hw-vkdebug` and retain both
logs.

## 2026-07-31 consumer READ-fence retest

The ordinary release recipe reproduced within roughly fifteen seconds. The
run logged no READ-fence import failure, GPU fault, or device loss. In addition
to the familiar Dolphin-row flicker, a transient artefact appeared near the
bottom of the window; the snapshot dump probably did not capture it. Publishing
the consumer fence is therefore not the complete fix.

The kernel dma-buf documentation explains the remaining synchronization hole:
the userspace export/submit/import sequence is not atomic. Yserver currently
waits asynchronously for the exported producer snapshot before it records and
submits the copy. KWin can submit another write after that snapshot signals but
before yserver imports its completion READ fence; that already-submitted write
cannot retroactively acquire the new dependency. The rejected repeated-repark
experiment observed exactly these replacement fences but made the desktop too
slow.

`just yserver-plasma-hw-synctrace` is the next low-overhead discriminator. It
uses a release build with no x11trace, readback, or Vulkan validation. After a
successful READ-fence import it immediately exports and polls the dma-buf's
writer scope, logging `PRESENT-SYNC`. A `post_publish_writer=pending` result
means a newer producer write entered the non-atomic gap. If the artefact
reproduces without any pending result, prioritize the already-validated invalid
no-modifier Vulkan import on this Polaris system. The architectural sync fix is
to import the producer sync-file into a temporary Vulkan binary semaphore and
queue the copy wait immediately, rather than parking the X request until the
producer fence signals.

The sync trace reproduced with 407 `PRESENT-SYNC` samples, and every sample
reported `read_fence=published post_publish_writer=pending`. This includes the
entire alternating KWin pair, sources `0x400081` and `0x400091`. Thus an
unsignaled write-scope reservation fence was present immediately after every
consumer publication; the old CPU-wait design did not establish an exclusive
handoff interval. (The state-only probe cannot name the fence owner, so a
driver-added reservation fence for yserver's own Vulkan access remains a
possible contributor to that count.)

An experiment then implemented the intended Vulkan interop sequence: import the
initial producer sync-file temporarily into a fresh binary Vulkan semaphore,
record the copy immediately, and wait on that semaphore in the copy-containing
submit. Despite passing compile, unit, and clippy validation, it caused another
GPU fault on the first ordinary hardware run. The fault reproduced after a
clean reboot with nothing else running. Yserver's log shows RADV cancelling its
context as innocent and `ERROR_DEVICE_LOST` at 11:30:12 UTC, about eleven
seconds after Vulkan startup; the application log does not identify the
kernel's initiating context. The entire Vulkan-wait experiment was reverted.
Do not retry it until the invalid no-modifier import and startup semaphore
lifetime VUIDs have been fixed and the resulting Vulkan baseline is clean.
