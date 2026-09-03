# Phase C.0 driver-specific coordinate eligibility review

**Date:** 2026-09-02
**Reviewer:** Opus, with kernel call-chain verification by Codex
**Reviewed spec:** Draft
`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
**Kernel basis:** local Linux tree at
`77cb8f24c2381a8abb7272d7bbdec548d6426a8a`
**NVIDIA stock-source basis:** published open modules `610.57.04` at
`e4a5faa2567f28c8eabe0ebb6422b6d0abcf37eb`
**Disposition:** Incorporated after user approval, with one mechanism correction

## AMD composition-dependent async eligibility

The review correctly identified that
`amdgpu_dm_plane_atomic_async_check()` rejects a cursor async update when the
current AMD CRTC state uses `DM_CURSOR_OVERLAY_MODE`. On pre-DCN 4.0.1/4.2.0
hardware, including the required Navi 21 cohort,
`dm_crtc_get_cursor_mode()` selects overlay mode for relevant below-cursor YUV
formats, active plane color pipelines, incompatible scaling, or incomplete CRTC
coverage.

The proposed userspace errno mechanism was not correct. In the pinned core,
`drm_atomic_helper_check()` evaluates the driver's result as:

```c
if (state->legacy_cursor_update)
    state->async_update = !drm_atomic_helper_async_check(dev, state);

return ret;
```

The internal `-EINVAL` therefore selects `async_update=false`; it is not returned
as the cursor ioctl's errno. The legacy request continues through the ordinary
commit path. Userspace can observe the returned host-call duration and its own
installed plane composition, but cannot truthfully report that internal result
as a visible `EINVAL`.

The spec now defines a stable `CursorCompositionKey` from the owner-controlled
below-cursor plane shape. It excludes framebuffer identity/content/damage, so
ordinary primary replacement cannot manufacture requalification attempts. The
immutable cohort evidence provides a checked composition predicate. A known
ineligible key emits no fast coordinate ioctl. An otherwise valid call that
returns above the latency bound suspends only its exact key; a different,
canonically installed predicate-eligible key receives one requalification
attempt. A real userspace-visible `EINVAL` remains a request/driver
contradiction and cannot enter this recovery path.

Current merged yserver direct eligibility uses primary planes, one XRGB8888
plane, and exact root/source dimensions. Consequently a client YUV or scaled
buffer does not automatically create every AMD KMS shape discussed above. The
RX 6800 XT matrix uses a documented nonmergeable raw-KMS harness for shapes not
reachable through production eligibility, labels them driver-mechanism evidence,
and separately verifies the production fallback and recovery state machine.

## NVIDIA stock-driver boundary

The stock NVIDIA source audited for C.0 has only `.atomic_check` in
`nv_plane_helper_funcs`; it has no cursor `.atomic_async_check` or
`.atomic_async_update`. The core consumes that missing-hook rejection in the
same way described above and takes its ordinary legacy fallback. It does not
create a qualified immediate coordinate hook.

C.0 considers only stock, publicly released NVIDIA modules. Patched, proposed,
out-of-tree and unreleased driver builds are excluded from the support table,
measurement arms and acceptance evidence. Therefore stock NVIDIA cannot select
`OwnerMediatedLegacyMove`: composed presentation uses the shipping software
cursor, while direct scanout may use `SynchronousAtomicMove` only if its own
stock-driver gates pass. The legacy-HW arm remains historical blocking scale,
not fast-hook evidence. Any future stock mechanism requires a separate spec
revision and complete new cohort matrix.
