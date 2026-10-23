# Hardware cursor motion does not compose scene frames

Short plan (no spec: this changes no behavior a client can see, only which commit carries a cursor move).

## Problem

With a hardware cursor over a static screen, every pointer move composed and flipped a full scene frame instead of a cursor-only commit. On NVIDIA RTX 5060 Ti, master `afd064e5`: 200 moves in 10 s produced 232 `atomic commit kind=scene-with-cursor` and 0 `cursor-only`. AMD Raphael gave the same pattern, 190 and 0.

Cause:

- motion damages the cursor footprint (`crates/yserver-render/src/kms/render/scene/cursor.rs`);
- a Hardware cursor assignment still counts that footprint as scene damage (`scene/build.rs`);
- the generic `mark_dirty` wake (`crates/yserver/src/kms/render/backend/trait_impl.rs`) then runs an ordinary scene frame.

## Reuse inventory

- **Existing, unchanged:** the cursor-only position path, which queues atomic cursor positions per CRTC and submits them on the existing per-CRTC stream (P2). It already handles motion whenever no scene frame is due.
- **Existing, used as inputs:** the per-output cursor mode, the cursor image `record_version`, the forced-show retry and the pending cursor transition in the scene tick.
- **New:** one condition in the scene tick (`scene/tick.rs`, where cursor damage is unioned into the frame damage). It skips the cursor-footprint damage only for a steady Hardware → Hardware change of position alone: same image version, no forced show, no pending transition. No new type, state or mechanism.

## Fallback

Everything outside that case keeps the previous damage: a software or mixed cursor, any image or hotspot change, any Hardware↔Software/Hidden handoff, a forced show, and any real window damage.

## Tests

Backend-driven, with the recording atomic executor:

- pure hardware-cursor motion over a static screen, including the generic wake, submits only cursor-only commits with the latest positions;
- motion plus a window paint still submits a scene frame;
- software-cursor motion still repaints;
- an image change during motion keeps the previous path.

Mutation: remove the condition.

## Validation

NVIDIA RTX 5060 Ti, release builds, master `afd064e5` vs this branch:

- **Pointer motion over a static screen:** 232 scene frames → 0, with 188 cursor-only commits; yserver CPU 2.3 % → 1.7 %.
- **Windowed 60 fps mpv with an animated cursor over it:** unchanged, with 1133 scene and 66 cursor-only commits and 0 dropped frames; CPU 4.3 % → 3.5 %.
- **Idle:** 0 % on both.

AMD Raphael, same builds: pointer motion 192 scene frames → 0 (190 cursor-only commits), CPU unchanged within noise (1.3 % / 1.5 %; the saving there is GPU composition and flips, not measured); windowed mpv unchanged, 0 dropped frames.
