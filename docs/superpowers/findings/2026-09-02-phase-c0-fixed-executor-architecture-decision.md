# Phase C.0 — fixed executor architecture decision

**Date:** 2026-09-02
**Subject:** section 4.1 of
`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
**Disposition:** Incorporated after user approval; pending adversarial review

## Decision

C.0 requires one process-isolated `KmsIoExecutor` per DRM device incarnation
for every KMS host-call class. It has no in-process, worker-thread,
driver-specific, cohort-specific or runtime-selectable host-call branch.

This is a conservative design decision, not an empirical claim that every
atomic ioctl can block. Proprietary NVKMS is opaque and its reachable waits
cannot be bounded through source audit. The historical NVIDIA `MOVECURSOR`
measurement also demonstrates the consequence of permitting a driver wait to
occupy yserver's single-threaded X11 core. C.0 accepts the executor's IPC and
lifecycle cost to make core containment structural.

A later phase may recover in-process execution for an explicitly named call
class only through a new spec and cohort-specific returnability evidence. C.0
does not anticipate that successor or expose an ad-hoc switch.

## Normative consequences

- The owner installs `Submitting` or `CoordinateSubmitting` and every
  applicable fd lease before IPC dispatch.
- Explicit rejection, success and acceptance-unknown remain distinct after
  dispatch. Helper exit, IPC loss and watchdog expiry cannot become rejection.
- Watchdog, asynchronous reap, `ExecutorStalled`, quarantine and prompt logical
  VT/device-loss progress are unconditional.
- The provisional in-process latency and returnability artifacts, the four
  architecture-selection outcomes and all `InProcessQualified` branches are
  removed.
- Final-tip helper duration, IPC latency, input-to-dispatch overhead,
  concurrency, watchdog/reap injection and hardware soaks validate the chosen
  executor but never reopen the architecture decision.
- The local Raphael `1002:164e` iGPU may support development iteration but does
  not replace the required RX 6800 XT/Navi 21 release cohort in `CAP-4` or
  section 16.3.

## Slow coordinate returns

The removed in-process maximum no longer supplies an architecture-selection
budget. `CoordinateFastReturnMax = 1 ms` remains a transport-health invariant:
a qualified coordinate call above it is isolated from the X11 core but proves
that `OwnerMediatedLegacyMove` entered an ordinary slow path. The call closes
that transport and fails the affected AMD coordinate-policy, performance and
soak rows. Repairs use the ordinary section 18 reachability manifest and rerun
all affected evidence.

`CoordinatePolicyDefectCandidate` and its conditional preservation of a global
architecture selection are deleted because a fixed architecture leaves them no
consumer.
