# Present direct-scanout M1 TEST_ONLY plan

**Goal:** Prove that the exact compositor-authoritative buffer observed by M0
can be imported as one DRM framebuffer and accepted by one atomic `TEST_ONLY`
transaction spanning every active CRTC, without changing live scanout.

**Branch:** `feat/direct-scanout-scope`.

## Measured candidate

The silence/RX 580 Cinnamon dual-head capture showed one authoritative
5120×1440 DRI3 XRGB8888 linear buffer Presented to a COW descendant. It has
zero Present offsets and no valid/update region. Its two source crops are
2560×1440 at `(0, 0)` and `(2560, 0)`. Warframe separately rotates four
2560×1440 buffers on output 0, but those remain redirected and are not M1
candidates.

## Task 1: exact cached plane state

- [x] Cache the primary-plane source/destination property handles discovered
  at modeset initialization.
- [x] Build one plane state per active output: source coordinates are the
  output's virtual-root rectangle in 16.16 units; CRTC coordinates are the
  whole local mode at `(0, 0)`.
- [x] Reject overlapping/out-of-bounds crops, mode/layout mismatches, missing
  outputs, non-XRGB8888 formats, non-zero offsets/regions, and non-authoritative
  targets before any DRM ioctl.

## Task 2: once-per-drawable FB import and TEST_ONLY

- [x] Run M1 probing by default on the direct-scanout feature branch; no
  environment gate is needed for branch-only hardware development.
- [x] PRIME-import the already-owned dma-buf and register one framebuffer with
  the retained modifier/plane metadata.
- [x] If and only if an explicitly tagged linear framebuffer receives
  `EINVAL`, retry legacy `ADDFB2` without the modifier flag so AMDGPU can use
  the imported BO's layout metadata. Never discard a non-linear modifier.
- [x] Submit one `AtomicModeReq` containing all affected primary planes with
  `AtomicCommitFlags::TEST_ONLY` only. Never use `PAGE_FLIP_EVENT`, `NONBLOCK`,
  or mutate Present capability/completion state.
- [x] Cache pass/reject per stable `DrawableId`; never issue probe ioctls once
  per frame.

## Task 3: teardown and diagnostics

- [x] Retain accepted FB/GEM handles in an RAII record tied to the DRM device.
- [x] Drop cached records on topology change, VT suspend, DPMS off, drawable
  retirement, and backend shutdown.
- [x] Emit one result line per new candidate and counters in the existing M0
  summary; preserve the current Copy/composition path in every case.

## Task 4: validation

- [x] Unit-test exact dual-head crops and rejection geometry.
- [x] Run `cargo +nightly fmt`.
- [x] Run focused tests.
- [x] Run `cargo clippy --all-targets -- -D warnings` exactly.
- [x] Update `docs/status.md`; direct flip remains disabled.

## Hardware gate

- [x] Cinnamon ordinary desktop with M1 enabled: no image/input regression.
- [x] Confirm each stable source is imported/probed once and ordinary/game
  redirected buffers issue no probe ioctls.
- [x] Re-run after the linear-only legacy `ADDFB2` compatibility retry; legacy
  registration succeeded for every eligible source.
- [x] Cinnamon dual-head Warframe: seven exact root candidates logged
  `TEST_ONLY passed`, with no rejects or errors.
- [x] Eiger negative control: software cursor fallback prevents any probe.
- [x] Proceed to M2 only after the above passes.
