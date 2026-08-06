# Present direct-scanout M0 measurement plan

**Goal:** Measure the live Cinnamon Present graph needed by the draft
[`2026-08-05-present-direct-scanout-design.md`](../specs/2026-08-05-present-direct-scanout-design.md)
without changing rendering, issuing DRM probe ioctls, or advertising Present
flip capability.

**Branch:** `feat/direct-scanout-scope`.

## Task 1: retain queryable imported-buffer metadata

- [x] Add a small immutable metadata record to imported drawable storage:
  fourcc/Vulkan format, modifier, plane offset, pitch, width, height, depth,
  and bpp.
- [x] Populate it at `dri3_import_pixmap`; server-owned storage reports no
  imported metadata.
- [x] Unit-test exact preservation, including non-zero offset/padded pitch.

## Task 2: expose a read-only Present candidate snapshot

- [x] Add a backend observer called at Present execution before Copy.
- [x] Resolve source by stable `DrawableId`, destination paint target, COW
  relationship, destination absolute geometry, root extent, output layouts,
  offsets, and update-region coverage.
- [x] Keep the observer incapable of consuming pins, changing damage, or
  submitting work.
- [x] Unit-test COW target, COW descendant, unredirected covering window, and
  ordinary ineligible window classifications.

## Task 3: change-deduplicated telemetry and counters

- [x] Log one line only when a destination's candidate shape changes or a new
  source buffer identity first appears.
- [x] Track per-second candidate Presents, rejection reasons, distinct-buffer
  count, and rotation depth without allocating/logging per frame.
- [x] Include per-output source crops for whole-root candidates.
- [x] Add telemetry unit tests proving a steady stream does not emit/logically
  allocate one record per frame.

## Task 4: software validation

- [x] Run `cargo +nightly fmt`.
- [x] Run focused unit tests for metadata and candidate classification.
- [x] Run `cargo clippy --all-targets -- -D warnings` exactly.
- [x] Update `docs/status.md` with M0 availability and the fact that no direct
  flip is enabled.

## Task 5: hardware measurement

- [x] Cinnamon ordinary desktop/control capture.
- [x] Cinnamon dual-head fullscreen Warframe capture.
- [ ] MATE control capture.
- [x] Record the exact source/destination geometry, COW relationship, buffer
  rotation depth, format/modifier, and per-output crop.
- [x] Decide: proceed to M1 TEST_ONLY, revise candidate model, or stop because
  no compositor-authoritative candidate exists.
