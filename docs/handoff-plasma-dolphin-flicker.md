# Handoff: Plasma/Dolphin alternating stale-frame flicker

## Scope

- GitHub Discussion: <https://github.com/joske/yserver/discussions/100#discussioncomment-17773692>
- Branch: `fix/plasma-present-import-coherency`
- Status: work in progress; the foreign-ownership change has not had a hardware
  retest.
- Reproduction machine/display: Plasma at 2560x1440. In Dolphin's detailed
  list view, move the pointer slowly from one item to the next. Previously
  hovered rows flicker on and off; KWin's status overlay can flicker too.

This is probably not a regression. Dolphin had not previously been tested in
this setup.

## Captured evidence

The latest local artifacts are `plasma.xtrace` (26 MiB), `plasma.log`, and
`yserver-hw-plasma.log`, captured at 09:39 on 2026-07-31. They are deliberately
not committed. The drawable dump directory also contains PPMs from older runs;
use timestamps and pixmap IDs rather than globbing every file together.

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

The current, untested follow-up also enables `VK_EXT_queue_family_foreign` and
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

1. On the original Plasma machine, run `target/debug/yserver` and repeat the
   slow Dolphin hover test. Also watch the status overlay.
2. Confirm startup does not emit Vulkan validation errors around
   `VK_QUEUE_FAMILY_FOREIGN_EXT` ownership transfers.
3. If the flicker remains, capture a fresh trace/log and clean or isolate old
   PPMs before dumping. Compare both imported source pixmaps again.
4. The next likely experiment is to instrument buffer contents at Present-copy
   time rather than only dumping the retained images afterwards. Correlate each
   source hash with Present serial and the producer sync-file state.
5. Do not merge solely on the unit test: the bug is hardware/coherency-specific.

The foreign ownership fix is intentionally limited to CopyArea, which covers
this Present path. If it succeeds, audit other imported-image consumers before
generalizing the ownership state machine.
