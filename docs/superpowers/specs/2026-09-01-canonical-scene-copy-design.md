# Canonical scene copy design

> **Status, recorded 2026-09-09 when `fix/clipped-noncomposited-repaint` was
> deleted.** Neither half of this design has landed, and both halves are still
> referenced by live code or an open bug, which is why the document was lifted
> onto master rather than deleted with the branch.
>
> - **The canonical scene image (§Design 1-4, plan 1-4) is NOT superseded.**
>   What landed was damage-clipped repaint into the reused scanout BO
>   (`02bafec3`, plus the follow-up repairs in `8d448583`) — the thing §Problem
>   describes, not the fix for it. `scene.rs`'s `Repaint::AuditClearClipped`
>   exists on master and its own doc comment says it "models the future
>   canonical image update rule without changing production scanout", so the
>   diagnostic is on master and the design it points at lived only on that
>   branch.
> - **§Resize preservation (plan 5-6) is the fix shape for an open bug** —
>   window resize reallocates storage and neither background-fills the newly
>   exposed area nor gravity-copies the old pixels, which Xorg does not do
>   (verified against a real Xorg trace, quoted under §Reference behaviour).
>   The `bitGravity`/`ForgetGravity` rule and the "core already emits the
>   `Expose`, KMS must not synthesize a duplicate" caveat are the parts worth
>   not rediscovering.
>
> The deleted branch (tip `94a5ea8e`) held 22 commits that are mostly
> apply/revert pairs: bit gravity applied and reverted twice, "preserve window
> pixels across resize" applied and reverted twice, clipped repaint enabled and
> reverted. **The spec outlived every attempt at it**; read it before starting a
> third.

## Problem

The KMS compositor currently reconstructs a changed region directly into a
reused scanout BO with `LOAD`. That BO is not an authoritative desktop image:
pixels outside shaped windows, alpha surfaces, root overlays, and regions
whose visibility changed can contain data from an unrelated old frame. The
result is transient black decoration bars and menu flicker.

## Reference behaviour

Xorg modesetting keeps the root pixmap as the authoritative desktop. Its
TearFree path accumulates root-pixmap DAMAGE per CRTC and copies those regions
from the root pixmap into each flip buffer before queueing the flip.

Window resize is a separate X11 correctness requirement. The local Xorg trace
shows repeated `ConfigureWindow` requests followed by narrow `Expose` regions
and `CopyArea` operations that preserve existing window pixels. Xorg uses the
window's `bitGravity` (default `ForgetGravity`) to determine which old pixels
survive; the remainder is exposed for the client to repaint.

## Design

Each output owns a device-local canonical scene image. It is the sole target
for scene composition and is always complete after a successful update.

1. On first use, topology change, failed rendering, or an unrepresentable
   structural update, compose the complete scene into the canonical image.
2. Otherwise, rebuild the current damage on the canonical image. The render
   pass must establish the root/background base within every repaint rect
   before window draws, so shaped and alpha content are correct.
3. Maintain per-scanout-BO pending damage exactly as Xorg does: union every
   canonical-image update into every reusable BO's pending region.
4. Before a BO is flipped, copy its pending rectangles from the canonical image
   into that BO, then clear that BO's pending region only after the flip
   retires successfully.

The scanout BO is therefore never used as a scene-composition source. Its age
is an implementation detail, not a correctness contract.

### Resize preservation

The KMS drawable path must mirror Xorg's ordering:

1. Keep old drawable storage and its in-flight fence alive.
2. Allocate new storage and initialize newly exposed area from the background.
3. If `bitGravity != ForgetGravity`, copy the gravity-translated intersection
   of the old visible region into the new storage. Do not copy for
   `ForgetGravity`.
4. Atomically switch the xid to new storage, then release old storage only
   after queued GPU work retires.
5. Mark old and new scene coverage dirty. Core emits the visible-window
   `Expose`; KMS must not synthesize a duplicate.

The copy must be a transaction separate from an open client paint batch. A
failed allocation, copy, or submission leaves the old mapping intact.

## Non-goals

This phase does not change direct scanout, Composite Overlay Window handling,
or introduce a public DAMAGE-extension dependency. Direct scanout remains a
separate path. COW is composed into the canonical image as a normal scene.

## Acceptance

- No stale pixels while moving/resizing MATE windows or opening shaped menus,
  with and without Marco compositing.
- Interactive resize preserves explicit `bitGravity` content and never loses
  window or panel contents when several configures arrive before repaint.
- The no-compositor Awesome/mpv case no longer rasterizes the complete desktop
  on every frame; telemetry shows bounded scene damage and copy work.
- Full redraw remains the recovery path after modesets, failed submits, and
  canonical-image recreation.
