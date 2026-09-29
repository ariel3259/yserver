# Phase C.0 addendum: upstream RANDR features on the Owner route

**Status:** scope and route audit only. This addendum does not enable CRTC
transforms or virtual KMS outputs on Owner. It records the behavior that the
merge exposes today and the work a later C.0 change would need.

**Related specifications:**

- `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
- `docs/superpowers/specs/2026-09-29-randr-crtc-transform-design.md`
- `docs/superpowers/plans/2026-09-29-randr-crtc-transform-plan.md`

## Route audit

| Upstream behavior | Owner exposure in this merge | Boundary |
| --- | --- | --- |
| #184 reusable host-cached screen readback | Partial | The reusable staging allocation and one-shot copy are shared. A managed Shared scanout read holds a `ResourceService` read lease through the copy fence. The direct-source lookup still resolves Legacy `scanout_m2` frames; it does not select the current direct frame from Owner's admission ledger. Root readback during Owner direct scanout is therefore not covered. |
| #186 `RRSetCrtcTransform` protocol state | Protocol state only | Core accepts the transform, retains current/pending transforms, and publishes the current transform after a successful CRTC configuration. The request is serialized by `RandrMutationGate`. `KmsBackend::randr_layout_changed` omits Owner outputs from the platform transform map, so Owner rendering, scanout dimensions, pointer mapping, cursor policy, and readback do not apply the transform. |
| #186 transformed scene composition and cursor handling | None on Owner | The intermediate, scale pass, cursor save/restore, transformed root readback, and pointer confinement are used only when the Legacy output has a platform transform. Owner output layout and commit state remain mode-sized. |
| #187 `xrandr --dpi` for newly started clients | Shared behavior | Screen physical-size state and the setup-time DPI propagation live in shared core state and message handling; they do not depend on the KMS rendering route. |
| #188 virtual monitor requests and queries | Shared behavior | Set/list/delete monitor state is handled by shared RANDR core state. Virtual monitor metadata does not create or configure KMS outputs, so no Owner output ledger is involved. Set and delete are gate-classified mutations. |

## Owner work required for transforms

1. Add a transform to the Owner output configuration identity and generation.
   A configuration replacement must prevent a commit prepared with the old
   transform, mode extent, or footprint from becoming current afterward.
2. Give Owner composition a mode-sized scanout target and a transform-sized
   intermediate. Record scene composition and scaling in the same tracked
   GPU work, with the destination write reserved through `ResourceService`.
   Retain the scanout allocation and intermediate until the Owner render and
   KMS milestones that use them have completed.
3. Keep root, intermediate, and BO damage in explicit coordinate spaces.
   The scale pass writes the full BO; incomplete scene recording must
   invalidate the BO's damage state. Carry these facts through
   `OwnerDamageTransaction` and only acknowledge drawable damage at the
   existing Owner milestones.
4. Include cursor save/restore storage in the same lifetime proof. Transformed
   outputs need software cursor composition, transformed cursor coordinates,
   pointer confinement, and commit-scoped cursor transitions. A failed or
   withdrawn commit must not advance cursor state.
5. Route root `GetImage` and `CopyArea(IncludeInferiors)` reads through the
   buffer that is actually current. For composed Owner output, select and
   reserve the ledger's `Current` allocation with a read lease held through
   the host-cached staging fence. For direct Owner output, resolve the current
   source and offsets from the admission ledger rather than `scanout_m2`.
6. Connect CRTC transform publication to Owner configuration, root extent and
   pointer extent updates, scene wakeup, and RANDR notifications without
   bypassing the mutation gate or C.0's topology episode and withdrawal rules.

## Mutation classification

`RRSetCrtcTransform` (minor 26) remains `Mutation`: its pending transform
changes the configuration installed by a later `SetCrtcConfig`. `RRSetMonitor`
(minor 43) and `RRDeleteMonitor` (minor 44) are `Mutation`: both change shared
RANDR monitor configuration and must not interleave with an in-flight
configuration publication. `RRSetScreenSize`, used by the DPI path, was
already a mutation. These classifications serialize protocol state; they do
not claim that Owner renders transforms or consumes virtual monitor geometry.

## Required Owner probes

- Scale/crop/cursor/pointer: run the scale scenarios from
  `tools/vng-scenarios/` once on Legacy and once on Owner; Owner support is
  incomplete until output pixels, pointer coordinates, cursor restoration,
  and transformed root reads all match the specified transform.
- Readback: while Owner direct scanout is current, request root `GetImage` and
  compare it with the displayed source; then repeat on a composed Owner frame
  and verify the selected allocation stays leased through copy completion.
- DPI: `tools/vng-scenarios/xrandr-dpi.sh`, including a client launched after
  changing DPI, on both routes.
- Virtual monitors: `tools/vng-scenarios/xrandr-monitors.sh` set/list/delete
  sequence on both routes; compare the wire replies and notifications.
