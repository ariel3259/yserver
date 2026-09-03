# Phase C.0 — adversarial review: driver-side plane expansion, latch timing, and gate blast radius

**Date:** 2026-09-02
**Reviewer:** Opus, with kernel and yserver-path verification by Codex
**Reviewed spec:** Draft
`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
**Kernel basis:** local Linux tree at
`77cb8f24c2381a8abb7272d7bbdec548d6426a8a`
**Disposition:** Incorporated after user approval, with the runtime latch
replaced by a smaller checked construction contract

This finding supersedes the history-bearing `CursorCompositionKey` /
`CoordinateCompositionPredicate` disposition in the preceding composition-
predicate review.

## Verified findings

### A-1. AMDGPU may add the cursor after userspace construction

`amdgpu_dm_atomic_check()` calls `dm_crtc_get_cursor_mode()` for each new CRTC.
When a relevant plane enable/disable, framebuffer-format, scale-ratio, z-order
or plane color-pipeline change requires reconsideration and the cursor is
enabled, that helper obtains the cursor with `drm_atomic_get_plane_state()`.
This adds the cursor plane to the atomic state. The later
`drm_atomic_helper_setup_commit()` attaches the CRTC commit to every plane then
present, including that driver-added cursor.

An earlier AMDGPU path independently calls
`drm_atomic_add_affected_planes()` when the request needs a modeset, changes
CRTC color management, changes VRR state, or carries `dsc_force_changed`.
DRM marks `color_mgmt_changed` when `GAMMA_LUT`, `CTM`, or `DEGAMMA_LUT` is
replaced. Consequently a C.0 gamma-only commit is also a cursor-expansion
hazard even though it does not depend on `dm_crtc_get_cursor_mode()`.

The userspace request's plane set therefore cannot be reported as the driver's
actual post-check closure.

### A-2. Completion-time latch advancement was too late

AMDGPU writes cursor mode into the candidate CRTC state during atomic check;
the atomic helper swaps that state before a successful nonblocking ioctl
returns. Waiting until canonical hardware completion to reject coordinate
overlap leaves a window in which the driver has adopted the candidate software
state while the owner still reasons from the old state.

### A-3. Predicate defects and architecture failures need different evidence scope

An explained coordinate-policy defect can be repaired without invalidating
unreachable NVIDIA or returnability paths, but metadata alone is not sufficient
attribution. Otherwise any unexpected stall could receive a plausible post-hoc
explanation and bypass the architecture gate.

### A-4. C.0 cannot install the motivating coverage-only shape

The merged direct path accepts only XRGB8888/B8G8R8A8, root-sized sources,
exact output tiling, and mode-sized 1:1 plane assignments. Composed scanout also
uses a full-mode XRGB8888 primary. C.1 changes async submission semantics but
does not widen direct eligibility to scaled, HDR/10-bit, YUV/video, or partial-
coverage planes. The coverage-only transition used to justify a history-
bearing latch is therefore outside C.0 production reachability.

## Approved disposition

1. Delete `CursorCompositionKey`, `CoordinateCompositionPredicate`, and the
   history-bearing composition latch.
2. Introduce `NativeCursorCompositionContract` as a checked construction
   precondition while `OwnerMediatedLegacyMove` is selectable: one full-mode
   XRGB8888 primary per active CRTC, `(0,0)` destination, 1:1 scale, fixed
   z-order below the cursor, and no active plane color pipeline. Direct
   candidates outside it are rejected before KMS; another future path must
   transition away from the fast transport before installing such a shape.
3. Introduce `AuditedCursorExpansionHazard` over the complete final serialized
   request. It is a conservative cohort-specific prediction, not measured
   driver closure. On the pinned AMD cohort its reasons include modeset, CRTC
   gamma/CTM/degamma, VRR, DSC-force, plane enable/disable or binding,
   framebuffer format, scale ratio, z-order, and plane color pipeline.
4. Permit coordinate overlap only for a contract-preserving primary whose
   userspace request omits the cursor and whose audited hazard is false. The
   prohibition exists before host-call dispatch and remains through canonical
   completion.
5. Define consecutive coordinate `EBUSY` as attempts without an intervening
   successful coordinate return. Success resets the count; atomic completion
   alone does not. No hazard-completion reset is added.
6. An over-bound coordinate return always closes the fast transport. It may
   preserve the recorded global architecture selection only after a unit test
   or documented raw-KMS harness deterministically reproduces the precise
   omitted/misclassified rule and slow-path consequence. The repaired tip must
   rerun the affected AMD coordinate strata, quotas, policy/performance rows,
   and complete AMD soak. Other evidence requires an explicit section 18 reuse
   proof. Missing attribution or cross-cutting impact invalidates the global
   selection.
7. Keep XRGB8888 explicit in the native contract. The merged composed path's
   `VkScanoutFb::format()` returns `DrmFourcc::Xrgb8888`, and direct eligibility
   requires `DRM_FORMAT_XRGB8888`; tests bind both load-bearing sources to the
   contract so they cannot drift independently.
8. If a future path constructs an out-of-contract primary, invalidate
   `OwnerMediatedLegacyMove` during construction and before its ioctl, never at
   canonical completion. The same plane incarnation cannot reselect the fast
   transport afterward. Re-entry requires cursor detach/reattach and complete
   plane-incarnation requalification with the contract-valid primary already
   represented in the atomic state; cursor enabling and the CRTC `plane_mask`
   change force AMD cursor-mode and z-order recomputation against that state.
