# Two test regressions this branch introduced, found while validating the upstream merge

**Date:** 2026-09-18. **Tree:** merge `d4dd495c` (joske/master `2ea40635` into this branch), not pushed.
**Found by:** the first run of upstream's `render_acceptance` hardware suite on this branch, from a bare VT.

## Summary

Validating the merge on hardware surfaced three failures. **None was introduced by
the merge** — each fails on this branch's pre-merge commit `46198717` too — and
two of them are regressions this branch caused in upstream tests months of
review never saw, because **this branch's hardware gate only ever ran
`cargo test -p yserver --lib c0_2ci -- --ignored`**, never `render_acceptance`
nor upstream's other ignored hardware tests.

| Test | Cause | Status |
| --- | --- | --- |
| `c0_2ci_sink_gamma_gate_four_states_drm` | environment: on a VT with no display server, the kernel makes the first opener of a primary node DRM **master**, so the test's "no master" fd has master and the gamma write succeeds | cause proven, fix validated (not yet committed) |
| `render_acceptance::compose_then_fill_then_get_image_returns_second_fill` | this branch's `f475b04c` | cause proven |
| `render_acceptance::set_container_background_pixmap_tiles_across_root` | this branch's `f475b04c` | cause proven |

## The gamma test: auto-granted master

Measured: a freshly opened `/dev/dri/card0` and `card1` fd **is already master**
on the active tty session with no compositor (`DRM_IOCTL_DROP_MASTER` succeeds
on it). The fixture opens its node through `TestDevice::open_real_drm_or_ignore`
expecting no master — true on a desktop, where the compositor holds it, false
on a bare VT. It passed on tty2 on 2026-09-17 only because the suite ran in
parallel and another test had opened the card first; run serially, it fails.

**Fix, validated on the bare VT:** the two test openers
(`open_real_drm_or_ignore`, `open_real_drm_matching`) drop any auto-granted
master right after opening (`DRM_IOCTL_DROP_MASTER`; `EINVAL` means the fd was
not master, which is the wanted state). With it, the gamma test passes on the
bare VT, and the part-3 live-KMS fixture — which re-acquires master on top —
still passes all three of its tests.

## The two `render_acceptance` tests: `f475b04c`

`git bisect` over 297 commits (`10095624` good, `46198717` bad; both tests
checked at each boundary) names **`f475b04c` — "fix(kms): run the read-source
regression on a real drm node with real payloads"** (F4-B2, 2026-09-12) as the
first bad commit for **both**. Its parent `e8df2de2` passes both.

That commit made `KmsBackend::for_tests_with_vk_live_scene()` substitute a real
DRM primary node for the `Device::for_tests()` socket stand-in, so the fixture's
scanout-pool allocation — which used to fail with `ENOTTY` — now succeeds, and
the scene takes the real flip path against the fixture's **synthetic** output
(zeroed mode, invented CRTC ids), which the kernel rejects. Upstream wrote both
tests against the pool-less fixture.

Decisive experiment: with that substitution block removed — which makes this
branch's fixture byte-for-byte upstream's — **both tests pass**. The two tests
are exactly the two `render_acceptance` callers of `for_tests_with_vk_live_scene`.

## The fix (to be written by codex)

Do not change the semantics of a fixture upstream's tests rely on:

1. `for_tests_with_vk_live_scene()` goes back to upstream's body, with no real
   node substitution.
2. This branch's variant becomes a new `for_tests_with_vk_live_scene_real_drm()`,
   used by the callers that need real `PRIME_FD_TO_HANDLE`/`ADDFB2`/`RMFB`: the
   part-3 fixture `for_tests_with_live_kms`,
   `c0_2ci_read_source_scratch_regression_vulkan` and
   `c0_2ci_scene_managed_shared_compose_vulkan`.
3. Upstream's three library callers keep the upstream fixture:
   `root_get_image_reads_scanout_pixels_not_root_storage`,
   `root_overlay_xor_pass_reaches_scanout`,
   `root_copy_area_include_inferiors_captures_window_into_pixmap`. They are
   upstream hardware tests this branch never ran either, and they read scanout
   pixels, so they are likely affected the same way today.
4. The auto-granted-master drop in the two test openers.

## Process change

This branch's hardware gate must run upstream's ignored hardware tests too —
`render_acceptance` and the library's non-`c0_2ci` `_vulkan`/`_drm` tests — not
only `c0_2ci`. A regression against an upstream test is exactly what a
`c0_2ci`-only gate cannot see.
