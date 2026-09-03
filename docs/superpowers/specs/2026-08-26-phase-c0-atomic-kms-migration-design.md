# Phase C.0 — complete KMS ownership and atomic state migration

**Date:** 2026-09-03 (revision 2: measured cursor policy replaces the cohort allowlist)
**Status:** Approved — concurrency, evidence, delivery, driver-eligibility,
composition-predicate, driver-expansion, fixed-executor and shutdown-barrier
dispositions incorporated. Revision 2 replaces the `CAP-4` cohort allowlist with
a runtime-derived cursor policy and measured demotion, and no longer gates merge
on any particular device remaining available
**Branch:** `feat/phase-c0-atomic-kms-migration`
**Baseline:** `master` at `02bafec3`, including the final squash of PR #129 at
`fc76b743`, the subsequent VT cursor-plane fix at `c09358a1`, and the
damage-clipped repaint work for non-composited desktops
**Successor:** Phase C.1, on-demand async page-flip tearing
**Cursor-latency successor:** Phase C.2, above-vblank hardware-cursor motion

## 1. Summary

Phase C.0 completes ownership of yserver's live KMS state under one
device-local scheduler. Persistent cursor state uses universal cursor-plane
properties and CRTC gamma uses atomic `GAMMA_LUT` blobs. A narrowly qualified,
owner-mediated legacy `MOVECURSOR` transport may remain only for coordinates of
an already installed cursor where Linux exposes the driver's immediate cursor
hook through no atomic userspace UAPI. Primary-plane, cursor attach/image/
visibility, color, modeset, unflip, DPMS, and topology state remain atomic.

The merged Phase A+B implementation is normative input, not code to reshape
back toward an older draft. In particular, C.0 inherits Xorg-compatible
target-MSC supersession in core and the bounded primary-plane successor slot in
the KMS backend. C.0 converts that exact architecture to the device-local
atomic owner.

This phase does not implement tearing, advertise
`PresentCapabilityAsyncMayTear`, or submit `PAGE_FLIP_ASYNC`. Its purpose is to
provide a single ordering model before Phase C.1 raises primary-plane commit
cadence above vblank. Phase C.2 still owns a durable above-vblank atomic/UAPI
mechanism and removal of any temporary coordinate exception. C.0 preserves each
device's already-shipping coordinate latency rather than deliberately regressing
it during the interval before C.2.

## 2. Problem

yserver currently controls primary planes, direct scanout, composed unflips,
modesets, and DPMS with atomic commits, but still mutates KMS state through two
legacy families:

- hardware cursor through `SETCURSOR`/`SETCURSOR2` and `MOVECURSOR`;
- RANDR gamma through the legacy CRTC `set_gamma` ioctl.

Linux DRM deprecates the cursor entry points in favor of universal cursor
planes. Atomic-capable CRTCs expose color management through the `GAMMA_LUT`
blob property. The defect is uncontrolled mutation outside one owner; the
userspace ioctl number alone is not the invariant when the kernel exposes a
cursor fast path only through its legacy-to-atomic compatibility entry point.

The existing combination already has a measured failure history:

- the 2026-05 `bundle-cursor-atomic` experiment bundled cursor properties only
  with scene page flips, so cursor motion on an idle scene reached only about
  5–9 Hz;
- flushing cursor-only atomic state every loop iteration generated roughly 200
  `EBUSY` failures per second because the kernel had an earlier nonblocking
  commit pending;
- atomic cursor implementation was incomplete because the required plane state
  was not available at the original call sites; the resulting production
  workaround moved cursor load/move back to legacy ioctls, restoring
  responsiveness but leaving two state-management models active on the same
  CRTC.

NVIDIA has a distinct measured failure. On the GTX 1050 proprietary stack,
legacy `MOVECURSOR` averaged about 11.5 ms and reached 16.3 ms while returning
no `EBUSY`; the driver paced the call to vblank and blocked yserver's
single-threaded core. The shipping policy therefore selects the empirically
smooth software cursor on NVIDIA. An ordinary atomic cursor commit is also
vblank-paced, so C.0 does not claim to make that hardware update immediate.
C.0 chooses a process-isolated `KmsIoExecutor` for every KMS host-call class as
section 4.1 defines. The historical stall is not proof that every atomic ioctl
will block, but it demonstrates the consequence of allowing a driver wait to
occupy the X11 core. Lifting the NVIDIA software-cursor policy remains
conditional on the separate hardware gate in section 16.3.

Phase C.1 will submit immediate primary-plane flips at high cadence. It depends
on cursor-plane and completion capability, not programmable gamma. Unsupported
gamma is an explicit terminal RANDR state and does not invalidate an otherwise
qualified primary/cursor pipeline.

## 3. Goals

1. Express cursor image, framebuffer, CRTC binding, position, size, hotspot,
   visibility, animation, and detach through universal cursor-plane properties.
2. Express RANDR CRTC gamma through atomic `GAMMA_LUT` property blobs.
3. Eliminate every legacy cursor load/show/hide/disable and gamma ioctl. Permit
   only a qualified coordinate-only `MOVECURSOR` transport under the same owner
   until Phase C.2 supplies and qualifies its replacement.
4. Preserve cursor correctness and each device's shipping coordinate-latency
   baseline on idle and animating desktops, during composition, and during
   fullscreen direct scanout. Where a device cannot hold that baseline, prove it
   by measurement and fall back, rather than by withholding the capability from
   unlisted hardware.
5. Maintain bounded latest-wins motion under high-rate input without observed
   X11-core stalls or an `EBUSY` storm on any device. Process isolation
   makes core containment structural even when a driver host call returns late
   or not at all.
6. Introduce one commit owner per DRM device that serializes every atomic
   state-changing operation affecting that device.
7. Coalesce pending cursor motion/image changes latest-wins while preserving
   cursor framebuffer and completion lifetimes.
8. Preserve the merged Phase A+B protocol and direct-scanout contracts:
   target-scoped core supersession, one in-flight direct flip, one bounded
   primary-plane successor, immediate release of a displaced never-submitted
   buffer, ordered deferred `Skip`, cursor visibility, RANDR gamma, unflip, VT,
   DPMS, hotplug, and multi-output state. C.0 deliberately does not promise
   Phase B's aggregate multi-output commit throughput; section 9 states the
   accepted device-slot ceiling and section 16 measures it.
9. Provide separate cursor/primary structural capability, atomic-gamma
   capability, incarnation qualification, and per-submit readiness. Phase C.1
   consumes cursor/primary completion capability and never depends on gamma.
10. Give every submitted commit an unambiguous completion identity and terminal
    state, including across VT, hotplug, shutdown, and device loss.
11. Introduce no runtime environment variable, command-line flag, config key,
    or rollout lever for C.0 behavior. Production policy is capability- and
    state-derived; fault injection is test-only.

## 4. Non-goals

- `PAGE_FLIP_ASYNC` or visible tearing.
- A new above-vblank atomic cursor UAPI. Phase C.2 owns that mechanism and the
  removal decision for C.0's explicitly scoped coordinate exception.
- Core Present scheduling, target equivalence, async-option parsing, or Present
  capability changes. C.0 does convert the already-shipped direct primary
  submission and successor promotion below that layer.
- Deadline-scheduled primary-plane submission. It is an independent design on
  the merged Phase A+B base, not a Phase C dependency or successor; C.0 neither
  provides a timer seam for it nor changes the immediate retirement-time
  dispatch policy.
- Variable refresh rate.
- Software-cursor composition redesign.
- Providing hardware cursor or programmable gamma on devices that expose no
  corresponding atomic property. Missing cursor coverage disables the
  cursor/primary capability consumed by C.1; missing gamma reports gamma
  unavailable without disabling C.1.
- Removing `DRM_IOCTL_CRTC_QUEUE_SEQUENCE`; it arms an observation event and
  does not mutate KMS display state.

### 4.1. Fixed process-isolated executor architecture

C.0 requires `KmsIoExecutor`: one process-isolated host-call executor per DRM
device incarnation. This is a design decision, not the result of a finite
returnability claim. Proprietary NVKMS is opaque, its reachable waits cannot be
bounded by source audit, and the historical NVIDIA `MOVECURSOR` measurement
demonstrates that a driver wait can block yserver's single-threaded X11 core for
an output period. C.0 therefore pays the IPC and lifecycle cost once and makes
core containment structural for every supported cohort. That cost is measured
rather than assumed: dispatch-to-reply is single-digit microseconds at p99
under load, a live coordinate reservation occupies under 0.5% of the channel at
1000 Hz, and the dominant term in the coordinate path is the ioctl held by the
call in flight. That dominant term is identical for an in-process owner on the
same single thread, which would carry it without crash containment, watchdog or
bounded reap. The arms, their load conditions and their limits are recorded in
`docs/superpowers/findings/2026-09-02-phase-c0-executor-ipc-cost-measurement.md`.

The executor owns every C.0 KMS host call: cursor-only atomic `NONBLOCK`,
primary-only and changed-primary/changed-cursor commits, qualified coordinate-
only `MOVECURSOR`, gamma-only, unflip, modeset/install/recovery, final
`TEST_ONLY`, and `GET_SEQUENCE`/`QUEUE_SEQUENCE`. There is no per-driver,
per-cohort, per-call-class or runtime in-process branch. A future phase may
recover in-process execution for a named call class only after a new spec and
cohort-specific returnability evidence establish its safety; C.0 neither
anticipates nor exposes such a switch.

The owner installs `Submitting` or `CoordinateSubmitting` and the applicable fd
lease before IPC dispatch. From that boundary, explicit rejection, success and
acceptance-unknown remain distinct; IPC loss, helper exit or watchdog expiry
cannot be rewritten as rejection. The executor watchdog, asynchronous reap,
`ExecutorStalled`, quarantine, and prompt logical VT/device-loss progress are
unconditional requirements. Sections 6.3, 6.4 and 10 define those transitions.
No worker-thread alternative exists: it would retain the same possible kernel
acceptance, holder memory, fd alias and unbounded join problem without process
crash containment.

Release evidence measures the chosen architecture; it does not reopen the
choice. On the final integrated tip, record helper-measured ioctl duration,
executor IPC dispatch-to-reply latency, total input-to-dispatch overhead,
context switches, message/fd counts, watchdog/reap outcomes and the exact GPU,
kernel, module and available source identity. The latency/concurrency recorder
uses a checked fixed-size, single-writer buffer preallocated for the complete
declared arm. It performs no filesystem write, allocation, buffer flush or
additional supervisor IPC on the measured path, never wraps or overwrites, and
exports only after the arm ends. Exhaustion makes the affected evidence row
`EvidenceInsufficient`.

The concurrency premise is fixed against a verified kernel range rather than a
single revision. It holds for Linux 7.1.9 through 7.2.2 in the five functions
that carry it: `drm_atomic_helper_async_check()`,
`drm_atomic_helper_setup_commit()`, `drm_atomic_add_affected_planes()`,
`amdgpu_dm_atomic_check()`'s modeset/color-management/VRR/`dsc_force_changed`
guard, and `dm_crtc_get_cursor_mode()`. Across that range those functions
differ only by the 7.2 rename of `struct drm_atomic_state` to
`struct drm_atomic_commit` and by two `dm_crtc_get_cursor_mode()` changes that
section 7.1 records; no C.0 conclusion depends on either. Commit
`77cb8f24c2381a8abb7272d7bbdec548d6426a8a` is recorded as the provenance of the
original reading, not as a mainline reference point. A cohort kernel outside
the verified range requires the same five-function comparison before its
evidence is admissible.
`drm_atomic_helper_async_check()` checks the outstanding
`old_plane_state->commit`; it contains no outstanding-CRTC-commit check.
`drm_atomic_helper_setup_commit()` installs a plane commit only for planes
present in the atomic state. Therefore a primary request that omits an unchanged
cursor does not by the core helper alone block the legacy cursor async path,
whereas restating that cursor attaches the primary commit to its plane. The
request is not thereby proven coordinate-overlap-safe: a driver may add the
cursor during atomic check. Section 7.1 names AMDGPU's audited expansion paths
and requires the complete serialized request to have
`AuditedCursorExpansionHazard=false` before coordinate overlap. This field is a
conservative source-derived classification, not observation of the driver's
internal atomic closure.

Coordinate concurrency has a phase-aware release quota on each required device
whose stock cohort nominates `OwnerMediatedLegacyMove`; in the current release
matrix this is the Raphael iGPU, never stock NVIDIA. Under both continuous composed and continuous direct primary
traffic, collect 100,000 production-shape initial coordinate attempts made
after the matching primary's accepted dispatch and before its
`HardwareComplete` milestone. Only a primary whose old/candidate shapes pass
the native contract, whose userspace request omits the cursor and whose audited
expansion hazard is false can supply this interval. Gamma, modeset or any other
hazard-classified record creates no coordinate attempt and cannot fill a quota.

Post-processing divides the exact dispatch-to-`HardwareComplete` interval into
ten equal normalized phase deciles; every decile must contain at least 5,000
qualified initial attempts. Attempts outside the interval and deferred retries
after completion are reported but do not count. Each composed/direct stratum
stops successfully at 100,000 qualified initial attempts and 5,000 in every
actual decile. It may consume at most `PhaseCycleCap = 250,000` accepted-primary
cycles and `PhaseAttemptCap = 250,000` initial attempts. An underfilled quota,
cap exhaustion or recorder overflow is `EvidenceInsufficient`; extending a cap
requires a reviewed spec revision.

Every coordinate record contains the coordinate generation, primary
`CommitId`, primary dispatch, attempt and eventual `HardwareComplete` instants,
request shape, initial-versus-retry kind, result/errno, helper-measured duration,
construction-time `NativeCursorCompositionContract`, complete-request
`AuditedCursorExpansionHazard` and reasons, exact userspace object/plane set,
and owner state proving the accepted primary was overlap-safe. No field claims
the driver's unobservable post-check plane set. Phase is computed only after
canonical completion as `(attempt - dispatch) / (HardwareComplete - dispatch)`
with checked arithmetic. Zero/negative intervals or attempts outside
`[dispatch, HardwareComplete)` do not qualify. A retry points to its initial
`EBUSY`, records the next primary-admission instant, and proves it ran after the
matching completion but before later primary admission. Initial and retry
samples never combine to satisfy the quota.

The cursor-unchanged-absorbed shape remains optional bounded characterization
through a documented nonmergeable diff. It has no quota, phase scheduler,
transport-closure suppression or merge effect. Production cursor omission is
established by source audit, unit tests and the required production-shape
hardware arms.

## 5. Global invariant

For every device/domain advertised as C.0-complete:

```text
all persistent live KMS state mutation uses atomic commits
&& every mutation is ordered by the owning device-local commit owner
&& no legacy modeset/page-flip/gamma or cursor load/show/hide/disable is reachable
&& a legacy cursor ioctl, when present, is coordinate-only MOVECURSOR for an
   already installed framebuffer on an evidence-qualified plane
```

The coordinate exception is a transport, not a second owner. It is admissible
only with no atomic `Submitting`, lifecycle barrier, or validation lease. It
may coexist with one accepted primary commit only when that request's recorded
userspace object set omitted the cursor plane, its old and candidate primary
shapes both passed `NativeCursorCompositionContract`,
`AuditedCursorExpansionHazard=false`, and it changed no connector, topology,
unflip, cursor, or lifecycle state. It carries the current cursor/
topology generations, coalesces latest-wins, and may change no
framebuffer, hotspot, visibility, CRTC binding, size, or source rectangle. Its
qualified fast return—or actual helper reap after an uncertain isolated
result—is the ordering boundary before any later cursor-affecting or newly
dispatched atomic mutation; the already accepted overlap-safe primary cannot
overwrite it. A contract failure or audited expansion hazard prevents overlap
under section 7.1 and leaves the newest coordinates as ordinary bounded atomic/
software desired state. A returned call above the latency bound, visible ioctl
rejection, uncertain return, or loss of any other transport qualification closes
the fast transport for the complete plane incarnation. It never retries
immediately or beyond section 7.1's single deferred `EBUSY` attempt, and never
falls back to another uncontrolled ioctl.
C.2 owns replacing or removing this temporary transport.

`DRM_IOCTL_CRTC_QUEUE_SEQUENCE`, event reads, capability queries, framebuffer
allocation/import, and read-only property discovery are not state mutations and
remain outside the commit owner.

The modifier-less `ADDFB2` fallback is also outside this prohibition: it creates
a framebuffer object but does not change live CRTC/plane state.

Atomic `TEST_ONLY` requests do not mutate live state, but a result intended to
authorize a later live transition is ordered by the owner. The owner assigns an
`AtomicSnapshotId` containing device generation, lifecycle epoch, topology
generation, and the current/desired generations of every object in the request. Immediately before
installation it stops admission and merges all mandatory current cursor, gamma,
primary, connector, and CRTC persistent state.

The final `TEST_ONLY` and live request must contain identical DRM objects,
framebuffer/blob ids, routing, modes, and plane/color geometry. Ephemeral
synchronization properties are deliberately excluded from this equality:
`IN_FENCE_FD`, `OUT_FENCE_PTR`, event user data, and their userspace storage are
freshly built for the live ioctl under section 10.2. Under C.0's pre-submit wait
rule, the live request omits `IN_FENCE_FD` or supplies only its `-1` no-wait
value; no unresolved producer fd crosses the ioctl. The final test uses the same
state-affecting flags, including `ALLOW_MODESET` when applicable, but omits
live-only `NONBLOCK` and `PAGE_FLIP_EVENT`. C.0 never uses
`PAGE_FLIP_ASYNC`; a C.1 candidate-specific test retains `PAGE_FLIP_ASYNC`
because async capability/request shape is exactly what it validates. No
persistent generation may change between test and live submit. A helper or
speculative `TEST_ONLY` may run outside serialization, but it is only candidate
qualification and never authorizes a live commit by topology generation alone.

`TEST_ONLY` is a `ValidationOnly` host-call class. The kernel executes
`drm_atomic_check_only`; a supplied `NONBLOCK` bit does not turn it into a live
nonblocking commit and is omitted. It touches no hardware, transfers no live
resource ownership, creates no out-fence, and does not occupy the submitted-
commit slot. A final serialized validation does hold an exclusive owner
validation lease so no persistent generation can change before the live call.
Seat-active validation has a two-second host-call watchdog and cold-start/
offline validation has the 30-second watchdog. Timeout invalidates the candidate
snapshot but never classifies hardware state as acceptance-unknown because no
live mutation was requested.

This final-`TEST_ONLY` rule applies to topology/install/restore/qualification
transitions, not to every ordinary C.0 cursor, gamma, or primary replacement.
C.1 explicitly extends it to each candidate async request so a content failure
can be separated from a capability/topology rejection before any latch. Section
6 is authoritative for terminology, evidence, and lifecycle; section 5 remains
authoritative for the global mutation invariant and this scoped construction
ordering. Neither section silently broadens the other's `TEST_ONLY` scope.

The conversion inventory includes every live mutation in the merged base, not
only the former cursor and gamma helpers. In particular C.0 must route through
the owner:

- direct primary replacement currently issued by
  `drm::modeset::submit_direct_scanout`;
- retirement-time promotion of `ScanoutM2State::queued_successor`;
- the best-effort legacy `cursor_plane_hide_all` added before the authoritative
  VT all-off transaction by `c09358a1`;
- cursor load, show, hide and detach, including direct-entry cursor bind, plus
  coordinate-only movement through either the atomic request or the qualified
  owner-mediated exception above;
- legacy RANDR `set_gamma`, the legacy `get_crtc().gamma_length()` discovery
  path, every `u16::MAX` gamma-size clamp/fallback, and every resume/reapply
  consumer of that state; and
- every existing atomic modeset, primary, unflip, DPMS and topology call site,
  including the global `kms_outputs_active` summary, the all-output
  `dpms_set_outputs_active(bool)` loop, and each caller that currently drives
  it from protocol DPMS, VT, reprobe, or topology recovery.

The conversion inventory also includes event and clock infrastructure whose
baseline shape cannot represent C.0 identities even though it does not itself
mutate KMS state:

- `drm::control::Device::receive_events()` and `Event::PageFlip`, whose
  compatibility parser folds `user_data` into `crtc` and discards the two raw
  fields' independence;
- the existing target-dependent `IoctlReq`/`libc::Ioctl` alias in
  `drm/page_flip.rs`; and
- every construction, read, and write of
  `crtc_queue_sequence_unsupported_devices`, whose baseline key is only
  `DrmDeviceKey` and whose result otherwise survives reopen and CRTC clock-
  epoch replacement.

Read-only clock probes and `DRM_IOCTL_CRTC_QUEUE_SEQUENCE` remain outside the
mutation invariant, but their events still use the identity model in section
10.

## 6. Normative core model

This section is the authority for terminology, capability layers, commit proof,
and device lifecycle. Later sections define payload-specific construction and
scheduling but must not redefine these concepts. If a summary, test description,
or acceptance bullet conflicts with this section, this section wins and the
conflicting text is a spec defect.

### 6.1. Terms and identities

| Term | Normative meaning |
| --- | --- |
| Device identity | Stable physical/logical DRM-device identity used to project protocol capability. It is not an fd or object handle. |
| Device incarnation | One DRM open file description plus its complete owner-tracked `IncarnationFdSet` of aliases and helper leases. |
| Device generation | Monotonic generation attached to one incarnation's objects, events, and `CommitId` namespace. |
| Event token | Non-zero `u64` allocated from one monotonic namespace across the complete device incarnation and carried verbatim in DRM event `user_data`. It is never reused within that incarnation and resolves to exactly one typed target: an atomic commit record or a sequence-arm record. |
| CRTC clock source | Epoch-local `KernelSequence`, selected only from a successful `DRM_IOCTL_CRTC_GET_SEQUENCE` result with a trusted `u64` reference. `EOPNOTSUPP` closes C.0 qualification; software protocol-clock synthesis is deferred beyond C.0. |
| Topology generation | One mapping of protocol outputs/CRTCs to discovered DRM objects within an incarnation. It cannot clear incarnation poison. |
| Native cursor composition contract | Checked construction precondition for every active C.0 primary while `OwnerMediatedLegacyMove` is selectable: exactly one below-cursor primary per active CRTC, XRGB8888, inactive plane color pipeline, fixed normalized z-order below the cursor, source and destination at 1:1 scale, and destination `(0,0)` with the complete mode extent. Framebuffer identity, contents, damage and a qualified modifier may change without changing the contract. An out-of-contract direct candidate is rejected before KMS. Any other path that constructs an out-of-contract primary invalidates `OwnerMediatedLegacyMove` before that primary's ioctl may begin and may install it only after an ordered transition has made the transport unselectable. The same plane incarnation cannot reselect the transport afterward; re-entry requires complete plane-incarnation requalification through a cursor detach/reattach with a contract-valid primary already represented in the atomic state. |
| Audited cursor expansion hazard | Immutable cohort-specific, conservative prediction over the complete final serialized atomic request. It is true when source audit says the driver may add or otherwise serialize the cursor plane despite userspace omission. It records named reasons and the userspace object set but never claims to observe the driver's internal post-check plane set. Only a contract-preserving primary request with this value false may overlap `OwnerMediatedLegacyMove`. |
| Lifecycle epoch | A monotonic per-device `LifecycleEpochId` that is always present, including during ordinary `Ready` traffic. It changes before lifecycle work can supersede or invalidate an in-flight operation. |
| Lifecycle transition | Optional bounded work identified by `LifecycleTransitionId`; ordinary cursor/gamma/primary commits have no transition id but always carry the current lifecycle epoch. |
| Protocol domain | Client-visible device/output identity set for which cacheable capability is advertised, after applying the merged connector-class policy, including the `non-desktop` filter. A connector excluded by that policy is not a hidden member of the domain. |
| Intent | Bounded desired work not yet accepted by KMS. It owns only never-submitted resources. |
| Submitting record | A live transaction durably installed in the owner before it crosses executor IPC. Kernel acceptance is unknown until an explicit result arrives, so it occupies the device slot and owns both possible state/resource sets. |
| Coordinate-submitting record | One owner-installed, per-cursor-plane mutation reservation for a qualified coordinate-only ioctl. It occupies no atomic device slot and may coexist only with an accepted contract-preserving primary whose userspace request omits the cursor and whose `AuditedCursorExpansionHazard` is false. While its host call is unresolved it excludes every new KMS host-call dispatch; an uncertain isolated result retains the plane reservation until actual helper reap. |
| Commit record | One owner-serialized live transaction with identity, expected evidence, resources, and exactly one terminal state. |
| Atomic CRTC closure | The exact set of CRTCs pulled into the userspace-constructed persistent atomic request by CRTC properties and by connector/plane properties through their old or new `CRTC_ID` binding. `ExpectedCompletionCrtcs` is its old-or-new powered subset. Ephemeral `OUT_FENCE_PTR` entries are added only after this set is fixed and may not enlarge it. This is not a claim about objects a driver's atomic check may add internally. |
| Teardown barrier | Evidence that a stated class of old hardware/userspace ownership is unreachable. Barriers are resource-class-specific, never universal by implication. |

**ID-1 — no handle inference.** DRM object ids, fd integers, CRTC handles, and
topology epochs are never used as proof of incarnation identity or resource
retirement.

**ID-2 — fd-family retirement.** Poison clears only after new aliases are
forbidden, submitters/readers detach, helpers are cancelled and reaped, and the
complete old `IncarnationFdSet` reaches zero leases. Only then may a newly opened
file description receive a new incarnation identity.

**ID-3 — epoch at the invalidation boundary.** A lifecycle event first closes
new admission and lets the sole `Submitting`/accepted record drain under its old
epoch. A clean completion remains authoritative. Only after that record
terminalizes—or immediately before a timeout/supersession deliberately abandons
its authority—does the owner increment `LifecycleEpochId` and publish the new
transition. Multiple events coalesced during the same drain cause one increment,
not one per event. Every executor request/reply and commit record carries the
epoch. A reply is current only when incarnation, lifecycle epoch, optional
transition id (when present), and commit id all match; a normal `Ready` commit
uses `transition_id=None`, never a fabricated or previous transition id.

### 6.2. Capability contract

| Layer | Key | Becomes true when | Becomes false when |
| --- | --- | --- | --- |
| `atomic_kms_pipeline_structurally_capable` | `(device_identity, protocol_domain)` | Discovery proves simultaneous cursor-plane coverage, at least one structurally available coordinate transport, required completion-property coverage, `DRM_CAP_CRTC_IN_VBLANK_EVENT=1`, monotonic DRM event timestamps, and usable `GET_SEQUENCE`. A multi-CRTC domain additionally forms one structurally homogeneous group. Gamma and release validation are not part of this bit. | Only recomputation for a changed client-visible protocol domain says so. Runtime failure never rewrites it. |
| `atomic_kms_cursor_policy` | `(device_identity, driver_identity)` | Runtime-derived. It is `AtomicHardware` when structural capability and incarnation qualification hold, no measured demotion is in force for this device identity, and no degradation prior matches the installed driver version. | A measured demotion under `CAP-4`, a matching degradation prior, or loss of structural capability or qualification makes it `SoftwareComposited`. Demotion is remembered for the server process lifetime and never reopens within it. |
| `atomic_gamma_capable` | `(device_identity, protocol_crtc)` | That protocol CRTC maps to a usable atomic `GAMMA_LUT` and representable `GAMMA_LUT_SIZE`. | Recomputed when that protocol CRTC's mapping or advertised gamma contract changes. It never gates C.1. |
| `atomic_kms_incarnation_qualified` | `(device_incarnation, topology_generation)` | Required cursor/primary/completion properties exist, no relevant latch/poison exists, and the mandatory real install/restore commit completes with canonical fence evidence. Gamma properties are independent. | Topology rejection, completion breach, incarnation retirement, or new topology requiring qualification. |
| `atomic_kms_pipeline_ready` | `(device_incarnation, lifecycle_epoch, protocol_crtc, topology_generation)` | Structural capability, `atomic_kms_cursor_policy = AtomicHardware` and incarnation qualification are true, and seat/output, owner, generations, recovery, and cursor state permit this submission. | Any per-submit gate closes. Owner occupancy alone does not make it false. |

**CAP-1 — cache stability.** VT, DPMS, busy owner, fd reopen, qualification
failure, and incarnation poison close qualification/readiness as applicable but
do not mutate an already advertised structural-capability bit. A measured
`CAP-4` demotion changes `atomic_kms_cursor_policy` for the device identity and
nothing else; it never rewrites structural capability either.

**CAP-2 — domain change.** Provider/output routing, domain-changing hotplug,
connector-class change, or server restart may construct a new protocol domain
and recompute advertisement before exposing it. A connector changing its
`non-desktop` property is a domain-membership change, not a mode-only topology
change. A card whose only connected connectors are `non-desktop` retains a
discovery-only DRM record/fd and udev-triggered reprobe but has no C.0 KMS owner,
mutation authority, protocol output or readiness state. If discovery monitoring
cannot be established, a connector-class change on that card is not claimed
live and requires server restart.

**CAP-3 — no readiness bootstrap or gamma coupling.** The C.0 owner may submit
the real qualification commit while C.1 readiness is false; only C.1 admission
consumes the cursor/primary ready gate. `atomic_gamma_capable=false` is a
terminal supported state for that CRTC and cannot close C.1 capability,
qualification, or readiness.

**CAP-4 — measured cursor policy, not a maintainer allowlist.** Cursor policy
separates a correctness gate from a quality gate. Only the correctness gate is
decided ahead of runtime, because only it protects something a measurement
cannot observe.

`OwnerMediatedLegacyMove` keeps an explicit allowlist. Its precondition is
`AuditedCursorExpansionHazard`, a conservative prediction derived from reading
the driver's source: whether that driver may add or serialize the cursor plane
despite userspace omission. No runtime measurement substitutes for a source
audit, and a wrong answer is an unmodelled concurrent mutation rather than a
slow cursor. A plane therefore selects this transport only on an exact match in
the audited-cohort table, which records the driver, kernel range, GPU class, the
audited expansion reasons and the `NativeCursorCompositionContract` rule.

`SynchronousAtomicMove` has no such precondition. It is the ordinary owner
commit with canonical out-fence evidence, and its failure mode is quality: it is
vblank-paced, occupies the sole atomic device slot, and can leave motion slower
than software composition. C.0 selects it optimistically on every device whose
structural capability and incarnation qualification pass, and withdraws it from
measurement rather than from a table.

`atomic_kms_cursor_policy` is per `(device_identity, driver_identity)` and
derived at runtime as defined in section 6.2. C.1 admission requires structural
capability, `atomic_kms_cursor_policy = AtomicHardware`, incarnation
qualification and per-submit readiness.

**Measured demotion.** The owner demotes a device to `SoftwareComposited` only
on the conjunction of a symptom and its attributable cause, over a window of
continuous cursor motion:

- `CursorServiceRate`, the distinct cursor generations retired per second, falls
  below `DemotionRatio` of the CRTC's mode-derived refresh rate, which is the
  achievable ceiling for a vblank-paced transport; **and**
- the p99 helper-measured duration of that window's cursor-affecting host calls
  exceeds `CursorHostCallMax`.

Both are required, and the conjunction is normative rather than a tuning
convenience. Section 9.2.1 tier-3 absorption legitimately pins cursor updates to
a slow client's cadence on healthy hardware, so the symptom alone would demote a
correct device; the measured host-call duration is what separates a driver
defect from ordinary scheduling. Demotion requires consecutive qualifying
windows, and a window whose atomic slot was occupied by primaries unrelated to
the cursor is discarded rather than counted against the device. Demotion is
remembered for that device identity for the server process lifetime, executes
through the section 11.4 hardware-to-software transition as an explicit policy
change, and never reopens within that process.

**Degradation prior.** A compiled-in table may record cohorts with recorded
measured failure, so a known-bad driver starts in `SoftwareComposited` without
ever exposing its degraded window. This table is an optimization and never a
safety mechanism: measured demotion protects every device whether or not it is
listed, so a missing, stale or wrong entry costs at most one short degraded
window and cannot produce an unsafe policy. Each entry carries the worst
recorded driver version; an installed version above that bound does not match
and the device starts optimistically again. Nothing promotes a device out of the
prior by measurement, because measuring the hardware path requires using it and
section 10.1 forbids synthetic probes. The version bound is the sole exit, and
it requires no spec revision.

Only stock, publicly released driver builds are valid evidence for either table.
C.0 records no patched, proposed, out-of-tree or unreleased build and assigns no
hypothetical future version range. Stock NVIDIA lacks the cursor async hook in
the source audited for this phase and therefore cannot select
`OwnerMediatedLegacyMove` under any measurement; its production cursor policies
are software composition and `SynchronousAtomicMove`.

Neither table is a rollout lever. Both are compiled in and expose no environment
variable, command-line flag, configuration key or user override, preserving
goal 11.

Discovery defaults are class-specific and explicit. The existing
`connector_is_non_desktop` policy remains fail-open on read-only property-query
failure so a transient query error does not silently remove an output.
Completion-property and event-identity discovery remains fail-closed because
those properties are evidence required to retire live state. Neither default
may be generalized to the other discovery class.

### 6.3. Commit evidence matrix

For every transaction, the owner first computes the closure from the final
serialized persistent property list, before adding any class-permitted
completion property:

```text
AtomicCrtcClosure =
    every CRTC with a persistent CRTC-property entry
    union every non-zero old or new CRTC_ID binding of each connector or plane
          having a persistent property entry

ExpectedCompletionCrtcs =
    every CRTC in AtomicCrtcClosure where old.active || new.active
```

The persistent property list is minimal rather than a full desired-state dump.
It contains every object whose persistent generation actually changes and any
additional object the kernel requires for that change. In particular, a
primary-only request omits an unchanged cursor plane entirely; it never adds
cursor properties or the plane merely to restate desired state. Coordinate-only
intent is not an atomic property-list input.

This is the userspace construction counterpart of the kernel's atomic CRTC
inclusion rule. A plane/connector move includes both powered endpoints; detach
therefore retains the old CRTC and attach retains the new one. Enable, disable,
modeset, and bound primary/cursor/color changes participate. Inactive-to-inactive
work may produce an empty set, but an empty set cannot qualify an incarnation.
For every class except C.1 async direct primary, the owner adds exactly one
`OUT_FENCE_PTR` property for every member of `ExpectedCompletionCrtcs`, and none
outside it. The C.1 async class adds none and uses its required sole page event
as specified in the matrix. The final serialized request is
re-scanned before dispatch; if its kernel-visible CRTC closure differs from the
recorded set, construction fails before submit. When `PAGE_FLIP_EVENT` is set,
`KernelEventCrtcs = ExpectedCompletionCrtcs`; `PresentEventCrtcs` is the subset
with a Present consumer. Events in the set difference are correlated and
drained but do not create protocol completion or an event deadline.
For a cohort that nominates `OwnerMediatedLegacyMove`, that same immutable
request is also passed to the cohort's `AuditedCursorExpansionHazard` rule after
all persistent and ephemeral properties are present. The resulting prediction
and reasons are installed in the `Submitting` record before dispatch; later
request mutation is forbidden. This classification does not alter or claim to
measure `AtomicCrtcClosure`.

The off-to-off restriction is about kernel `drm_crtc_state.event`, not only a
userspace-visible page event. Linux `prepare_signaling()` creates that event
state for every CRTC in the atomic state when either the request carries
`PAGE_FLIP_EVENT` or that CRTC carries `OUT_FENCE_PTR`; the later atomic check
rejects it when both old and new CRTC state are inactive. Therefore construction
fails before submit for every inactive-to-inactive CRTC in
`AtomicCrtcClosure` if the global page-event flag is set or an out-fence pointer
was assigned to that CRTC. An inactive-to-inactive closure member is permissible
only with neither signaling source. Adding an out-fence to an off-to-off CRTC
"for symmetry" is forbidden even when userspace requested no page event. The
`ExpectedCompletionCrtcs` powered-state filter and the rule that ephemeral
out-fence entries cannot enlarge `AtomicCrtcClosure` preserve this invariant,
and the final serialized re-scan checks it explicitly.

| Commit class | Producer gate | Required KMS evidence for `Completed` | Page event | Full teardown proof |
| --- | --- | --- | --- | --- |
| Nonblocking primary Present | Every source dependency completed before admission | Successful canonical out-fence status for all `ExpectedCompletionCrtcs` | Required for each Present CRTC and supplies MSC/UST | No; only class-specific replacement rules may release old/shared resources |
| Nonblocking non-Present primary/cursor/gamma/combined | Every source dependency completed before admission | Successful canonical out-fence status for all `ExpectedCompletionCrtcs` | None unless the record separately carries a Present consumer | No |
| Nonblocking C.1 async direct primary | One unresolved producer sync-file may be transferred through the changed primary plane's `IN_FENCE_FD`; export and fd ownership are proven before admission, but `ProducerReady` is not a pre-accept milestone | The successful correlated `PAGE_FLIP_EVENT` for the sole affected CRTC establishes both `HardwareComplete` and `Presented` for this commit class; atomic async requests carry no `OUT_FENCE_PTR` | Required for the sole CRTC and supplies MSC/UST | No; only the C.1 page-event replacement rule releases the previous primary buffer |
| Blocking ordinary | Every source dependency completed before admission | Successful ioctl return | None | Only if the blocking operation and resource-specific contract establish it |
| Blocking qualification | Every source dependency completed before admission | Successful ioctl return plus successful canonical out-fence status for all `ExpectedCompletionCrtcs` | None unless it separately carries a Present consumer | No |

The two blocking rows are legal only at the cold-start or final-offline boundary
defined by `COMMIT-5`; they are not alternative runtime submission modes.

The merged synchronized direct path is the first row, not an inherited
exception. C.0 replaces `drm::modeset::submit_direct_scanout` with an
owner-built request carrying a fresh `EventToken`, the complete canonical
out-fence set, and every compatible persistent cursor/gamma property required
by section 9. The existing zero-`user_data`, plane-only wrapper is not a valid
C.0 live submission.

**COMMIT-1 — one terminal state.** A transaction cancelled before executor
dispatch or explicitly rejected by the ioctl reaches `FailedBeforeSubmit`. Once
dispatched, any outcome that does not prove rejection—including IPC loss,
executor death, watchdog expiry, or a stale success—reaches `CompletionUnknown`
unless the current owner consumes an explicit success through the normal
`Completed` path.

**COMMIT-2 — typed milestones.** `ProducerReady`, `Dispatched`, `Accepted`,
`HardwareComplete`, `Presented`, and `PriorBufferReleased` are independent typed
facts. Observing one never fabricates another.

**COMMIT-3 — flip is not teardown.** An out-fence proves the CRTC flip/scanout
milestone. Full disable/cleanup requires a blocking owner barrier or another
resource-appropriate teardown barrier.

**COMMIT-4 — no unresolved C.0 input fence.** Every C.0 producer dependency
finishes successfully before admission; the live request omits `IN_FENCE_FD` or
uses `-1`. The C.1 async-direct row above is the sole extension: its request may
transfer one unresolved primary-plane input fence under the C.1 ownership
contract. It does not absorb cursor, gamma, connector, topology, or another
primary plane, and it carries no `OUT_FENCE_PTR` because the atomic async UAPI
rejects effective CRTC-property changes.

**COMMIT-5 — process-isolated host calls.** The device owner is the sole logical
submitter. A device-local `KmsIoExecutor` receives every serialized
request over message-boundary-preserving IPC (`SOCK_SEQPACKET` or an equivalent
framed transport), performs the raw atomic ioctl or a typed read-only clock probe
on one registered `IncarnationFdSet` alias, and returns the complete typed result;
atomic success also returns every out-fence through fd-passing. Atomic messages
carry incarnation, `LifecycleEpochId`, optional `LifecycleTransitionId`,
`CommitId`, and `EventToken`. A clock-probe message instead carries incarnation,
`LifecycleEpochId`, topology generation, hardware CRTC, CRTC clock epoch, and a
monotonic `ClockProbeId`; it owns no commit resources and cannot authorize KMS
state. Both message classes obey stale-result rejection and the same host-call
watchdog/reap rules. The X11 core never executes or waits
synchronously for a potentially blocking KMS ioctl. During normal seat-active
service every live commit, including modeset/install/recovery, uses `NONBLOCK`;
VT release and device removal never initiate a blocking barrier. Blocking atomic
calls are restricted to cold startup before service or final offline/shutdown
work after prompt lifecycle obligations have ended. `ValidationOnly` is neither
a live blocking commit nor a submitted record; it uses the exclusive validation
lease and watchdog defined in section 5.

The qualified coordinate-only transport is also a seat-active executor host
call. It is a typed message carrying incarnation, lifecycle epoch, plane identity, installed
cursor generation, the installed `NativeCursorCompositionContract` proof, the
overlapped primary's `AuditedCursorExpansionHazard` classification when any,
and newest coordinates; its typed reply includes the helper-measured ioctl
duration independently of IPC. A returned duration above
`CoordinateFastReturnMax` is subject to section 7.1's transport-health rule. It
has no
out-fence and creates no submitted atomic record. Before dispatch the owner
installs `CoordinateSubmitting` as a cursor-plane mutation reservation, not as
the atomic device slot. It may begin with the atomic slot idle or with exactly
one accepted coordinate-overlap-safe primary commit as defined in section 5,
but never
while another atomic ioctl is still `Submitting`. While the coordinate host
call itself is unresolved, the owner dispatches no new KMS host call. A typed
return releases the plane reservation while any compatible accepted primary
commit continues to own the atomic slot. Timeout, lost/invalid IPC, or helper
failure closes the transport and enters the coordinate case of
`ExecutorStalled`: no fallback or other KMS mutation may dispatch until actual
helper reap proves that the old ioctl can no longer mutate coordinates. After
reap, the newest coalesced coordinate is submitted through the selected
fallback and overwrites any acceptance-unknown old position.

**COMMIT-6 — dispatch is an uncertainty boundary.** Before sending IPC, the
owner installs a `Submitting` record, reserves the sole device slot, registers
its event identity, and transfers every possible old/new resource to that
record. Page events arriving during the host call are staged on it. Cancellation
is local only before IPC send. After send, only an explicit ioctl rejection
proves `FailedBeforeSubmit`; success is accepted even if its lifecycle tag is
stale, and missing/invalid reply, helper exit, IPC failure, or watchdog expiry is
acceptance-unknown and therefore `CompletionUnknown`. No second ioctl may be
dispatched on the device while this record or its executor lease exists.

The host-call watchdog starts at IPC dispatch: 2 seconds for seat-active
`NONBLOCK` work and 30 seconds for an allowed cold-start/final-offline blocking
call. Expiry terminalizes protocol work, closes readiness, quarantines both
possible states, requests executor termination, and asynchronously reaps it; it
never blocks core dispatch or prompt VT/device-loss obligations. If the executor
cannot yet be reaped because the kernel call is uninterruptible, the device
enters `ExecutorStalled`: logical outputs are withdrawn and no fd retirement,
resource release, new incarnation, or automatic retry is permitted until reap
proves the executor alias and lease gone. Kernel recovery is intentionally not
claimed while that external stall persists.

**COMMIT-7 — orderly-exit reap barrier.** Logical shutdown immediately stops
protocol admission, terminalizes client work, and releases the seat without
waiting for an executor. Physical server-process exit is separate: the parent
must retain quarantine and its teardown supervisor until `waitpid`/`waitid` (or
an equivalent platform primitive) proves every executor/helper terminated and
all registered leases are released. Sending a termination signal, closing the
IPC channel, `PR_SET_PDEATHSIG`, or a watchdog expiry is a request, not reap
proof. If a child remains in an uninterruptible kernel call the parent waits in
`ShutdownExecutorStalled` until its teardown deadline and then exits. Reap
remains the preferred path; the bounded exit is a fallback that records the
unreaped lease, the helper identity and the device, leaves the helper orphaned
for `init` to reap, and never claims the lease was released.

The guarantee that no later incarnation installs state underneath a still-live
helper comes from neither exit policy. It comes from a device-scoped advisory
lock — one `flock` per DRM device, or its platform equivalent — taken by the
executor for as long as it lives and released only by its death. Every server
start consults that lock before installing any state. While it is held, a helper
of an earlier incarnation can still have a KMS ioctl accepted, so the start waits
or refuses rather than installing. The lock survives `SIGKILL`, survives
reparenting to `init`, and does not depend on the parent still running.

Three facts fix that placement, recorded here so the rule is not relitigated.
First, the kernel closes this window by itself only for a directly opened
device: `drm_setmaster_ioctl()` returns `EBUSY` while `dev->master` is set, and
`drm_master_release()` runs from `drm_file_free()` for a primary-node client, so
a wedged helper holding the last reference to that `drm_file` prevents a new
server from taking master at all. Second, that protection does not exist under
seat management, which is the ordinary desktop case: the `drm_file` belongs to
logind and yserver's fd — and by inheritance the helper's — is a dup of it, so
logind's `DROP_MASTER`/`SET_MASTER` for the next session act on the very
`drm_file` the wedged ioctl is using, and a stale commit can land under the new
session's state. Third, waiting indefinitely does not cover `SIGKILL`: a service
manager's stop timeout kills the parent, which is interruptible in `waitid`,
while the helper inside an uninterruptible call survives, so the window opens
anyway with no deadline and no record. The bounded exit therefore opens the
residual window at a chosen and logged moment instead of an arbitrary one, and
the device lock is what closes it.

Executors are created by reexecuting the yserver helper mode through
`posix_spawn` or fork-followed-immediately-by-exec using only async-signal-safe
child setup; they never run Rust allocator, locks, Vulkan, GBM, or backend code in
a forked multithreaded child before exec. Linux may additionally arm
`PR_SET_PDEATHSIG` and/or pidfd supervision as crash containment, but those are
not orderly-shutdown evidence and portable builds use their platform-equivalent
spawn/wait supervision.

### 6.4. Device lifecycle matrix

| Device state | Live KMS admission | Resource rule | Exit |
| --- | --- | --- | --- |
| `Unqualified` | C.0 install/restore only; never C.1 | Retain desired protocol state; no synthetic probe | Mandatory real qualification commit completes |
| `Ready` | Owner policy permits C.0 and qualified C.1 | Normal commit/resource matrix | Quiesce, rejection latch, or completion breach |
| `Quiescing` | Closed | Drain, reject-before-submit, or terminalize pending work | Lifecycle transition completes or becomes unknown |
| `Poisoned` | Closed, including ordinary primary/composed work | Quarantine every possible state; retire complete fd family | Sole automatic recovery attempt or lifecycle-specific teardown |
| `Recovering(RecoveryId)` | Fresh install/qualification only | Preserve device-independent desired state; no recursive recovery | `Ready` on qualification, otherwise `RecoveryFailed` |
| `RecoveryFailed` | Closed | Logical withdrawal; retain quarantine until proven barrier | Actual hotplug identity change, VT reacquire, administrative reprobe, or restart creates at most one fresh attempt |
| `ExecutorStalled` | Closed | Logical withdrawal; retain the complete submitting record, quarantine, executor alias, and lease | Executor reap, then the still-current lifecycle transition may retire the fd family and continue once; otherwise remain withdrawn |
| `ShutdownExecutorStalled` | Closed permanently | Logical shutdown is complete; retain teardown supervisor, quarantine, aliases, and leases until the teardown deadline | Prefer reap of every child and release in barrier order; at the deadline exit with the unreaped lease recorded, leaving the device lock held by the orphaned helper |
| `Removed` | Closed | Logical withdrawal and device-loss teardown | Newly discovered device identity/incarnation only |

**REC-1 — one automatic attempt.** Each normal-operation completion-loss
incident creates exactly one `RecoveryId`. Any failure or unknown completion
during that attempt enters `RecoveryFailed`; timers, DPMS, client traffic, and
queued intents cannot recurse or retry it.

**REC-2 — logical versus physical disable.** Hardware disable is a KMS mutation
allowed only through a healthy incarnation with provable evidence. Logical
withdrawal changes the backend/RANDR model and makes no claim that physical
scanout changed.

**REC-3 — unknown cursor detach.** Accepted detach with unknown completion
suppresses software reveal, destination attach, and every further live commit on
the poisoned incarnation until teardown plus a fresh proven attach/detach.

**REC-4 — one lifecycle arbiter.** Each device owns at most one
`LifecycleTransition { id, kind, phase }`. Event precedence is:

```text
Shutdown
> DeviceRemoved
> VTRelease
> DeviceAddedOrReplaced
> VTAcquire
> AdministrativeReprobe
> IdentityChangingHotplug
> TopologyRebuild
> DPMS
> NormalRecovery
```

The arbiter guarantees prompt logical progress while an executor host call is
outstanding. This is structural: no driver wait occupies the X11 core.

A higher-priority event supersedes the active transition; an equal event
coalesces by the typed `REC-5` rules; a lower event updates `LifecycleDesired`
but cannot start work until the current transition terminalizes and convergence
revalidates it. Supersession stops
admission/alias creation, cancels pre-submit work and helpers, terminalizes each
Present exactly once, preserves only remappable tickets/intents, and transfers
all quarantine ownership to the winner. A `Submitting` or accepted commit is
never cancelled as never-submitted; it is terminalized or quarantined under
section 10. No transition may open a new fd, run final `TEST_ONLY`, or install
state unless its id and lifecycle epoch are still current. `RecoveryId` is a
distinct incident identity, never derived from `LifecycleTransitionId`; its fate
under supersession is the total `REC-6` matrix below.

A late `KmsIoExecutor` result whose incarnation/lifecycle epoch is stale, or
whose present transition id is no longer current, cannot promote installed state
or restart submission. An explicit rejection permits
generation-local never-submitted cleanup. An explicit success is accepted-stale:
the owner adopts/closes every returned fd exactly once, terminalizes protocol
work once, and retains both possible state/resource sets in the winning
transition's quarantine. An absent or invalid result remains
acceptance-unknown with the same quarantine. The executor lease prevents old-
incarnation retirement until return or reap. Because the executor is isolated,
VT/device-loss handling performs prompt logical obligations without waiting for
the host call; physical retirement waits truthfully in `ExecutorStalled` when
the kernel cannot return it.

**REC-5 — bounded lifecycle-intent convergence.** Lifecycle events never form an
unbounded FIFO. Before arbitration, each event receives a monotonic
`LifecycleEventId` and updates one owner-held `LifecycleDesired` snapshot:

```text
shutdown_requested                 // monotonic true
device_presence + identity_epoch   // newest observation
seat_target + seat_epoch           // newest acquire/release target
administrative_reprobe_epoch        // newest explicit request, if any
topology_dirty + discovery_epoch + change_class
                                      // same-identity or identity-changing;
                                      // payload is always rediscovered
protocol_dpms_level + dpms_epoch    // X11's newest global protocol request
dpms_target[protocol_output]        // its projection per stable output identity
recovery_incident                   // the sole existing RecoveryId, never cloned
```

X11 DPMS remains one global protocol control, not a new per-output client API.
Accepting a protocol request first replaces `protocol_dpms_level` and then, in
the same `dpms_epoch`, projects that level into every current stable protocol
output's `dpms_target`. A newly discovered output inherits the current global
level before its topology can be installed; removing an output invalidates and
forgets only its projected target. Owners on different DRM devices converge
their projections independently under the same global epoch, while compatible
outputs on one device are constructed as the one ordered atomic transition
required by sections 9 and 10. The protocol request has one representative
event id per affected owner; projected outputs carry that representative and
epoch but do not manufacture client-visible per-output requests. Its owner-local
representative is `Applied` only after every still-current projection on that
device retires or is explicitly invalidated by output removal. A partial best-
effort loop of per-output commits is not a valid C.0 DPMS transition and a
single global `kms_outputs_active` boolean is not proof that all projected
targets retired.

The snapshot retains at most one representative `LifecycleEventId` per field
(per owner for the global DPMS field); the projected target map is bounded by
the current protocol-output domain, not by event count. Equal-kind coalescing is
typed:
shutdown/removal duplicates are immediately assigned
`AbsorbedByEvent(representative_event_id)`;
newer device-presence, seat, administrative-reprobe, and global DPMS
generations replace the representative and give the displaced event
`SupersededBy(newer_event_id)`; topology events set
`topology_dirty`, retain only the newest discovery epoch, and similarly
terminalize the displaced representative because execution must rediscover
rather than replay an old object payload. Duplicate normal-recovery events are
immediately assigned `AbsorbedByEvent` referencing the existing incident's
representative and cannot create another `RecoveryId`.

While a transition is active, lower-priority changes update `LifecycleDesired`
but cannot start a second transition. The active transition must build every
final `TEST_ONLY` and install from a fresh snapshot read under the owner. On
completion or supersession, the arbiter revalidates every changed field against
the resulting incarnation, topology, seat, and protocol state, then maintains
one current disposition for every retained representative `LifecycleEventId`:

- `Applied(transition_id)`: that transition directly established its target;
- `AbsorbedByEvent(representative_event_id)`: an idempotent/coalesced event was
  satisfied by the retained representative before a transition id existed;
- `AbsorbedByTransition(transition_id)`: the winner's fresher installation already
  established the same target;
- `Invalidated(reason)`: shutdown, device identity loss, protocol-output
  removal, or a newer generation made the requested target meaningless;
- `SupersededBy(newer_event_id)`: typed latest-wins coalescing replaced this
  representative before execution;
- `Deferred(prerequisite)`: the target remains desired but cannot run while a
  required external state such as VT ownership is absent. This is a nonterminal
  snapshot disposition, not queued work; it must become `Applied`, one of the
  `AbsorbedBy*` outcomes, or `Invalidated` after the prerequisite changes or
  shutdown invalidates it.
All other dispositions are terminal and immutable; each event id reaches exactly
one terminal disposition and is then forgotten except for bounded telemetry
aggregation.

If a still-valid field remains unsatisfied and its prerequisite is present, the
arbiter selects the highest-precedence unsatisfied kind and creates exactly one
new `LifecycleTransitionId` after the previous transition is terminal. It may
repeat this convergence step, but never has two transitions and never replays an
obsolete hardware-object payload. Recovery supersession follows `REC-6`; DPMS
and ordinary traffic remain unable to create retry authority.

**REC-6 — total transition mapping and recovery fate.** Every unsatisfied
`LifecycleDesired` field maps to exactly one kind: absent device to
`DeviceRemoved`, newly present/changed device identity to
`DeviceAddedOrReplaced`, released/acquired seat target to
`VTRelease`/`VTAcquire`, explicit request to `AdministrativeReprobe`, structural
identity change to `IdentityChangingHotplug`, same-identity discovery dirtiness
to `TopologyRebuild`, output power targets to `DPMS`, and an unconsumed
completion-loss incident to `NormalRecovery`. `DeviceAddedOrReplaced` and
`VTAcquire` defer respectively on seat ownership and device presence rather than
guessing or opening an unusable fd.

An active `RecoveryId` is handled by the winning transition as follows:

| Winning kind | Existing recovery incident | Qualification/failure rule |
| --- | --- | --- |
| `Shutdown` | Terminal `Invalidated(Shutdown)`; quarantine transfers to shutdown teardown | No recovery or retry |
| `DeviceRemoved` | Terminal `Invalidated(DeviceRemoved)`; quarantine transfers to device-loss teardown | No retry until a newly present identity is an external boundary |
| `VTRelease` | Terminal `Invalidated(VTRelease)` after logical/seat obligations; quarantine remains with old-incarnation teardown | A later `VTAcquire` may allocate one fresh `RecoveryId` if recovery is still required |
| `DeviceAddedOrReplaced`, `VTAcquire`, `AdministrativeReprobe`, or `IdentityChangingHotplug` | The old incident is terminally invalidated by that authorized external boundary; if installation/recovery is required, the winner allocates exactly one fresh `RecoveryId` before its first attempt | Successful real install consumes it as recovered; any stage failure/unknown becomes `RecoveryFailed` for that fresh id |
| `TopologyRebuild` on the same identity | Transfer the same incident/quarantine and remaining one-attempt budget to the winner | Qualified real install consumes it as recovered; failure/unknown terminalizes that same id as `RecoveryFailed` |
| `DPMS` | Pause the same incident without consuming or cloning its attempt. DPMS-off defers it on `dpms_target=On`; DPMS-on resumes it after the DPMS transition | DPMS alone never revives `RecoveryFailed` or allocates an id |
| `NormalRecovery` | Continue the same incident; equal events are absorbed | Exactly the single attempt in `REC-1` |

Thus a recovery id is never both invalidated and transferred, every invalidation
has a named external authority for any successor id, and a lifecycle transition
may finish only after recording the matrix outcome.

## 7. Atomic cursor payload

Requirements in this section are identified as **CURSOR-PAYLOAD**.

Each active CRTC is assigned a distinct compatible universal cursor plane. The
existing per-card plane matching remains responsible for proving simultaneous
coverage; a single plane cannot satisfy two active CRTCs.

The desired cursor state contains at least:

```rust
struct AtomicCursorDesired {
    visible: bool,
    framebuffer: Option<CursorFramebuffer>,
    crtc: crtc::Handle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    hotspot_x: i32,
    hotspot_y: i32,
    generation: u64,
}
```

The KMS plane properties derive from the desired state:

- visible: `FB_ID`, `CRTC_ID`, `SRC_*`, and `CRTC_*` describe the cursor;
- hidden/detached: `FB_ID=0` and `CRTC_ID=0`;
- `CRTC_X/Y` use hotspot-adjusted CRTC coordinates;
- clipping and negative positions preserve partially off-screen cursors;
- the source rectangle remains in framebuffer coordinates and must not encode
  the hotspot twice.

Cursor-plane qualification validates `possible_crtcs`, supported pixel format
and modifier, maximum dimensions, and the no-scaling property set used by
yserver. The selected framebuffer is plane-local: crossing to another plane or
device reuses it only after proving the exact format/modifier/size import;
otherwise the destination receives its own upload while the source generation
remains retained.

Cursor discovery has one mandatory, ordered capability negotiation per device
incarnation. Yserver first enables `DRM_CLIENT_CAP_ATOMIC`, then attempts
`DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT=1`, and only then enumerates planes and
their properties:

- success selects `HotspotMetadataRequired`; virtualized cursor planes are now
  visible, and every selected cursor plane must expose both `HOTSPOT_X` and
  `HOTSPOT_Y`;
- `EOPNOTSUPP` selects `NativeCoordinateOnly`; this is the documented normal
  result for non-virtualized drivers and does not disable hardware cursor;
- any other error makes cursor discovery fail for that incarnation. During
  initial protocol-domain construction it prevents structural capability from
  becoming true; on a later incarnation it closes qualification/readiness but,
  under `CAP-1`, does not rewrite an already advertised bit.

In `HotspotMetadataRequired`, missing only one hotspot property, missing both,
or rejection of either property makes that plane unusable; yserver never retries
that virtualized plane without metadata. It programs the unmodified logical
hotspot into both properties on every visible attach/image change, while still
subtracting the hotspot exactly once from `CRTC_X/Y` for visual placement. In
`NativeCoordinateOnly`, no hotspot properties are required or programmed and
the same single coordinate subtraction supplies normal image-top-left plane
placement. Property exposure is checked against the negotiated mode: observing
hotspot properties after `EOPNOTSUPP`, or their absence after successful
negotiation, is a discovery contradiction rather than a guessed fallback.

Cursor edge handling is qualified once per plane/incarnation, never guessed per
movement. The mandatory default is `FullSourceSignedDestination`: compute image
origin `(root_x - output_x - hotspot_x, root_y - output_y - hotspot_y)` in
checked signed arithmetic; keep `SRC_X/Y=0`, `SRC_W/H` equal to the complete
framebuffer in 16.16, and keep `CRTC_W/H` equal to the complete framebuffer
while encoding signed, possibly negative `CRTC_X/Y`. This matches the kernel's
legacy-cursor translation and i915's requirement that cursor source panning
remain zero. A partially offscreen cursor therefore stays attached with its
complete source and signed destination. An entirely disjoint rectangle may be
detached through the ordered visibility path.

An optional `SourceCrop` policy is selectable only when a serialized
qualification `TEST_ONLY` on that exact plane, format/modifier, framebuffer
size and topology proves representative left, top, right and bottom clipping
with non-zero `SRC_X/Y` and reduced source/destination sizes. The decision is
cached for that plane incarnation and invalidated by topology, format, size or
incarnation change; movement never probes or switches policy. A rejected crop
selects `FullSourceSignedDestination`, not software fallback. If the mandatory
full-source shape itself fails qualification, that plane is not cursor-capable.

Signed DRM property values are encoded by checked sign-extension through `i64`
to `u64`, never by a narrowing cast. Overflow, unsupported format/modifier/
size, or a required scale is not submitted: it selects the ordered software-
cursor transition instead.

### 7.1. Coordinate transport qualification

Linux currently reaches a driver's `atomic_async_check`/
`atomic_async_update` cursor hook through the legacy plane update path, which
sets `legacy_cursor_update`; `drm_mode_atomic_ioctl` exposes no equivalent
cursor-coordinate flag. C.0 qualifies the viable coordinate transports for the
exact driver/kernel/GPU/plane-incarnation cohort through measurement rather
than vendor-family inference:

- `OwnerMediatedLegacyMove` is eligible only when a real baseline/qualification
  probe has placed that exact driver/kernel/GPU/plane class in the immutable
  audited-cohort table, including external input-to-visible evidence no worse
  than the shipping legacy baseline. This is the one transport `CAP-4` still
  gates ahead of runtime, because its precondition is a source audit. Where the driver may expand a userspace request or
  restrict its async hook from below-cursor state, that evidence also supplies
  the checked `NativeCursorCompositionContract` and cohort-specific
  `AuditedCursorExpansionHazard` rule. Runtime does not infer an internal async
  hook merely from one fast ioctl return. It requires an exact cohort-table
  match, a contract-valid installed primary, the qualified plane incarnation,
  unchanged installed framebuffer/binding and only coordinate fields in the
  cursor request.
- `SynchronousAtomicMove` uses the ordinary owner commit and out-fence evidence
  where no qualified fast legacy transport is available. It is vblank-paced and
  occupies the atomic device slot. It is selected optimistically wherever
  structural capability and incarnation qualification hold, and is withdrawn by
  the `CAP-4` measured demotion rather than by absence from a table.
- `SoftwareCursor` remains viable independently of a hardware-transport failure.
  It is the terminal fallback: the state a device reaches when it lacks
  structural capability, matches a degradation prior, or has been demoted by
  measurement. It is no longer the default for hardware merely because no
  campaign has been run against it.

The C.0 NVIDIA model considers only stock, publicly released driver builds.
Those builds expose no cursor-plane `atomic_async_update` hook in the source
audited for this phase, so no NVIDIA cohort in C.0 can select
`OwnerMediatedLegacyMove`. A composed NVIDIA desktop uses `SoftwareCursor`;
direct scanout may use `SynchronousAtomicMove` only after its own stock-driver
gate passes, otherwise it performs the ordered software/unflip transition. C.0
does not measure, mention in a support table, or derive future capability from
an out-of-tree, patched, proposed, or unreleased driver. A later released driver
with a new mechanism requires a future spec revision and complete new cohort
evidence; it is not anticipated by this document.

For the required Raphael iGPU cohort (RDNA2/DCN 3.1.5) across the kernel range
verified in section 4.1, C.0 does not
attempt to mirror AMD's hysteretic cursor-mode state. Every primary constructed
while `OwnerMediatedLegacyMove` is selectable must instead satisfy
`NativeCursorCompositionContract`: one full-mode XRGB8888 primary at `(0,0)`,
1:1 source/destination scale, fixed z-order below the cursor, and no active
plane color pipeline. The merged direct path already rejects non-XRGB8888,
non-root-sized and non-exact-output-tiling candidates before KMS; composed
scanout programs the same full-mode shape. Specifically,
`kms/vk/scanout.rs` registers the composed `VkScanoutFb` as
`DrmFourcc::Xrgb8888`, while the direct gate in `kms/render/backend.rs` requires
`DRM_FORMAT_XRGB8888`; both are load-bearing sources for the fixed-format
contract. C.1 inherits this eligibility and does not add scaled, HDR/10-bit,
YUV/video or partial-coverage scanout. Direct eligibility and every owner
primary builder assert the contract. A direct candidate that fails it uses the
existing composed fallback.

Any future path that constructs an out-of-contract primary must invalidate
`OwnerMediatedLegacyMove` during construction, before that primary's ioctl may
begin, and complete the ordered transition away from the transport before
installing the primary. Canonical completion is too late for this invalidation.
After such an installation the same plane incarnation cannot simply reselect
the fast transport when a later primary passes the contract. Re-entry requires
complete plane-incarnation requalification: atomically detach and reattach the
cursor with the contract-valid primary already represented in the state, then
rerun the cohort qualification. The reattach makes
`drm_atomic_plane_enabling()` set AMD's cursor-mode reconsideration, while the
associated CRTC `plane_mask` change makes `drm_atomic_normalize_zpos()` include
the affected planes, so the driver recomputes against the contract-valid
primary rather than a cursor-only state. This future extension requires a
reviewed spec and new cohort evidence; current C.0 has no producer of an out-of-
contract primary.

Omitting the cursor from userspace's request is necessary but not sufficient
for overlap on AMDGPU. `dm_crtc_get_cursor_mode()` obtains and thereby adds the
cursor plane when an enabled cursor shares a CRTC with a relevant plane
enable/disable, framebuffer-format, scale-ratio, z-order or plane color-pipeline
change. Earlier in `amdgpu_dm_atomic_check()`,
`drm_atomic_add_affected_planes()` also adds every current plane on an enabled
CRTC when the request needs a modeset, changes CRTC color management, changes
VRR state, or carries `dsc_force_changed`. DRM marks CRTC color management
changed when `GAMMA_LUT`, `CTM` or `DEGAMMA_LUT` is replaced, so C.0 gamma-only
commits are expansion hazards even though they do not pass through the cursor-
mode trigger.

For this cohort, `AuditedCursorExpansionHazard` is therefore true for the
complete final serialized request when it includes or implies any of: modeset;
CRTC `GAMMA_LUT`, `CTM` or `DEGAMMA_LUT` replacement; VRR change; DSC-force
change; relevant plane enable/disable or binding; framebuffer-format change;
scale-ratio change; normalized-z-order change; or plane color-pipeline change.
The classification is installed before host-call dispatch and remains on the
accepted record through canonical completion. Coordinate overlap is permitted
only for a primary-only request that preserves the native contract, omits the
cursor in the userspace object set, and has no audited hazard reason. Gamma,
modeset and all other non-primary classes already exclude coordinate overlap;
the explicit hazard still prevents later code from misclassifying them. A
kernel/driver revision that changes the expansion set creates a new cohort, and
so does a change of display IP version; C.0 never guesses from vendor name or
reports this prediction as measured kernel closure. The DCN native-cursor
exemption list is 4.0.1, 4.2.0 and, from Linux 7.2, 4.2.1. Those are DCN 4.x, so
neither DCN 3.0 nor the required cohort's DCN 3.1.5 is exempt, and that reading
transfers across the substitution. From the same revision
`dm_crtc_get_cursor_mode()` additionally returns native mode early for a
disabled CRTC, which cannot affect an enabled CRTC on either IP version. Both
readings hold across the range verified in section 4.1.

The trigger set above does **not** transfer by the same argument. It was derived
by reading `amdgpu_dm` against DCN 3.0, and cursor-mode selection and overlay
handling are IP-version-specific. Before `OwnerMediatedLegacyMove` is
allowlisted for the Raphael iGPU cohort, the audit is redone against DCN 3.1.5
and its reasons recorded for that cohort. Until then the cohort is not in the
audited-cohort table and its cursor policy is decided entirely by `CAP-4`, which
needs no audit: structural capability plus qualification select
`SynchronousAtomicMove`, and measurement withdraws it if it underperforms.

Runtime preference is state-derived, not a configuration lever. A qualified
`OwnerMediatedLegacyMove` is preferred in both composed and direct presentation.
Without it, a composed desktop uses `SoftwareCursor`; direct scanout uses
`SynchronousAtomicMove` only when that exact cohort passed its continuous-
primary qualification, otherwise the ordered software-cursor transition exits
direct scanout. Thus composition favors fluid cursor interaction while direct
scanout may preserve scanout with a vblank-paced hardware cursor. Crossing the
composed/direct boundary re-evaluates this fixed preference table and performs
the existing ordered HW/SW transition; it does not probe during motion or
oscillate on individual results. A driver, kernel, GPU, plane incarnation, mode
or relevant topology change invalidates the applicable runtime incarnation
qualification before reuse. Every primary construction rechecks the native
contract, and every final serialized atomic request receives a fresh audited
hazard classification before dispatch. A driver/kernel/GPU identity change
additionally requires new release evidence. Phase C.2 may replace this table
when it introduces the above-vblank atomic transport.

`OwnerMediatedLegacyMove` is dispatched only from the owner with no atomic
`Submitting`, lifecycle barrier, or exclusive validation lease. The atomic slot
may be idle or held by one accepted contract-preserving primary commit whose
userspace object set omitted the cursor and whose audited expansion hazard is
false; every other accepted class blocks it. The move contains only `CRTC_X/Y`
for the current installed cursor state. On kernels
using the atomic helpers, the compatibility ioctl constructs its own atomic
state internally; userspace never treats it as an independent owner.
`CoordinateFastReturnMax` is one millisecond. A slow return means a successful
call exceeded that transport-health maximum; an unresolved call instead
follows the executor watchdog and reap rules. On atomic-helper drivers,
an internal `atomic_async_check()` rejection is not a userspace errno: the
helper sets `state->async_update=false` and continues through its ordinary
commit path. That ordinary blocking path waits for dependencies before its
commit tail and may cost an output period. Therefore any returned over-bound
call is a coordinate-policy/cohort defect, not runtime composition detection:
it closes `OwnerMediatedLegacyMove` for the complete plane incarnation, emits telemetry
with the contract proof, exact userspace request, audited hazard classification
and duration, and moves the newest point to `SynchronousAtomicMove` or the
ordered software transition. It cannot be rehabilitated within that plane
incarnation. A userspace-visible rejection other than the eligible concurrent
`EBUSY`, topology/lifecycle invalidation, an uncertain return, or evidence that
the request changed more than coordinates remains a non-composition failure and
closes the transport at the existing incarnation/reap scope. An eligible
concurrent `EBUSY` is a benign
pre-submit rejection: it does not close the transport and retains/coalesces the
newest point for exactly one deferred retry after the accepted primary
completes. At that completion wake, a waiting lifecycle barrier still wins;
otherwise the single coordinate retry dispatches before the next atomic primary
admission. Consecutive coordinate `EBUSY` means attempts with no successful
coordinate return between them; every successful coordinate return resets the
count. A second consecutive `EBUSY`, or any `EBUSY` without the recorded
overlap-safe primary conflict, closes the transport. Completion of an atomic
request does not itself reset the count, and there is no immediate retry loop.
Every fallback waits for
`CoordinateSubmitting` to clear, and an uncertain isolated result clears only
after helper reap, never merely at watchdog or IPC failure. Phase C.2 owns a
future atomic-UAPI replacement and its removal plan; C.0 neither claims nor
invents that UAPI.

The desired state is separate from submitted and retired state. Updating the
desired cursor never releases the framebuffer referenced by an in-flight or
currently scanned atomic state.

## 8. Atomic gamma payload

Requirements in this section are identified as **GAMMA-PAYLOAD**.

Each CRTC discovers and retains the `GAMMA_LUT` property handle and reads
`GAMMA_LUT_SIZE`. The property is usable only when both exist, the size is
non-zero, converts exactly to RANDR's `u16` size, and
`size * size_of::<DrmColorLut>()` succeeds in checked arithmetic and does not
exceed 1 MiB. A larger, unrepresentable, or overflowing value sets only
`atomic_gamma_capable=false` and exposes RANDR gamma as unavailable; it does not
alter cursor/primary capability, incarnation qualification, readiness, or C.1
eligibility. The value is never clamped or partially allocated.

This discovery replaces, rather than supplements, the merged baseline's
legacy `get_crtc().gamma_length()` query and every cache fallback that clamps
an unrepresentable length to `u16::MAX`. Resume, hotplug, reassignment,
`GetCrtcGammaSize`, cached-ramp resampling, and `SetCrtcGamma` all consume the
validated atomic property value or the explicit gamma-unavailable state; no
legacy size can survive as a second source of truth.

RANDR supplies three equally sized `u16` arrays. C.0 validates them against the
advertised size and encodes one array of DRM color-LUT entries:

```rust
#[repr(C)]
struct DrmColorLut {
    red: u16,
    green: u16,
    blue: u16,
    reserved: u16,
}
```

`reserved` is always zero. The exact kernel ABI layout is pinned by size,
alignment, and byte-level tests on supported targets. No Linux-specific libc
request alias is introduced.

The device creates a property blob from the complete LUT and submits its id as
the CRTC's `GAMMA_LUT` value through the device-local atomic owner. A gamma
change may be combined with a compatible pending modeset/primary/cursor intent,
or submitted alone when no such commit is imminent. It must not wait
indefinitely for scene damage.

### 8.1. Gamma blob lifetime

The current yserver RANDR model allocates one stable protocol CRTC id alongside
each connector-qualified `OutputKey`; `OutputKey` is an output/connector
identity, not a hardware or protocol CRTC type. The protocol-id mapping and the
gamma cache deliberately share that stable key. A topology installation maps
the key to its current device/hardware CRTC and advertised LUT size. Provider
routing or any future model that permits a protocol CRTC to move independently
of its `OutputKey` must first introduce a distinct stable gamma-owner key; it
must not guess from the hardware handle.

If reassignment changes the hardware LUT size, yserver resamples the cached
client ramp with the existing endpoint-preserving gamma helper before it creates
the replacement blob. Gamma follows the stable protocol mapping across DPMS,
VT, and hardware-CRTC reassignment and is never inherited accidentally by an
unrelated output that later receives the old hardware handle.

Coalescing happens on the complete `u16` triplet before blob creation. The owner
creates at most the blob needed for the next submit, so a client issuing rapid
updates cannot create an unbounded sequence of kernel blobs. The owner tracks
desired ramps plus pending and current gamma blob ids per mapped CRTC:

- failure before successful submission destroys the new unreferenced blob;
- a submitted blob remains alive through commit completion;
- the previously current blob is destroyed only after a newer commit proves it
  has been replaced;
- queued gamma changes coalesce latest-wins, destroying only blobs never
  submitted and no longer desired;
- VT, DPMS, hotplug, CRTC removal, shutdown, and device loss follow the explicit
  commit-terminalization rules in section 10.

Identity gamma is represented explicitly by an identity LUT blob unless the
driver documents and tests `GAMMA_LUT=0` as equivalent for that CRTC. Reset and
resume restore yserver's cached desired LUT; they do not silently reset client
gamma.

### 8.2. Unsupported color management

A CRTC without usable atomic `GAMMA_LUT` sets
`atomic_gamma_capable=false` and uses RANDR size zero as C.0's explicit
unavailable representation. This is not claimed to be a commonly exercised Xorg
KMS shape and requires the real-client compatibility gate below. This terminal color-management state does not change
`atomic_kms_pipeline_structurally_capable`, `atomic_kms_cursor_policy`, C.1
qualification, or C.1 readiness:
`GetCrtcGammaSize` returns zero and `GetCrtcGamma` returns empty channels.
`SetCrtcGamma` applies the following validation and error precedence regardless
of whether gamma is supported:

1. validate the fixed request header and minimum request size (`BadLength` on a
   truncated header);
2. resolve the RANDR CRTC with read access, equivalent to Xorg's
   `DixReadAccess`; an invalid id returns the RANDR `BadCrtc` extension error and
   any access denial returns its lookup error;
3. return `BadAccess` if that CRTC is leased;
4. compute the checked minimum payload for the declared `size` as three `CARD16`
   arrays plus X11 four-byte padding, and return `BadLength` if the request is
   shorter; overflow or an unrepresentable request length is also `BadLength`;
5. compare declared `size` with the advertised CRTC gamma size and return
   `BadMatch` on mismatch;
6. apply the three arrays and return `Success`. Bytes beyond the padded minimum
   are accepted and ignored, matching Xorg's `REQUEST_AT_LEAST_SIZE` behavior.

Consequently an unsupported CRTC accepts a well-formed request whose declared
`size` is zero as a no-op, including one with trailing bytes. A well-formed
non-zero declaration returns `BadMatch`, but a request whose body is too short
for that declaration returns `BadLength` first. No branch falls back to
`set_gamma`; C.0 removes that call site from production KMS entirely. Unit tests
pin the complete precedence and trailing-byte behavior at the protocol boundary
rather than only testing the backend trait.

Before merge, a deliberately gamma-less CRTC or test-only equivalent runs
`redshift`, `gammastep`, and one Proton title that exercises gamma. A crash,
division-by-zero, unrecoverable client loop, or material regression leaves the
spec Draft and requires an explicit compatibility redesign; C.0 does not invent
a silent 256-entry no-op LUT to make the clients pass.

`DEGAMMA_LUT`, `CTM`, HDR metadata, and color-pipeline extensions are separate
features. They are not required to preserve the existing RANDR gamma contract.

## 9. Device-local atomic commit owner

Requirements in this section are identified as **SCHED**. They govern bounded
intent storage and admission only; section 10 governs accepted-commit lifecycle.

Every opened KMS device owns one scheduler. All state-changing call sites submit
intent to it rather than issuing `atomic_commit` directly.

There is exactly one dispatched-or-submitted live atomic transaction per DRM
device, not one per CRTC. `Submitting` reserves that slot before executor IPC;
the slot is not released merely because the ioctl result is late. This
deliberately conservative C.0 rule serializes commits
that could conflict through shared planes, connectors, routing, or a multi-CRTC
transaction. A later phase may admit disjoint concurrent commits only after it
tracks complete DRM object conflict sets; CRTC identity alone is insufficient.
The section 7.1 coordinate transport is not an atomic transaction: its
per-cursor-plane `CoordinateSubmitting` reservation may overlap the completion
interval of the one accepted contract-preserving primary whose final serialized
request omitted the cursor and was conservatively classified with
`AuditedCursorExpansionHazard=false`, but it blocks every new KMS host-call
dispatch until its typed return or helper reap.

This is the named `SingleSlotMultiCrtcCeiling`, an accepted C.0 throughput
limitation. A multi-CRTC protocol domain is C.0-complete only when all active
CRTCs have the same exact mode-derived refresh rational and hardware
qualification proves the per-CRTC fairness and aggregate single-slot floors in
section 16.3. Such CRTCs form one `HomogeneousCompletionGroup`; compatible ready
primary generations may improve throughput through the section 9.2.1 bundle
tier, but full refresh on every output is not promised. Different refresh
rationals, failure of the single-slot gate, or an unknown mode period keeps the
topology on the merged Phase A+B backend path. A hotplug/modeset that would leave
the group first quiesces the C.0 owner through the lifecycle barrier and
installs the baseline topology. A future independent **Multi-CRTC Parallel
Retirement** design may lift the ceiling after tracking complete DRM-object
conflicts; C.1 does not.

Physical commits/s and per-CRTC retired state generations/s are separate
metrics and may not be substituted for one another. C.1 remains ineligible
while more than one CRTC is active on the device. Concurrent physical atomic
commits still require a successor design that computes complete DRM-object
conflict sets; releasing the atomic slot at ioctl acceptance alone is
forbidden.

The owner tracks:

- last retired/displayed state per CRTC and plane;
- at most one `Submitting` or accepted nonblocking atomic commit pending for the
  device, plus at most one `CoordinateSubmitting` reservation on a cursor plane
  only in the concurrency class defined by sections 5 and 7.1;
- the pending commit id, device/topology generation, affected CRTC set, expected
  completion set, completions already observed, exact userspace-included DRM
  object/plane set, native-composition-contract result, and the separately named
  audited expansion-hazard prediction used to prove coordinate-overlap safety;
- the current final-validation `AtomicSnapshotId`, including device, lifecycle,
  topology, primary, cursor, gamma, connector and CRTC desired generations, and
  the exclusive validation lease that makes it usable by exactly one live
  installation;
- every pre-submit producer wait, every returned out-fence, and the separate
  presentation/release milestones in section 10.2;
- newest queued cursor intent per CRTC;
- the per-plane coordinate transport, its qualification generation and any
  newest owner-mediated coordinate intent;
- desired/pending/current gamma blob and generation per CRTC;
- primary-plane/direct/unflip intents that must not be reordered;
- cursor framebuffer references for current, pending, and queued generations;
- topology/seat generation used to reject stale work;
- the always-current `LifecycleEpochId`, optional current
  `LifecycleTransitionId`, its precedence/phase, and the process-isolated
  `KmsIoExecutor` IPC/watchdog/lease/result state;
- the bounded `LifecycleDesired` snapshot, contributing event ids, prerequisite,
  and exact terminal/nonterminal disposition required by `REC-5`.

### 9.1. Bounded primary intent model

The owner does not turn existing render/present traffic into a generic FIFO.
For each primary-plane ownership unit it holds at most one submitted state and
these bounded unsent categories:

- one composed desired state, represented by accumulated damage/scene
  generation rather than a queue of rendered frames;
- one primary-plane direct successor, latest-wins across every eligible
  authoritative-root direct intent for that plane, regardless of the Present
  async option bit;
- one unflip/recovery barrier, which supersedes incompatible unsent direct work
  but is never superseded by a later primary intent.

This does not weaken the merged core Present rule. The core may scrap a request
only when CRTC, effective target MSC, and coverage establish the same Present
equivalence class; `effective_target_msc=None` establishes no equivalence. The
KMS successor slot operates one layer lower: once a newer full-plane
authoritative-root intent replaces an older never-submitted intent for the same
physical primary plane, that older intent can no longer be displayed. This
plane-ownership fact applies to synchronized and async requests without
authorizing broader core supersession.

The direct eligibility predicate is therefore also a coverage invariant: every
intent admitted to the slot replaces the complete authoritative root on the
same output set. If a future path admits partial coverage, different output
ownership, or a request that cannot replace the complete plane state, it must
define a different bounded representation before becoming eligible; it may not
reuse this slot.

Replacing a never-submitted successor releases its source buffer, pins and
wakes immediately and emits `IdleNotify` exactly once. Its `Skip`
`CompleteNotify` is retained until the in-flight predecessor retires, then is
published after that predecessor's completion. Submitted primary state is
never scrapped. Topology invalidation completes or rejects every never-
submitted intent through the same split before dropping resources.

Phase C.1 inherits this slot; it does not introduce a second async-only queue.
It adds only async capability/admission, request shape and `PAGE_FLIP_ASYNC`
semantics to a surviving eligible successor.

### 9.2. Ordering classes

1. **Topology and ownership:** modeset, connector routing, disable, DPMS, VT
   suspend/resume, hotplug reconstruction.
2. **Primary replacement:** composed page flip, direct scanout, composed
   unflip.
3. **Cursor-only:** image, movement, animation, show/hide.
4. **Color-only:** RANDR gamma LUT replacement or identity restoration.

Topology changes invalidate queued KMS intents from earlier generations, while
their lifecycle request updates the bounded `LifecycleDesired` snapshot rather
than entering an event FIFO. Primary
replacement, cursor, and color state may be combined when ready, but cursor or
gamma must not wait indefinitely for future scene damage.

Topology/ownership work has priority but never overtakes a `Submitting` or
accepted commit. It first completes, cancels before executor dispatch, or
terminalizes/quarantines that record under section 10. During seat-active
operation, `ALLOW_MODESET` is combined with
`NONBLOCK` and submitted by the required section 4.1 executor path; result
handling is still serialized by the owner. A permitted offline blocking call acts as a
device barrier: ioctl success retires its submitted state according to section
6.3, while failure leaves the prior current state authoritative. Its executor
lease and transition tag follow `COMMIT-5`/`REC-4`.

### 9.2.1. Fair admission and starvation bound

The C.0 dispatch-timing policy is named
`DispatchTimingPolicy::ImmediateOnRetirement`: when retirement makes work
eligible, the owner runs admission in that wake and dispatches the selected work
without a retention timer. This is an explicit policy point rather than an
accidental consequence of the event handler. Phase C.2 may replace the policy
when it owns late/above-vblank cursor submission; C.0 fixes it to immediate
dispatch.

Each ready unsent maintenance identity `(CRTC, cursor|gamma)` owns at most one
desired payload and receives one device-monotonic `AdmissionTicket` immediately,
even when the device slot is free. Latest-wins replacement changes the payload
but preserves that ticket and its original age. A topology transition drops only
tickets whose generation cannot be remapped; surviving desired protocol state
retains its relative age.
Qualified `OwnerMediatedLegacyMove` coordinate intent instead uses the bounded
per-plane lane from section 7.1 and owns no maintenance ticket. It enters this
ticketed scheduler only after a native-contract fallback or transport closure
converts the newest coordinate to `SynchronousAtomicMove` or an ordered
software transition. Its one eligible
post-`EBUSY` retry runs at primary completion before the atomic tiers below,
except that a waiting lifecycle/topology barrier closes admission first.

Retirement-time promotion of the merged direct successor is a named admission
path, not an event-handler bypass. When a direct predecessor retires, the owner
first enqueues that predecessor's completion followed by its deferred successor
`Skip`s for ordered later publication, then runs the same admission function
used by every other wake. Core publishes the queued protocol events after the
retirement handler returns; owner admission and any immediate KMS submission
therefore occur before wire publication without changing the client-visible
predecessor-before-`Skip` order. With the immediate C.0 policy, that function
admits work in this order:

1. topology/ownership barrier already waiting;
2. unflip or software-cursor recovery needed to restore a visible/correct
   desktop;
3. a ready synchronous primary-plane direct successor only if its complete
   atomic closure absorbs every aged maintenance identity that would otherwise
   win and its CRTC is permitted by the primary round-robin rule;
4. the aged cursor/gamma identity with the oldest `AdmissionTicket`, with a
   stable `(CRTC, class)` tie-break;
5. one compatible homogeneous multi-CRTC primary bundle when at least two
   CRTCs in the qualified group have ready synchronous generations;
6. the oldest ready primary replacement, round-robin across CRTCs; the
   retirement successor participates here and is preferred when no different
   CRTC is owed the next turn;
7. the ready non-aged cursor/gamma identity with the oldest `AdmissionTicket`.

Tier 3 is the fairness-qualified version of Phase A+B's
submit-after-retirement contract. When no aged maintenance or owed primary CRTC
blocks it, `ImmediateOnRetirement` keeps the dispatch instant unchanged and
introduces no configurable margin. Tier 3 is legal only when the owner can
serialize the full persistent request, canonical completion evidence, and every
aged maintenance generation that precedes it. Compatible generations are
absorbed into that request. If an aged identity is incompatible or belongs to
another CRTC outside the qualified closure, tier 4 wins; if another primary CRTC
is owed the round-robin turn, tier 5 or tier 6 wins. The direct successor remains
the sole queued successor and is reconsidered immediately when the intervening
commit retires. Thus fairness may
add the bounded time of the maintenance/primary admissions already specified
below, but C.0 adds no arbitrary frame-retention budget. If a required cursor or
gamma generation for an affected CRTC cannot be absorbed because its snapshot
is stale or incompatible, the owner does not issue a plane-only successor. It
retains or terminalizes that successor under its existing direct/unflip rules
and services the correctness barrier first.

If an incompatible higher-priority item is admitted while a ready maintenance
identity remains unsent, that identity becomes aged without changing its
ticket. Thus maintenance is selected immediately when it is the only ready work,
but may lose at most one ordinary primary admission before entering the aged
tier.
Topology/unflip/recovery barriers also age surviving maintenance that they
overtake.

Every admitted synchronous request includes only compatible persistent cursor
or gamma generations that actually changed. An unchanged cursor plane is
omitted from the atomic request rather than copied defensively, and coordinate-
only intent is never absorbed. A tier-3 retirement successor must additionally
absorb every changed aged maintenance identity that would precede it; otherwise
it is ineligible for that tier. A retirement-promoted successor that absorbs a maintenance
generation consumes that generation's ticket exactly as a maintenance-selected
commit would; a continuously occupied direct-successor stream therefore cannot
starve compatible cursor or gamma work. Absorption is symmetric: when tier 4 or
tier 7 selects a maintenance identity and the same CRTC has a compatible ready primary
replacement, the owner combines the oldest such primary without overtaking an
unflip/topology barrier. Those generations retire with the combined request
regardless of which identity won admission. Latest-wins applies within a CRTC,
but does not change the CRTC's age or its place in device admission order.

Tier 5 takes the oldest ready synchronous primary generation for every ready
CRTC in the `HomogeneousCompletionGroup` and builds one atomic transaction. It
is eligible only when at least two CRTCs are ready, no lifecycle/unflip barrier
intervenes, every included CRTC has canonical completion coverage, and all
changed aged maintenance required by tiers 3–4 is either compatibly absorbed
or serviced first. It never carries unchanged cursor state or coordinate-only
intent. Each distinct included primary generation retires once from the same
physical commit; a CRTC not ready at construction is not represented by carried
state and earns no logical retirement. When fewer than two CRTCs are ready,
tier 6 preserves the existing round-robin singular path without waiting on a
timer for a bundle.

A C.1 async direct primary is never an absorption target and never absorbs
maintenance state. Its serialized request contains only the sole changed
primary plane plus its permitted ephemeral fence/event state; cursor, gamma,
connector, topology, and another CRTC remain separate non-async work. A ready
aged maintenance identity is selected before promoting such a future async
successor; after maintenance retires, the retained newest async successor is
submitted. This preserves bounded progress without creating a second queue or
converting a synchronous request to async.

A cursor or gamma intent is also aged when it arrives behind an already
submitted commit. After that commit completes, no primary that cannot absorb
the aged generation may take the device slot while its ticket exists. Admitting
or absorbing an identity consumes its ticket exactly once; a newer desired
update arriving while that identity is submitted receives a new ticket and
ages normally. If `N`
incompatible maintenance identities are aged, each is admitted after at most
the one commit already submitted when it aged, `N - 1` older-ticket maintenance
admissions, and owner dispatch latency. Finite topology/unflip/recovery barriers
may interrupt this bound and are measured separately, but cannot reset surviving
tickets.

Likewise, a continuously ready primary CRTC may not take two successive device
slots while another CRTC has a ready primary intent. C.1 uses the inherited per-CRTC
latest-wins slot but yields device admission under these rules. The bounded
completion/terminalization policy in section 10 prevents a stuck commit from
turning either guarantee into indefinite wait.

An async primary intent cannot absorb state from another CRTC when that would
create an unqualified multi-CRTC async transaction. The owner instead services
the other CRTC with a separate non-async commit before promoting the next async
primary. Telemetry and tests measure the maximum number of intervening
admissions for each class; a value above the bounds above is an invariant
failure.

### 9.3. Cursor and gamma progress on idle scenes

A cursor image, visibility, binding or synchronous-coordinate intent schedules
its own prompt atomic flush when no compatible primary commit is imminent. A
qualified coordinate-only legacy intent runs from the same owner as soon as its
cursor-plane reservation and lifecycle permit, including while an accepted
contract-preserving, no-hazard primary commit owns the atomic device slot.
Neither path is gated on `scene_wants_compose`, damage, or the next scene tick.

Gamma-only intent follows the same progress rule, while multiple unsubmitted
gamma updates coalesce to the newest complete LUT.

If a nonblocking commit is already pending, new motion overwrites the queued
cursor position. Image/show/hide changes replace older queued cursor state while
retaining any framebuffer still referenced by current or pending state. The
retirement/event wake immediately attempts the newest queued cursor intent.

The queue is bounded:

```text
per device: one submitted atomic commit
per cursor plane: at most one CoordinateSubmitting reservation
per CRTC: one latest desired cursor state + one latest desired gamma ramp
```

Here “submitted” includes the `Submitting` dispatch-to-reply interval.

Gamma has the same bounded desired-state rule; a newer unsent LUT replaces and
destroys the older unsent blob.

### 9.4. `EBUSY`

The owner never dispatches an atomic request while its own `Submitting` or
accepted record occupies the device slot. Consequently `EBUSY` cannot mean
“wait for our pending commit” and is not a normal scheduling signal. Atomic
`EBUSY` with no owner-tracked live record is an explicit pre-submit rejection
and a driver/ownership invariant failure: retain only the newest desired state,
close qualification/readiness, record the foreign/internal-busy evidence, and
enter the bounded topology/recovery path. There is no completion to wait for,
no immediate retry and no retry spin.

The qualified coordinate exception is narrower. `MOVECURSOR` may return
`EBUSY` when it is deliberately dispatched alongside the one accepted cursor-
overlap-safe primary commit. That rejection is benign and consumes no atomic
slot: the owner retains/coalesces the newest point and allows one retry after
that primary completes and before any later primary admission. Coordinate
`EBUSY` results are consecutive only when no successful coordinate return
intervenes; every success resets the count, while atomic completion alone does
not. A second consecutive coordinate `EBUSY` closes the plane's fast transport
for the incarnation and moves the newest point to ordered atomic or software
fallback. An `EBUSY` when no such compatible primary is live is an invariant/
qualification failure and closes the transport immediately. Neither
case poisons otherwise proven atomic completion, retries immediately, bypasses
the owner, or selects another legacy ioctl. Telemetry distinguishes eligible
concurrent rejection, retry success, retry exhaustion, and impossible-context
`EBUSY`.

An internal driver `atomic_async_check()` result is not classified here as a
userspace `EINVAL`: the atomic helper may consume it by selecting the ordinary
blocking commit path. Section 7.1 prevents known-ineligible calls before
dispatch; any coordinate call that nevertheless returns above its latency bound
is a coordinate-policy/cohort defect and closes the fast transport for the plane
incarnation. A real userspace-visible `EINVAL` after the owner proved a
coordinate-only request is likewise a request/driver contradiction and closes
the transport; neither result creates an automatic rehabilitation path.

## 10. Commit lifecycle, completion, and recovery

Every live commit receives a monotonic device-generation-local `CommitId` for
logical owner/executor correlation and a distinct incarnation-monotonic
`EventToken: NonZeroU64` for kernel-event correlation. Sequence arms use fresh
tokens from that same namespace and resolve to a typed
`SequenceArm { hardware_crtc, clock_epoch, purpose, target }` record rather than
a commit. The owner allocates tokens in increasing order starting at one and
never wraps, resets, or reuses one while any alias of that incarnation exists.
Allocation uses checked increment plus a debug assertion that exhaustion is
unreachable within a process lifetime; C.0 specifies no administrative restart,
incarnation replacement or production recovery machinery for `u64` exhaustion.
C.0 extends or wraps the atomic ioctl submission layer so
`drm_mode_atomic.user_data` carries the commit's `EventToken` verbatim and the
parsed page-flip event exposes it. The current
`drm::Device::atomic_commit` wrapper, which leaves user data zero, is not used
for live owner submissions unless extended equivalently. The ABI boundary uses
fixed-width DRM UAPI fields and a request type correct for each supported
target. In particular C.0 removes or replaces the existing `IoctlReq`
definition backed by `libc::Ioctl` on Linux and `libc::c_ulong` elsewhere;
merely leaving that pre-existing alias in place does not satisfy this
requirement. The replacement may use target-specific typing internally, but
one reviewed wrapper owns it and Linux glibc, Linux musl, and FreeBSD compile
tests prove the three actual function signatures rather than assuming one
request type is universal.

Before executor dispatch, the owner installs the pending record with the exact
`AtomicCrtcClosure`, `ExpectedCompletionCrtcs`, `KernelEventCrtcs`,
`PresentEventCrtcs`, `CommitId`, `EventToken`, cursor framebuffer, gamma blob,
primary framebuffer, topology generations, stable out-fence holder
specification, and both possible old/new ownership ledgers. It is initially
`Submitting`; none of those resources is classified as never-submitted again
unless an explicit rejection arrives. A
page-flip completion first resolves the non-zero wire
`(device_incarnation, EventToken)` to exactly one live or tombstoned record and
then matches its `KernelEventCrtc`. The resolved record supplies, but the event
does not infer, `device_generation`, `LifecycleEpochId`, and `CommitId`. The owner
also keeps the last 64 identity-only tombstones for terminalized commits;
tombstones retain kernel-event, Present-event, and observed CRTC sets plus
terminal state, but own no KMS resource. The ring is cleared after a proven
event-queue drain or incarnation close; eviction merely changes a very old
duplicate from `tombstoned` to `unknown`, which has the same telemetry-only
behavior. Eviction is safe because an `EventToken` is never reused within the
incarnation; it cannot make an old event resolve to a newer commit.

Event correlation is a structural requirement, not a kernel-version guess.
During initial domain construction the device must report
`DRM_CAP_CRTC_IN_VBLANK_EVENT=1` and `DRM_CAP_TIMESTAMP_MONOTONIC=1`, and the raw
event parser must preserve `drm_event_vblank.user_data`, `crtc_id`, `sequence`,
`tv_sec`, and `tv_usec` independently. It may not use a compatibility parser
that substitutes `user_data` for a missing CRTC id. Because all DRM event types
share one byte stream, C.0 replaces the baseline `receive_events()` drain as a
whole; it does not race a second reader or raw-parse only selected events. The
executor never reads the DRM event fd: drain is owner-exclusive for the
incarnation even though the helper holds a registered alias of the same open
file description. A single helper-side read — a debug drain, a flush before
termination, a diagnostic added later — would consume completion events the
owner then never sees, and would surface as a missing out-fence or an expired
completion deadline rather than as a lost read. One
buffered parser validates every header length before advancing and decodes
`DRM_EVENT_VBLANK`, `DRM_EVENT_FLIP_COMPLETE`, and
`DRM_EVENT_CRTC_SEQUENCE` into typed records while retaining their raw fields.
Unknown well-formed event types are skipped by their validated declared length;
zero, undersized, over-buffer, truncated, or overflowing lengths are malformed
input rather than an excuse for an out-of-bounds or non-progressing parse. A
malformed record prevents initial qualification or, on a qualified incarnation,
immediately poisons the event/completion mechanism and follows normal teardown
and recovery. The baseline manual sequence parser and the crate compatibility
`Event::PageFlip` path are both removed from production drain ownership. Each
later incarnation requeries both caps before qualification; zero or query
failure closes qualification/readiness without rewriting the cached advertised
bit. A returned
zero `crtc_id` for the current pending `EventToken`, despite successful discovery,
is direct completion-mechanism contradiction and poisons the incarnation
immediately.

Before admitting an event-bearing commit on a newly installed active hardware
CRTC or clock epoch, the owner serializes one `DRM_IOCTL_CRTC_GET_SEQUENCE` clock
probe through the required section 4.1 executor path. A current successful result selects
`KernelSequence` and supplies its `u64` sequence as the trusted reference.
`EOPNOTSUPP`, any other explicit errno, or a malformed current result terminalizes
the probe as a qualification failure, leaves the clock source unresolved, and
permits no same-epoch retry loop. C.0 does not synthesize a software protocol
clock; a future independently validated design may add that support. Timeout,
IPC loss, or executor failure follows
`COMMIT-5`/`ExecutorStalled`; a stale result is discarded and the winning current
lifecycle transition owns any replacement probe. No event-bearing commit is
admitted until device replacement, VT reacquire, administrative reprobe,
identity-changing hotplug, or a genuinely new CRTC clock epoch obtains a current
result. None of these outcomes is converted silently into software mode. The
probe decision is per `(device incarnation, hardware CRTC, CRTC clock epoch)`,
immutable within the epoch, and based on ioctl behavior rather than a driver
allowlist.

The owner stores that decision directly in the epoch-local CRTC clock record as
`Unresolved` or `KernelSequence`; there is no separate
device-keyed unsupported cache. Reopen begins with a new incarnation and
`Unresolved` records, and a genuinely new CRTC clock epoch does the same even
when the physical device and raw CRTC handle are unchanged. Thus the merged
baseline's process-lifetime
`crtc_queue_sequence_unsupported_devices: HashSet<DrmDeviceKey>` and all of its
read/write/construction sites are removed rather than renamed or consulted as a
hint.

This qualification boundary follows Linux vblank-core behavior rather than a
driver name. A device without `drm_vblank_init()` makes GET/QUEUE_SEQUENCE
return `EOPNOTSUPP`; the generic page-event sender also publishes sequence zero.
Those outcomes share `drm_dev_has_vblank()` and are not independent evidence.
Such a cohort remains structurally incapable in C.0 even if deterministic tests
can model a future software clock.

Every permitted `DRM_IOCTL_CRTC_QUEUE_SEQUENCE` request in `KernelSequence`
allocates a fresh `EventToken` and places it in `user_data`; raw CRTC ids are not
event identities. A returned `DRM_EVENT_CRTC_SEQUENCE` may update a clock only if
its non-zero token resolves to a live `SequenceArm` whose incarnation, hardware
CRTC, clock epoch, purpose, and target are all current and whose event type is
sequence. The event terminalizes that arm exactly once. Unknown, duplicate,
cancelled, old-epoch, or wrong-typed tokens are telemetry-only and cannot update
a clock or satisfy a Present target; an active token delivered with the wrong
event type is a completion-mechanism contradiction and poisons the incarnation.
Epoch invalidation cancels logical arms and removes them from bounded active
storage, but token non-reuse ensures any delayed kernel event remains unknown
rather than matching new work.

This conversion explicitly includes both sequence producers in the merged
base: the relative idle arm exposed through the backend trait/run loop and the
per-target absolute arm currently represented by `absolute_vblank_targets` and
`absolute_seq_user_data`. The raw-CRTC encodings, including a possible zero
relative token and the reused high-bit absolute tag, are removed. A relative
arm becomes `SequenceArm { purpose: IdleClockWake, target }`; an absolute arm
becomes `SequenceArm { purpose: PresentTargetWake, target }`. Multiple parked
Presents for the same CRTC/epoch/target share one arm but retain a bounded set
of logical consumers outside the token identity.

If a parked Present is scrapped in core, displaced from the primary-plane
successor slot, terminalized as `Skip`, or invalidated by topology/lifecycle,
its consumer is removed immediately. An arm with no remaining consumers is
logically cancelled and tombstoned even if the kernel request cannot be
withdrawn; a delayed event is telemetry-only. A CRTC without a usable queue-
sequence path cannot qualify for C.0 and therefore creates no software timer or
synthetic protocol-clock sample.

A fresh `PresentTargetWake` event may advance the general and completion clock
while a page flip is in flight, preserving the merged Phase A behavior. Its
typed milestone is only `ClockSample`/`PresentDueWake`: it may make a parked
request due and may release a completion whose target clock condition is now
proven, but it never establishes `Accepted`, `HardwareComplete`, `Presented`,
or `PriorBufferReleased` for any atomic commit.

For each `(device incarnation, device generation, hardware CRTC, CRTC clock
epoch)`, the owner then normalizes the page-event clock before it can become
`Presented` or update the general CRTC clock:

1. Require `tv_usec < 1_000_000` and compute
   `UST = checked(tv_sec * 1_000_000 + tv_usec)` in monotonic microseconds.
2. In `KernelSequence`, extend the raw `u32 sequence` into the CRTC's `u64` MSC
   domain. Given the trusted `u64` reference, choose the unique non-negative
   value congruent to `sequence (mod 2^32)` whose modular distance from the
   reference is strictly less than `2^31`; an exact half-range tie or lack of a
   non-negative representative is invalid. Later trusted
   exactly correlated `DRM_EVENT_CRTC_SEQUENCE` samples may advance the
   reference. A raw zero in this mode is ordinary sequence data, including a
   legitimate `u32` wrap, and never switches the mode.
3. Apply the existing wrap-safe Present comparison. A valid late sample may be
   classified and drained but cannot move either CRTC clock backwards. A
   matching active Present event whose normalized sample contradicts its
   current clock epoch enters `CompletionUnknown` rather than inventing MSC/UST.

A lifecycle/topology operation that resets or replaces a hardware CRTC clock
increments the CRTC clock epoch and discards its kernel extension reference
before new events are admitted. The new epoch must be probed
again; a prior unsupported result cannot be inherited across incarnation or
epoch. Existing per-window/protocol MSC offset handling preserves the
client-visible domain across a legitimate CRTC selection change; raw event
sequence values are never used as already-extended protocol MSCs.

Events are classified normatively:

- an exact `Submitting` or accepted pending match records that kernel-event CRTC
  as observed. If it is also in `PresentEventCrtcs`, it stages `Presented`; that
  milestone becomes protocol-authoritative only after explicit ioctl success.
  A non-consumer event is drained/accounted and may update the general CRTC clock
  under the selected source mode, but never creates `Presented`. An explicit
  rejection paired with either kind is contradictory active evidence, enters
  `CompletionUnknown`, and never reports a Flip;
- zero or unknown `user_data`/`EventToken`, old-incarnation events,
  tombstoned events, and a duplicate for an already-observed CRTC are
  telemetry-only and cannot advance or retire any record; duplicates
  additionally raise an invariant warning. This does not include a zero
  `crtc_id` paired with the current pending `EventToken`, which is the structural
  contradiction defined above;
- the current pending `EventToken` paired with a CRTC outside its
  `KernelEventCrtcs`,
  contradictory identities for the same pending tuple, or an event payload that
  cannot be parsed safely is direct evidence that active correlation is
  unreliable and immediately enters `CompletionUnknown` with an incarnation
  poison;
- absence of an otherwise valid required `PresentEventCrtcs` event becomes
  `CompletionUnknown` only when its primary-event deadline expires. Missing
  non-consumer kernel events do not delay protocol or resource completion.

The single device slot is reserved before IPC dispatch and remains a conflict-
serialization rule, not a substitute for event identity. An unrelated event
cannot poison a healthy incarnation, but an explicit contradiction of its active
commit can. The coordinate-plane exception does not release or duplicate this
slot: it may overlap only the completion interval of the recorded cursor-
disjoint primary class and excludes every new KMS host call while its own result
is unresolved.

Every nonblocking commit has a canonical hardware-completion out-fence for each
member of the section 6.3 `ExpectedCompletionCrtcs` atomic closure. This set
includes enable, disable, modeset, both powered endpoints of a plane/connector
move, and every primary/cursor/color update bound to such a CRTC; it is never
empty merely because a disable makes `new.active=false`. A request whose exact
closure affects no old-or-new active CRTC may have an empty set and must not
manufacture completion evidence. C.0 structural capability therefore requires
usable `OUT_FENCE_PTR` coverage for every CRTC that can enter this set.

The owner allocates stable holder storage for the ioctl and initializes every
`s32` holder to `-1`. A successful live ioctl must replace every expected holder
with a non-negative sync-file fd; `-1` is valid only after a rejected ioctl or
`TEST_ONLY`, never as an already-signalled live completion. Live success plus
`-1`, a non-sync-file fd, partial output, poll error, or deadline expiry latches
the mechanism failed for that device incarnation and enters
`CompletionUnknown`.

A device incarnation identifies one opened DRM file description plus every fd
alias and helper lease referring to it, represented by an owner-tracked
`IncarnationFdSet`. All parent-side duplication and helper inheritance must go
through this registry; untracked KMS-fd duplication is an invariant violation.
Creating an alias acquires a lease before `dup`/fork, and closing it or reaping
its helper releases that lease exactly once. A topology epoch does not create a
new incarnation.

The completion-mechanism poison clears only after admission and new alias
creation stop, every submitter/event reader is detached, every helper holding a
lease is cancelled and reaped, and every fd in the old `IncarnationFdSet` is
closed. Closing only the owner's fd is insufficient. A new open file description
may be numbered as a fresh incarnation only after the old lease count reaches
zero; hotplug or `RRSetCrtcConfig` cannot clear the poison.

Valid fds are registered with the event loop. Readability is only a wakeup: the
owner queries canonical sync-file status (for example `SYNC_IOC_FILE_INFO`) and
counts only successful signalled status toward `HardwareComplete`. Pending
status remains registered. Negative/error status, inability to query a claimed
sync file, poll error, or a mixed success/error multi-CRTC set enters
`CompletionUnknown`; a signalled error never promotes state or releases a
resource. The owner never silently switches to an uncorrelated event heuristic.

Primary Presents additionally retain `PAGE_FLIP_EVENT` with `EventToken` for
Present MSC/UST. A primary update with no Present consumer, cursor-only, and
gamma-only commits do not depend on a page-flip event for lifetime progress.
Multi-CRTC state retires only after its complete expected out-fence set reports
successful signalled status. Page events and fences are separate typed
milestones; receiving one never fabricates the other.

An out-fence proves the CRTC flip/scanout milestone, not completion of every
device-level shutdown or cleanup operation. A disable/topology transition that
must prove full hardware teardown before releasing device-level resources uses a
blocking owner barrier or a later fd/device teardown barrier; it cannot promote
out-fence `HardwareComplete` into universal `hw_done` proof.

A dispatched transaction reaches exactly one terminal state:

- `Completed`: the evidence required by the authoritative section 6.3 row for
  this commit class has arrived. Hardware state may become current before this
  terminal point, while buffer release still obeys its separate section 10.2
  milestone;
- `FailedBeforeSubmit`: cancellation happened before IPC dispatch or an explicit
  ioctl result proved rejection. No submitted state becomes current,
  unreferenced new resources are released, any staged event is diagnostic only,
  and the
  classified result is retry, topology-scoped qualification rejection, or
  readiness closure. It never directly changes previously advertised
  capability; only construction of a changed client-visible protocol domain may
  recompute that value under section 6.2;
- `CompletionUnknown`: ownership/event delivery was lost after successful
  submission or acceptance could not be disproved after dispatch, so neither old
  nor new resources are proven unreferenced.

Before an orderly VT release, topology rebuild, DPMS teardown, or shutdown, the
owner stops admission and drains `CoordinateSubmitting`, `Submitting`, or the
accepted pending record without blocking the X11 core indefinitely. A bounded timeout, executor/IPC
loss, or device/master loss moves it to `CompletionUnknown`. An executor still
inside the host call additionally enters `ExecutorStalled` after the watchdog,
without delaying prompt protocol/seat handling. Resources from an unknown
transaction remain quarantined until
the complete old `IncarnationFdSet` is closed or another kernel-proven teardown
barrier makes both old and new state unreachable. No new commit on a reused CRTC
handle may be matched against undrained events from the old device generation;
the fd event queue is drained before reuse, or the device is reopened with a new
generation.

`EACCES`, `ENOENT`/removed objects, `EINVAL`, and device loss are not treated as
`EBUSY`. They cancel queued work from the invalid generation, preserve the
latest protocol-visible desired cursor/gamma state for reconstruction, and
either rebuild topology or revoke readiness. They never spin, silently fall
back to a legacy ioctl, or report an unproven state as current.

Failure latches have distinct scope:

- invalid/missing/error out-fence, completion deadline, an explicit active-event
  contradiction defined above, an unparseable event payload, or another
  completion-mechanism breach poisons the entire device incarnation and requires
  complete fd-set retirement plus a fresh open file description;
- `EINVAL`/`EOPNOTSUPP` proven attributable to one object combination or async
  topology latches only that topology generation;
- a topology generation change never clears an incarnation poison, while a new
  incarnation never reuses object handles or completion records from the old
  one.

Incarnation poison immediately stops all live KMS submission on that fd,
including primary/composed frames that omit the failing cursor or gamma state.
The core may remain responsive and rendering may continue off-screen, but no
new scanout/Present state is accepted until recovery installs and qualifies a
fresh incarnation. This prevents an untrusted completion mechanism from growing
the quarantine.

Recovery distinguishes two output actions:

- **hardware disable** is an atomic KMS transition and is allowed only through a
  healthy incarnation whose required completion/teardown evidence can be proven;
- **logical withdrawal** removes the affected outputs/CRTCs from the active
  RANDR/backend model, closes readiness, terminalizes protocol work, and makes no
  claim that physical scanout changed. It issues no KMS mutation on a poisoned,
  unavailable, or unqualified incarnation. Hardware/resource quarantine remains
  until device removal, shutdown's proven teardown order, or a later healthy
  incarnation proves replacement/disable.

Each completion-loss incident permits exactly one automatic recovery attempt,
but that attempt cannot open a replacement fd while an `ExecutorStalled` lease
survives. Reap resumes the same `RecoveryId` under whichever lifecycle
transition/epoch is then current according to `REC-6`; it does not create a
second incident or id.
The attempt owns a `RecoveryId` and cannot recursively start another attempt. If
closing the complete old fd set, reopen, discovery, final `TEST_ONLY`, live
install, or qualification fails or becomes `CompletionUnknown`, the device
enters `RecoveryFailed`: admission remains stopped and its outputs are logically
withdrawn. No timer or queued intent retries it. Only a new external boundary—an
actual hotplug identity change, VT reacquire, explicit administrative reprobe, or
server restart—may create one new recovery attempt with a fresh `RecoveryId`.
DPMS toggles and ordinary client traffic are not retry authority.

All rows below are inputs to the single `REC-4` arbiter and `REC-5` desired-state
snapshot, not independently callable handlers. A winning higher-priority row
absorbs cleanup from the row it supersedes. At most one transition may cancel
helpers, retire an fd family, open a replacement, or publish logical withdrawal
for a device. Completion of a row is not lifecycle convergence until every
contributing event has a terminal disposition or a truthful external-prerequisite
`Deferred` disposition.

The transition that encountered `CompletionUnknown` continues according to
this table:

| Trigger | Required action | Desired-state fate | Submission restart |
| --- | --- | --- | --- |
| Completion failure during otherwise normal live operation | Immediately stop admission and fd-alias creation, terminalize affected protocol work, detach event readers/submitters, quarantine both possible KMS/resource states, request executor/helper termination, and reap every lease. If reap is delayed by an in-kernel host call, enter `ExecutorStalled` and logically withdraw now; after reap, close the complete `IncarnationFdSet`, then resume the same incident's sole automatic reopen/discovery/final-`TEST_ONLY`/real-install qualification attempt. This recovery is owner-initiated and does not wait for VT, DPMS, or hotplug. Any later stage failure, including unknown qualification completion, enters `RecoveryFailed`, logically withdraws the outputs, and reports normal device-loss/RANDR failure without attempting a KMS disable. | Preserve newest device-independent cursor/gamma and remappable primary protocol state; discard stale hardware handles and generations only after the executor lease is gone. | Only after the sole fresh real install/restore commit reaches `Completed` with qualification evidence. After `RecoveryFailed`, only an authorized external boundary may create a new attempt. |
| VT release | Release the seat immediately; do not wait past the drain deadline or executor watchdog. Stop alias creation, request executor/helper termination, record the `REC-6` recovery invalidation, and retain any unreaped executor plus complete old fd set in `ExecutorStalled`; reap/close asynchronously and keep resources quarantined under the applicable barriers. | Preserve cursor/gamma protocol state in device-independent memory. | Only after `VTAcquire` and complete old-lease retirement permit a fresh KMS incarnation and qualified installation. |
| DPMS | Revoke operational readiness and apply the newest global protocol level projected to each current stable output. If the incarnation is healthy, one ordered per-device hardware transition may be used. If it is poisoned, issue no KMS mutation; logical power state changes as permitted and the same recovery incident is deferred/resumed exactly as `REC-6` specifies. | Preserve the newest global level/epoch, current per-output projections, and the same recovery budget; drop stale hardware generations only after their barrier. | DPMS-on resumes an existing paused attempt but never creates one or revives `RecoveryFailed`. |
| Same-identity `TopologyRebuild` | Abort the in-place install, revoke readiness, rediscover rather than replay payload, and transfer the same recovery incident/quarantine when one exists. Retire a poisoned fd family before replacement. | Preserve only newest valid intents remapped to the discovered topology. | After fresh discovery, exact final `TEST_ONLY`, and real installation qualify; failure applies the same `RecoveryId` outcome from `REC-6`. |
| `DeviceAddedOrReplaced`, `VTAcquire`, `AdministrativeReprobe`, or `IdentityChangingHotplug` | Wait for the old fd-family barrier, invalidate any old incident as specified by `REC-6`, rediscover, and allocate at most one fresh authorized `RecoveryId` before attempting final `TEST_ONLY`/real install when recovery is required. | Preserve only device-independent state valid for the new identity/domain and newest desired snapshot. | After qualified real installation; failure/unknown reaches `RecoveryFailed` for the fresh id. |
| Device removal/loss | Stop alias creation, request executor/helper termination, record `Invalidated(DeviceRemoved)` for any recovery, logically withdraw outputs immediately, and reap/close the complete fd set asynchronously. An unreaped host call remains `ExecutorStalled`; it retains hardware handles and quarantine until reap rather than delaying device-loss notification. | Preserve protocol cache only for identities that may reappear; retain old hardware handles solely inside quarantine until their lease barrier. | Only on `DeviceAddedOrReplaced` after the old executor/fd-family retirement barrier. |
| Shutdown | Stop admission and alias creation, record `Invalidated(Shutdown)` for every recovery, request termination of all executors/helpers, terminalize protocol work, release the seat, and enter the teardown supervisor. Close the complete DRM fd set only after every child has returned a wait/reap status; then drop exclusively file-owned quarantine and tear down Vulkan/GBM/shared owners in their established device-loss-safe order. An uninterruptible executor enters `ShutdownExecutorStalled`: logical shutdown remains complete and the parent process, supervisor, aliases, leases, and quarantine stay alive until the teardown deadline, after which the parent exits, records the unreaped lease, and leaves the helper orphaned holding the device lock. Signals, IPC EOF, and watchdog expiry are not reap proof, and a bounded exit never claims the lease was released. | No retry; protocol caches may be dropped only after the applicable wait/fd/device/shared-resource barriers. | Process exit after every executor/helper is reaped and teardown order completes, or at the teardown deadline with the unreaped lease recorded and the device lock left held. |

Closing the complete `IncarnationFdSet` is a barrier only for resources owned
exclusively by that DRM open file description after the backend has stopped every
submitter and detached the generation from event dispatch. It is not proof that
unrelated dma-buf, Vulkan, GBM, or shared objects are unused. Quarantine records
own every such userspace reference until the table's device-specific teardown
order proves it releasable. C.0's implementation plan must inventory those
owners rather than treating one raw fd close as a universal lifetime proof.

### 10.1. Completion qualification without synthetic probes

Structural completion support is determined from atomic capability plus
presence of `OUT_FENCE_PTR` on every affected CRTC, never from a driver-name
allowlist. C.0 performs no synthetic primary flip, transparent-cursor
attach/detach, or gamma replacement at cold start or after clients are exposed.
Those operations are not generally visual- or ownership-neutral.

For the initial device incarnation and every later reopen/topology installation,
the first required real install/restore commit whose
`ExpectedCompletionCrtcs` is non-empty is the qualification commit. Whether its
ioctl is blocking or nonblocking, the live request includes stable
`OUT_FENCE_PTR` storage for that entire set and qualification requires every
returned fd to report successful signalled status. A blocking ioctl's successful
return proves its state transition but does not, by itself, qualify the
completion mechanism. A nonblocking qualification commit additionally follows
the normal tagged-event requirements when it is a primary Present.

Operational readiness remains closed until that exact client/server-desired
commit reaches `Completed` with the complete qualification fence evidence. A
topology with an empty expected set has no vacuous qualification: readiness stays false
until the first real activation commit qualifies it. No extra FB, cursor, or
gamma transition is inserted. Thus the initial path has the same explicit gate
as VT resume and hotplug; qualification is never optional. The device-local C.0
owner may submit this qualification commit while C.1 readiness is false; only
C.1 admission consumes the ready gate.

Normal cursor-, color-, primary-, and combined live commits provide continuing
evidence. Any completion-mechanism breach immediately closes readiness and
poisons the device incarnation under section 10. A topology-specific atomic
rejection uses only its topology latch.

Transitions to off are explicit qualification cases, not assumed equivalent to
an active flip. Because `ExpectedCompletionCrtcs` includes an old-active CRTC, a
DPMS-off, VT all-off or disable request must return and signal its canonical
out-fence even though no later physical vblank is expected. Hardware validation
must prove each supported driver reaches its `commit_hw_done`/fake-vblank or
equivalent completion path for these requests. Missing or failed off-transition
fence evidence still enters `CompletionUnknown`; the design does not weaken
truthful poison to hide a driver defect. The zero-poison soak in section 16.3 is
therefore a release gate, not optional characterization.

Rediscovery on the same fd cannot clear
an incarnation poison, and a reopened incarnation must qualify through its own
real install/restore commit.

### 10.2. Synchronization ownership and retirement milestones

The owner preserves the existing explicit-synchronization contract while
centralizing the ioctl. A commit record distinguishes:

1. `ProducerReady`: every source-specific acquire dependency completed
   successfully in the asynchronous pre-submit wait; this milestone is not a
   pre-accept requirement for the C.1 async-direct class;
2. `Dispatched`: the `Submitting` record and event identity are installed, the
   device slot is reserved, resources are uncertainty-owned, and IPC was sent;
3. `Accepted`: atomic ioctl returned success; ordinary C.0 classes handed no
   unresolved producer fence to KMS, while the C.1 async-direct class
   transferred exactly one under `COMMIT-4`;
4. `HardwareComplete`: the commit class's section 6.3 evidence arrived—canonical
   successful out-fence status for ordinary classes, or the sole correlated
   page event for C.1 async direct;
5. `Presented`: the matching `(device incarnation, EventToken, CRTC)` page-flip
   event resolved to the record's generation, lifecycle epoch, and `CommitId`
   and supplied MSC/UST for a primary Present;
6. `PriorBufferReleased`: the replacement's release dependency proves the
   previous scanout buffer is no longer used and its existing Vulkan/FOREIGN
   ownership-return rules have completed.

Nonblocking non-Present primary/cursor/gamma work normally needs `Accepted` and
`HardwareComplete`; it cannot manufacture Present completion. A primary Present
needs both
`HardwareComplete` for KMS-state/resource transition and `Presented` for
protocol completion and MSC/UST. Either may arrive first and is recorded until
the other arrives. For C.1 async direct, the same validated page event
establishes the two independently typed milestones; code records both and may
not infer either from ioctl acceptance. The device submission slot remains occupied until all
milestones required for `Completed` arrive, so at most one page-event-bearing
commit per device awaits correlation. A page event never closes an out-fence,
and an out-fence never completes or timestamps Present. `PriorBufferReleased`
may occur later and remains in the existing bounded BO/resource retirement
ledger; it does not keep the device submission slot occupied.
An accepted coordinate-overlap-safe primary may coexist with one coordinate-
plane reservation during this interval; the reservation is not a second atomic
slot and cannot authorize another atomic transaction.

Ordinary C.0 resource ownership follows this table:

| Stage | Producer/acquire fence | `OUT_FENCE_PTR` result | FB/blob/BO and external ownership |
| --- | --- | --- | --- |
| Built/waiting, before ioctl | The source-wait owner holds each fd/reference exactly once and polls without blocking the core. No device slot or live sync property exists yet. | None. | Intent owns new resources; no KMS or Vulkan-to-FOREIGN transfer for this commit has occurred. |
| Producer success | Close/release the waited fence exactly once and mark `ProducerReady`; build the live request with `IN_FENCE_FD` omitted or `-1`. | Holder storage is live and initialized to `-1`. | Intent becomes eligible for device admission. |
| Producer error/timeout/cancel | Close/release the wait exactly once, complete/reject the never-submitted intent under its source/Present rules, and never call atomic commit. | None. | Resources are never KMS-owned and follow local never-submitted cleanup. |
| `TEST_ONLY` | Uses no live sync fd/pointer; it validates only persistent state under section 5. | None. | No ownership transfer is inferred from the test. |
| `Submitting` after IPC dispatch | No producer fd remains. Cancellation can no longer classify the request as never-submitted. | Helper-local holder storage remains live; no returned fd is assumed absent or valid until the typed reply. | The record uncertainty-owns every possible old/new KMS, framebuffer, blob, BO, pin, descriptor, and external-ownership state and occupies the sole device slot. Matching page events are staged. |
| Ioctl rejected | No producer fd remains and KMS acquired none. Diagnose any defensively unexpected non-negative output and close it exactly once. | No valid completion fence is adopted. | New KMS state is not current. Copied/direct BO state follows atomic-rejected recovery, including `ReleasedButAtomicRejected`; it is not reset as though KMS returned FOREIGN ownership. |
| Ioctl accepted | No producer fd remains in userspace or KMS for this request. | Owner must adopt one non-negative sync-file fd per CRTC in `ExpectedCompletionCrtcs`. Any holder still `-1` is missing completion evidence and enters `CompletionUnknown`; it is not success. | Pending record owns all possible old/new scanout, cursor, gamma, pin, descriptor, and external-ownership references. |
| Executor/IPC/watchdog unknown | No producer fd remains. | No missing fd is interpreted as rejection or completion; any later delivered fd is adopted/closed exactly once into quarantine. | Terminalize protocol work once and retain the entire `Submitting` record as `CompletionUnknown`; `ExecutorStalled` additionally retains the alias/lease until reap. |
| Hardware complete | No producer-fence ownership remains. | Query successful sync-file status, then close each adopted out-fence exactly once. | New KMS state may become current; old resources release only when the class-specific replacement/FOREIGN rules also allow it. |
| `CompletionUnknown` | No producer-fd ownership remains because every producer completed before submit. | Quarantine every adopted unsignalled/error fd with its record until teardown. | Quarantine both possible state/resource sets and all external ownership ledgers under section 10. |

The C.1 async-direct row overrides only the producer/out-fence cells above. Its
intent owns one acquire fd before dispatch; the live ioctl transfers the kernel
reference and userspace closes its local fd exactly once on either return. It
allocates no out-fence holder and adopts no out-fence fd. After acceptance, its
record retains both possible primary states until the correlated page event;
missing or invalid event evidence enters the same `CompletionUnknown`
quarantine. All framebuffer, pin, external-ownership, executor uncertainty, and
exact-once cleanup rules remain unchanged.

The executor owns stable `OUT_FENCE_PTR` holder memory until the ioctl has
returned and transfers one terminal reply plus every resulting fd in one
message-boundary-preserving IPC operation before its success reply is complete.
A truncated reply, missing fd, ancillary-data truncation, sequence mismatch, or
channel loss is acceptance-unknown, never an inferred rejection. The executor
closes its local returned-fd copies exactly once after successful transfer or on
failure/exit. The owner-side `Submitting` record already owns the
uncertainty ledger and event identity throughout that interval. Queued
intents may own pre-submit producer waits but never create an out-fence or occupy
the device slot until `ProducerReady`, so coalescing cannot leak or reuse fd
payloads. Unit tests exercise expected reject/`TEST_ONLY` `-1`, invalid
live-success `-1`, accept, reject, partial multi-CRTC output, poll error, unknown
completion, and every exact-once close/transfer edge.

### 10.3. Monotonic completion deadlines

All deadlines use `CLOCK_MONOTONIC`/`Instant`; wall-clock changes are irrelevant.
The owner separates four timers:

1. **Producer/acquire:** runs entirely before device admission and inherits the
   existing source-specific policy for dma-buf implicit readiness, imported
   syncobj, or Present acquire wait. C.0 introduces no universal 200 ms live
   timeout. Readability is followed by the source's canonical success/error
   query; error, cancellation, or that source policy's timeout terminates the
   never-submitted intent locally.
2. **Host-call watchdog:** starts when the owner sends executor IPC. It is 2
   seconds for seat-active `NONBLOCK` work and 30 seconds for a permitted
   cold-start/final-offline blocking ioctl. Expiry follows `COMMIT-6`; it is not
   evidence that the kernel rejected the request.
3. **Hardware completion:** starts immediately after the producer-ready atomic
   ioctl is accepted. Primary, cursor and gamma-only work uses
   `FastHardwareCompletionDeadline =
   clamp(3 * slowest_affected_mode_period, 100 ms, 2 s)`; an unknown mode uses
   16.667 ms before clamping. Modeset, topology install and seat-active recovery
   instead use the release cohort's audited/measured
   `LifecycleCompletionObservedMax` and
   `LifecycleHardwareCompletionDeadline =
   min(30 s, max(10 s, LifecycleCompletionObservedMax + 2 s))`. Missing evidence,
   an unrepresentable calculation, or an observed healthy completion above the
   28-second representable margin leaves that cohort unvalidated rather than
   poisoning it under the fast-update timer.
4. **Primary Present event:** after `HardwareComplete`, create one timer for each
   required Present CRTC whose event has not already arrived:
   `deadline[crtc] = hardware_complete_observed_at +
   clamp(2 * mode_period[crtc], 50 ms, 500 ms)`. An unknown mode uses 16.667 ms.
   Expiry of any required CRTC timer enters `CompletionUnknown`; an event that
   arrived before `HardwareComplete` needs no timer. Commits without a Present
   consumer have no primary-event timer.

For mainline atomic helpers, `OUT_FENCE_PTR` and the page event share one
`drm_pending_vblank_event`; the out-fence is `event->base.fence` and both are
signalled from the same kernel event path. `HardwareComplete` and `Presented`
remain independently typed because userspace observes different fds/streams and
must never infer one from the other, but temporal reordering is defensive
wakeup bookkeeping, not an expected independent hardware sequence. Timer 4
therefore detects a lost or malformed userspace event after the shared kernel
signal path, not ordinary presentation latency.

Because no unresolved producer fence crosses the ioctl, producer timeout is
always genuinely never-submitted and cannot occupy or poison the device slot.
Host-call, hardware, or primary-event timeout after dispatch enters
`CompletionUnknown`, closes readiness, and poisons the device incarnation. The
watchdog cannot claim it preempted an uninterruptible kernel call: failure to
reap enters `ExecutorStalled`. A later explicit rejection releases only the
never-submitted ledger after exact-once reconciliation; a later success remains
accepted-stale and quarantined unless its incarnation, lifecycle epoch, optional
transition id, and commit id are all current and the record has not already
terminalized.

### 10.4. Present and release terminalization

Every Present intent reaches a protocol terminal state even when its KMS commit
does not. The owner applies these rules during topology invalidation,
`CompletionUnknown`, device loss, and shutdown:

- a never-submitted primary-plane successor displaced by a newer intent for the
  same plane releases its buffer, pins and wakes at supersession time and emits
  `IdleNotify` exactly once. Its `Skip` `CompleteNotify` is withheld until the
  in-flight predecessor completes, then published after that predecessor; the
  idle event is not re-emitted with the deferred `Skip`;
- an accepted Present without a matching `Presented` milestone completes as
  `Skip` using the last validated CRTC clock sample; it never fabricates a new
  MSC/UST or reports `Flip`;
- sending/suppressing the notification follows normal drawable/client-liveness
  rules, but the per-client FIFO is unparked in either case;
- for an accepted commit, Present completion does not imply buffer idleness.
  Pixmap idle notification,
  release syncobj/timeline advancement, dma-buf release, and pin release wait
  until `PriorBufferReleased` or the section 10 teardown barrier proves the
  buffer unreachable. This accepted-state rule does not delay release of the
  never-submitted successor above, which KMS never observed;
- an accepted pending predecessor follows the accepted rule above and cannot
  be completed a second time during teardown.

The terminalization ledger keys protocol completion, idle/release completion,
and resource quarantine separately by Present serial/commit id. Device rebuild
cannot inherit or signal an old generation's release point accidentally.

## 11. Cursor lifecycle

Requirements in this section are identified as **CURSOR-LIFECYCLE**.

### 11.1. Load/change

- Upload/import a cursor framebuffer.
- Record a new desired generation.
- Retain the old framebuffer until every commit that references it retires.
- Coalesce multiple image/animation updates before submission to the newest
  complete state.

### 11.2. Move

- Convert root coordinates to the owning CRTC and subtract hotspot exactly
  once.
- Deduplicate identical effective positions.
- Coalesce unsent motion latest-wins.
- Crossing outputs detaches the old CRTC plane and attaches the destination
  plane as one ordered device transaction when both CRTCs share a device.
- Crossing devices uses the coordinator and detach-before-attach protocol in
  section 13; independent device owners cannot make that transition atomic.

### 11.3. Hide/show

- Hide atomically detaches the plane; its completion retires the last scanned
  cursor framebuffer reference.
- Show restores the newest image, hotspot, and position even when they changed
  while hidden.
- XFixes hide/show and `Cursor=None` use the same state machine.

### 11.4. Hardware-to-software transition

Every reason hardware cursor becomes unusable while its plane may still be
visible—format/modifier/size rejection, clipping arithmetic failure, required
scaling, topology/readiness loss, explicit policy change, or recovery—uses one
per-output state machine. A `CAP-4` measured demotion is an explicit policy
change and takes this path unchanged; it introduces no second transition
mechanism and no shortcut past the no-duplicate ordering:

```text
HwVisible
  -> HidePending        (direct admission closed; next composed frame omits SW)
  -> SwRevealPending    (cursorless frame retired and atomic detach completed)
  -> SwVisible          (damaged composed frame containing SW cursor retired)

HidePending
  -> HwDetachUnknown    (detach was accepted but completion became unknown)
  -> SwRevealPending    (only after teardown plus fresh detach proof)
```

`HidePending` first exits direct scanout through the ordered unflip path when
necessary. The frame immediately preceding detach contains no software sprite;
if detach fails before acceptance, hardware remains the only visible cursor and
the transition is retryable. If acceptance is followed by unknown completion,
the transition instead enters `HwDetachUnknown`. Only proven detach permits
`SwRevealPending`, which damages the old and newest cursor footprints and submits
the software reveal. The server reports software visibility only after that
composed frame retires. Hide, `Cursor=None`, topology invalidation, and a newer
cursor generation may redirect the desired terminal state, but never skip the
no-duplicate ordering.

`HwDetachUnknown` is an internal truth state: hardware may or may not still show
the sprite. The server keeps software cursor composition suppressed, closes
direct admission/readiness, reports neither proven HW nor SW visibility to
internal consumers, and quarantines both possible cursor-plane resource sets.
The core may remain responsive and render off-screen without a software sprite,
but it submits no ordinary frame or other live KMS commit on the poisoned
incarnation. This state ends only after the section 10 teardown barrier makes
the old plane state unreachable and a fresh incarnation installs one of two
proven outcomes:

- the real qualification/install commit atomically attaches the newest valid
  hardware cursor state and its completion enters `HwVisible`; or
- it proves the cursor plane detached, after which the machine enters
  `SwRevealPending` and may retire a newly damaged composed software cursor.

Retiring the complete old fd set and opening a fresh incarnation without a
successful replacement/detach commit is not visibility proof. Desired image,
hotspot, position, and hide/show generation are
preserved in device-independent memory and revalidated against the fresh
topology before either outcome. A requested hidden terminal state installs and
proves detach but does not schedule software reveal.

If hardware is already proven detached—as in cross-device `SwRecovery` after a
completed source detach—the machine enters `SwRevealPending` directly. This is
the sole shortcut. Direct scanout remains inhibited throughout every SW state
while the cursor is visible, because a client scanout framebuffer cannot contain
yserver's composed sprite.

### 11.5. Animation

- Animation deadlines update desired image state without forcing scene
  composition.
- Slow or pending KMS retains the newest animation frame; it never builds an
  unbounded queue.

## 12. Interaction with Phase B direct scanout

The merged Phase A+B baseline is explicit:

- core supersession follows Xorg target identity and coverage;
- due clocked async requests execute immediately rather than being blanket-
  parked;
- one in-flight direct frame plus one generic latest-wins authoritative-root
  successor is always active without a runtime gate;
- replacement idles the never-submitted victim immediately and defers its
  `Skip` behind the predecessor;
- composed/direct/unflip work is always visible to Present pacing; and
- the hardware-cursor strategy is enabled by default but the shipping NVIDIA
  device policy still selects software cursor.

C.0 preserves those protocol outcomes, but it necessarily changes the primary
submission machinery. `submit_direct_scanout` is converted from a plane-only,
zero-user-data atomic call into a section 6.3 owner transaction with exact
event identity, canonical out-fence evidence and compatible cursor/gamma
absorption. Retirement-time successor promotion enters the owner through the
named tier in section 9.2.1 rather than issuing an atomic commit directly from
the event handler.

Direct entry must attach the current cursor state atomically or prove that the
already-submitted cursor plane state remains valid. Direct exit/unflip must not
drop or flash the cursor. A primary flip event cannot retire a newer cursor
generation merely because both share a CRTC.

Phase B must preserve the current gamma blob across direct entry, direct frame
replacement, and composed unflip. A primary-plane event must not retire a newer
cursor generation or gamma blob merely because they share a CRTC.

The `c09358a1` VT workaround is also a mandatory conversion site. A C.0-ready
device does not call best-effort legacy `cursor_plane_hide_all` before the
all-off transaction. Its authoritative atomic VT request includes cursor-plane
detach for every affected CRTC together with the primary/connector/CRTC state,
or uses an owner-ordered detach whose canonical completion is proven before
disable. Final `TEST_ONLY` decides which exact driver-supported shape is used;
failure closes readiness and follows lifecycle recovery rather than ignoring a
cursor-hide error. Cursor framebuffer lifetime follows that proven detach.

The existing hardware-cursor requirement becomes part of structural
`atomic_kms_pipeline_structurally_capable`. A device without atomic cursor
coverage uses the software cursor and cannot enable Phase C.1. A device without
atomic gamma exposes gamma unavailable through `atomic_gamma_capable=false` but
may enable C.1 when the structural, cursor-policy, cursor/primary and completion
gates pass.

NVIDIA is not declared good at hardware cursor merely because its cursor plane
exposes the right properties, and it is not declared bad merely because it is
NVIDIA. Section 16.3 compares the shipping software cursor, the rejected
legacy-hardware path and the C.0 atomic-hardware path on the measured drag
workload. That comparison no longer decides whether the cohort may enable
hardware cursor at all — `CAP-4` makes that a runtime decision for every device.
It decides two narrower questions: whether the measured demotion actually
catches this driver, and whether the cohort earns a degradation prior so its
users never see the degraded window. If the atomic arm preserves smooth
interaction, removes the X11-core stall and meets the direct-scanout and
maintenance bounds, the cohort takes no prior and runs hardware cursor like any
other device. If it fails, the recorded result justifies the prior entry and its
driver-version bound. Either way the result and test hardware are recorded for
that exact cohort rather than generalized to every NVIDIA architecture, hidden
behind a runtime override, or modeled as transient not-ready state.

Structural coverage is re-evaluated before every topology installation. If a
proposed topology loses it while C.1 is enabled, C.1 submission is first
quiesced through the C.0 owner and only then is the topology installed. The
cacheable capability advertisement is recomputed only if the client-visible
protocol domain changes as defined in section 6.2; otherwise qualification and
readiness close without mutating the advertised bit. No C.1 commit may race
that transition.

## 13. Multi-device and multi-output

Requirements in this section are identified as **MULTI**.

- Commit ownership is per DRM device, never global and never based on device
  enumeration order.
- Every active CRTC maps to its owning device and a distinct cursor plane.
- A small backend-level cursor-transfer coordinator orders cross-device moves;
  it does not issue DRM commits itself and does not replace device-local commit
  ownership. It records one transfer generation, source, destination, and the
  newest desired cursor state.
- Cross-device movement is detach-before-attach: submit source detach, wait for
  its proven completion, then submit destination attach. This permits a bounded
  interval with no hardware sprite but never two visible hardware sprites.
  Motion/image changes during the transfer coalesce into its newest desired
  state.
- If source detach fails before submission, the source remains current and the
  transfer is retryable. If destination attach fails after a completed detach,
  no hardware cursor is visible; the coordinator retains the desired state and
  enters `SwRecovery`, never reattaching a stale source generation. Device loss
  follows the quarantine rules in section 10.
- If source detach is accepted but reaches `CompletionUnknown`, the coordinator
  enters source `HwDetachUnknown`, submits no destination attach, reveals no
  software cursor, and waits for the source teardown plus fresh-install proof
  defined in section 11. Only then may it attach the newest desired state at the
  destination or start software reveal; this branch can show a bounded outage
  but never duplicate an uncertain source sprite.
- `SwRecovery` closes the direct-scanout admission gate for the destination,
  requests composed unflip if direct ownership is active, and enters the common
  section 11 state machine at `SwRevealPending`. Because source detach is already
  proven, the first successfully retired composed frame may contain the software
  sprite; no cursorless intermediate is required in this failure branch. Direct
  scanout cannot re-enter until a later hardware attach retires or the cursor is
  no longer visible.
- If software composition/unflip fails, the cursor remains truthfully `Hidden`,
  readiness stays false, and normal renderer/device recovery runs. The server
  never claims software visibility before the composed frame retires. The
  detach-to-software-visible latency is bounded by one unflip/composed commit
  plus owner dispatch under section 9.2.1; timeout terminalizes under section
  10 rather than waiting indefinitely.
- Topology epochs invalidate queued cursor intents and framebuffer associations.
- Connectors excluded as `non-desktop` are outside the protocol domain. A
  non-desktop-only card has only the discovery record/fd from `CAP-2`, no C.0
  owner or readiness record; udev-triggered reprobe observes a runtime connector-
  class change before rebuilding domain membership. Without that monitor, the
  change requires restart and is never advertised as live support.
- A device without universal cursor-plane coverage reports the cursor/primary
  pipeline unavailable only for its own outputs. Missing atomic `GAMMA_LUT`
  reports gamma unavailable independently.
- Unsupported devices never regain an uncontrolled legacy state-mutating path;
  only the section 5 owner-mediated coordinate transport is permitted.

## 14. Projection to Phase C.1

Phase C.1 inherits C.0's generic primary-plane successor and its immediate-idle,
deferred-`Skip` contract. It does not introduce an async-only latest-wins slot
or change core Present equivalence. Its additional scope is limited to async
capability/admission, the singleton primary request shape, producer-fence
transfer, `PAGE_FLIP_ASYNC`, and its page-event completion contract.

Phase C.1 consumes exactly the cursor/primary capable, incarnation-qualified
and per-submit-ready layers in section 6.2; it does not consume
`atomic_gamma_capable`. This section does not redefine them. Before installing a topology that changes
structural coverage, C.0 quiesces C.1 through the owner. If the client-visible
protocol domain is unchanged, only qualification/readiness close. If the domain
changes, advertisement is recomputed before the new domain is exposed.

C.1 admission calls `atomic_kms_pipeline_ready` with the exact incarnation,
lifecycle epoch, protocol CRTC, and topology generation used to build its intent. A busy healthy
owner queues that intent under section 9.2.1. Any mismatch, cursor transfer,
software recovery, `Poisoned`, `Recovering`, or `RecoveryFailed` state returns
false without altering cacheable capability. `ExecutorStalled` likewise returns
false and cannot be cleared by
topology rediscovery, DPMS, or ordinary traffic; only actual executor reap
releases its `ID-2` barrier.
Independently, C.1 admission requires exactly one active CRTC on the DRM device;
multi-output uses synchronized Phase B until a conflict-set successor replaces
C.0's single device slot.

## 15. Telemetry

Provide counters or structured logs for:

- the merged `m1_gate_*` per-reason direct-eligibility counters, preserved and
  extended so `m1_gate_reject_cursor` distinguishes property/capability failure
  from the NVIDIA software-cursor device policy;
- primary-plane successor queued/replaced/promoted, async option only as
  diagnostics, immediate idle/release timestamp, deferred `Skip` publication,
  predecessor completion ordering, and duplicate-idle prevention;
- atomic cursor desired/submitted/retired;
- moves coalesced and identical moves deduplicated;
- cursor-only commits, changed-persistent cursor bundled with primary updates,
  unchanged cursor planes omitted from primary requests, and forbidden
  coordinate absorption;
- pending/queued high-water marks;
- atomic versus coordinate-transport `EBUSY`, whether the one permitted
  overlap-safe primary conflict existed, coalesced retry eligibility, retry-
  after-completion result, consecutive count, successful-return reset,
  readiness/transport closure, and proof that neither an atomic completion nor
  an immediate retry reset the count; measurement-only absorbed-conflict
  closure suppression and the production closure it would have taken are
  separate fields and must remain zero/unreachable outside that exact artifact;
- `NativeCursorCompositionContract` result and per-field rejection,
  `AuditedCursorExpansionHazard` boolean and named reasons over the complete
  final request, exact userspace-included objects/planes, returned coordinate-
  call duration/result, incarnation-scoped over-bound closure and fallback
  selected. These fields explicitly label the hazard as a source-derived
  prediction and never as the driver's observed post-check closure;
- commit ids, atomic/expected/event/completed CRTC sets, old/new binding inputs,
  exact userspace-included plane/object set, per-CRTC page-flag/out-fence
  signaling source, off-to-off construction rejection, coordinate-overlap-safe
  classification, partial completions, completion mechanism, and terminal state;
- per-CRTC primary-event deadline, arrival, and expiry, including the mode
  period (or documented fallback) used to calculate each deadline;
- accepted/hardware-complete/presented/prior-buffer-released milestones and
  fence/event arrival order;
- producer wait source/type, success/error/timeout/cancel outcome, and
  exact-once release; output-fd adopted, polled, canonical sync-file status,
  closed/quarantined, and exact-once invariant failures;
- per-`(CRTC,class)` `AdmissionTicket`, preserved latest-wins age, intervening
  maintenance commits, barrier interruptions, fairness yields, and calculated
  starvation-bound violations;
- final-live `AtomicSnapshotId` and rejection caused by a changed snapshot;
- completion-unknown quarantines and resources released at the teardown barrier;
- owner-initiated normal-runtime recovery stage, reopen/install failure, logical
  withdrawal or proven hardware-disable result, and submissions rejected while
  the incarnation is poisoned;
- `IncarnationFdSet` alias/lease count, helper cancellation/reap, last-reference
  closure, forbidden duplicate attempts, and fresh-incarnation creation;
- `RecoveryId`, sole automatic attempt, `RecoveryFailed`, authorized external
  retry trigger, hardware disable versus logical withdrawal, and retained
  quarantine;
- `LifecycleTransitionId`, kind, phase, precedence decision, supersession,
  equal-kind coalescing, lower-priority desired-state updates, and quarantine
  transfer;
- `LifecycleEpochId`, increment-before-invalidation evidence, optional transition
  id, and normal-versus-transition executor-result classification;
- `LifecycleEventId`, every `LifecycleDesired` field/generation change,
  `Applied`/`AbsorbedByEvent`/`AbsorbedByTransition`/`Invalidated`/
  `SupersededBy`/`Deferred` disposition, deferred
  prerequisite, convergence iteration, and selected next unsatisfied kind;
- `KmsIoExecutor` process/IPC generation, call class, blocking/nonblocking mode,
  dispatch/watchdog/reap times, incarnation, `LifecycleEpochId`, optional
  `LifecycleTransitionId`, `CommitId`/`EventToken` or `ClockProbeId`/CRTC/clock
  epoch as appropriate, lease duration,
  explicit reject/success/unknown result, staged events,
  accepted-stale quarantine, `ExecutorStalled` entry/exit, shutdown wait status,
  and `ShutdownExecutorStalled` duration;
- latency-artifact fixed capacity/occupancy/overflow, monotonic in-memory
  timestamps, primary dispatch/`HardwareComplete` interval, normalized phase
  decile, request shape, qualified-overlap count, initial/retry kind, linked
  initial `EBUSY`, retry-before-next-primary result, and whether a record belongs
  to required production-omitted evidence or optional absorbed characterization;
  per-stratum `PhaseCycleCap`/
  `PhaseAttemptCap` usage, terminal quota state, and cap-exhaustion reason;
- page-event classification and token allocation, resolved logical
  identity, pending tuple contradictions, duplicate warnings,
  tombstone-window occupancy/eviction, raw versus normalized MSC/UST,
  `KernelSequence` GET/QUEUE_SEQUENCE result,
  incarnation/CRTC/clock-epoch cache creation and reset,
  sequence-arm token/CRTC/epoch/purpose/target and terminal disposition,
  CRTC-clock epoch/reference, and primary-event deadline expiry;
- atomic CRTC closure inputs, old/new connector and plane bindings, final
  serialized closure recheck, expected fence/event sets, and any mismatch;
- cursor-hotspot negotiation outcome (`HotspotMetadataRequired`,
  `NativeCoordinateOnly`, or failure), property-pair validation, and discovery
  contradiction;
- cursor image/framebuffer generations current, pending, and queued;
- per-plane `FullSourceSignedDestination`/`SourceCrop` and coordinate-transport
  qualification, plus forbidden legacy cursor calls (zero) and permitted
  coordinate-only `MOVECURSOR` calls;
- input-to-cursor-submit, input-to-retirement and externally correlated input-
  to-visible-motion latency plus effective coordinate updates/s, aggregated as p50/p99
  and compared with the current shipping baseline under the same input/output
  mode: software composition on NVIDIA, legacy HW cursor where that remains the
  unmodified baseline, plus the separate NVIDIA legacy-HW scale arm;
- sustained physical device commits/s and logical retired generations/s per
  CRTC with one active CRTC and with two active homogeneous CRTCs, including
  each refresh target and the device-slot idle/occupied fraction; mixed/unknown-
  period runs instead record orderly C.0 quiesce and merged-baseline selection;
- direct-scanout Present/retirement FPS with an idle cursor and with continuous
  cursor motion, using the same application, mode, and producer load;
- helper-measured ioctl duration for every section 4.1 host-call class,
  executor IPC dispatch-to-reply latency and total input-to-dispatch overhead,
  aggregated as p50/p99/max with context-switch and message/fd counts;
- gamma desired/submitted/retired/coalesced;
- global protocol DPMS level/epoch, projected per-output targets, grouped
  device transition, per-output retirement, and topology inheritance/removal;
- gamma blob created/destroyed/current/pending high-water;
- legacy gamma ioctl calls and legacy gamma-size discovery queries, which must
  remain zero on a C.0-ready device;
- off-transition fence status and latency, incarnation-poison/
  `HwDetachUnknown`/watchdog counts, and eight-hour soak duration;
- RANDR-gamma-to-submit and submit-to-retirement latency;
- cross-device cursor-transfer generation, detach-to-attach gap, retries,
  `HwDetachUnknown`, and `SwRecovery` unflip/composed retirement;
- advertised-capability recomputation separately from incarnation qualification
  and operational-ready gate close/open, with the triggering protocol domain,
  device incarnation, and topology generation;
- mandatory initial/later real restore/install qualification evidence and
  deadline, device-incarnation completion poison, and topology-specific latch.

Per-motion logs remain debug/trace only.

## 16. Verification

### 16.1. Requirement traceability

The detailed test list below is evidence, not a second normative definition.
Every implementation plan task and test name must cite at least one requirement
group from this matrix. A behavior with no cited requirement is not acceptance
evidence; a normative requirement with no unit/state-machine and, where
applicable, hardware evidence is incomplete.

| Requirement group | Normative source | Unit/state evidence | Hardware/lifecycle evidence |
| --- | --- | --- | --- |
| `INV`, `ID-1..3` | Sections 5 and 6.1 | Owner reachability, lifecycle epoch, generation, fd-set, stale-event, and exact-close tests | Helper-alias, transition-race recovery, and teardown injection |
| `CAP-1..4` | Section 6.2 | Cursor/primary versus gamma capability, qualification/readiness, measured-demotion conjunction and its false-positive guard, and degradation-prior version-bound tests | VT, DPMS, topology, reopen, domain-change, and the demotion-mechanism hardware runs |
| `COMMIT-1..7` | Sections 4.1, 6.3, and 10 | Commit-class matrix, pre-IPC submitting boundary, fences/events, fd ownership, terminalization, per-CRTC deadlines, fixed executor architecture, watchdog, and orderly-reap tests | Driver fence/event/reply reordering, delayed/stuck-ioctl, executor death/reap, and shutdown injection |
| `REC-1..6` | Section 6.4 and section 10 | Poison, sole recovery attempt, total lifecycle mapping/precedence, recovery-fate matrix, bounded desired-state convergence, logical withdrawal, and quarantine tests | Concurrent/coalesced lifecycle, VT acquire/release, device replace, runtime loss, recovery-stage failure, and shutdown injection |
| `CURSOR-PAYLOAD` | Section 7 | Encoding, full-source/crop policy, coordinate-transport qualification, hotspot, plane compatibility, framebuffer lifetime tests | Visible/animated/high-rate cursor and legacy-baseline matrix |
| `GAMMA-PAYLOAD` | Section 8 | RANDR contract, LUT encoding, resampling, blob lifetime tests | Gamma round-trip and lifecycle matrix |
| `SCHED` | Section 9 | Bounded categories, tickets, coalescing, fairness, `EBUSY` tests | Concurrent CRTC/C.1 pressure runs |
| `CURSOR-LIFECYCLE` | Section 11 | HW/SW state machine and unknown-detach tests | Direct/unflip/failure-transition runs |
| `MULTI` | Section 13 | Device ownership and transfer coordinator tests | Same/cross-device crossing and failure injection |

### 16.2. Unit and state-machine tests

1. [`CURSOR-PAYLOAD`] Plane-property encoding covers visible, hidden,
   hotspot-adjusted and cross-output cursor states. Every partial edge defaults
   to full source plus signed destination; optional source cropping is selected
   only by the exact per-plane qualification matrix.
2. [`CURSOR-PAYLOAD`] Identical position deduplication.
3. [`SCHED`, `CURSOR-PAYLOAD`] Multiple unsent moves coalesce to the newest coordinates.
4. [`CURSOR-PAYLOAD`] Image replacement retains current/pending framebuffer lifetimes.
5. [`CURSOR-LIFECYCLE`] Hide followed by Show during a pending commit restores the newest sprite.
6. [`SCHED`] Cursor-only work progresses without scene damage or a primary flip.
7. [`SCHED`] Atomic `EBUSY` with no owner live record closes readiness and
   enters bounded recovery without retry. Coordinate `EBUSY` beside the one
   accepted overlap-safe primary coalesces and retries once after completion;
   absent a lifecycle barrier that retry precedes the next primary admission.
   Only a successful coordinate return resets the consecutive count. Atomic
   completion without such a return does not; a second consecutive or
   impossible-context result closes only that fast transport and falls back
   without a spin. Any returned over-bound coordinate call closes the fast
   transport for the plane incarnation and cannot be rehabilitated there.
8. [`SCHED`] Primary and cursor intents preserve ordering when combined or separated.
9. [`REC-4`] Stale VT/DPMS/hotplug/topology generations cannot submit.
10. [`MULTI`] Cross-device/output movement never leaves duplicate visible cursor planes.
11. [`CURSOR-LIFECYCLE`] Direct entry/unflip preserves cursor visibility and framebuffer ownership.
12. [`SCHED`, `CURSOR-PAYLOAD`] Animated cursor coalesces without unbounded state.
13. [`CAP-1`, `CURSOR-PAYLOAD`] Cursor/primary structural capability is false
    without complete universal-plane coverage and one qualified coordinate
    transport per required cursor plane. Qualification is exact-cohort evidence,
    never a vendor-family default. Preference tests select qualified
    `OwnerMediatedLegacyMove` in composed and direct state; without it they
    select software in composed state and qualified `SynchronousAtomicMove` in
    direct state, or the ordered software/unflip transition when synchronous
    atomic did not qualify. Driver/kernel/plane/mode/topology invalidation forces
    qualification before reuse. Every C.0 primary builder enforces
    `NativeCursorCompositionContract`; direct eligibility rejects scaled,
    partial-coverage, non-XRGB8888 including YUV/video and HDR/10-bit candidates
    before KMS. A format-consistency test binds the composed
    `VkScanoutFb::format()` and direct eligibility gate to XRGB8888 so neither
    can drift independently. Construction of any otherwise permitted out-of-
    contract primary invalidates the fast transport before ioctl dispatch;
    completion cannot perform that invalidation. State-machine tests forbid
    re-selection in the same plane incarnation and require detach/reattach plus
    complete requalification with the contract-valid primary represented in
    the state. Table-driven complete-request cases set
    `AuditedCursorExpansionHazard` for modeset, CRTC gamma/CTM/degamma, VRR,
    DSC-force, plane enable/disable or binding, format, scale-ratio, z-order and
    plane color-pipeline changes. Only a contract-preserving primary with no
    hazard and no userspace cursor object is overlap-safe. Stock NVIDIA never
    selects `OwnerMediatedLegacyMove` in C.0.
14. [`INV`, `CURSOR-PAYLOAD`] No C.0 cursor load/show/hide/disable reaches a
    legacy ioctl. The only reachable legacy call is coordinate-only
    `MOVECURSOR`, from the owner, for a qualified installed plane with an idle
    atomic slot or the exact accepted coordinate-overlap-safe primary class.
    The per-
    plane reservation blocks new KMS host calls until typed return/reap;
    changing any other field or bypassing the owner fails the test. Internal
    helper async-check rejection is exercised as an ordinary slow-path return,
    not fabricated as a userspace `EINVAL`.
15. [`GAMMA-PAYLOAD`] `GAMMA_LUT_SIZE` validation accepts exact representable arrays and rejects
    zero, `u16` overflow, byte-size overflow, over-1-MiB allocation, and request
    mismatch without clamping. Legacy `get_crtc().gamma_length()`, cached-size
    clamping, and any second gamma-size source are unreachable.
16. [`GAMMA-PAYLOAD`] DRM color-LUT encoding preserves all RGB entries and zeroes `reserved`.
17. [`GAMMA-PAYLOAD`] Gamma replacement/coalescing preserves desired/pending/current blob
    lifetime and destroys every superseded unsubmitted blob exactly once.
18. [`SCHED`, `GAMMA-PAYLOAD`] Gamma-only work progresses without scene damage.
19. [`GAMMA-PAYLOAD`, `REC-4`] Modeset, DPMS, VT, hotplug, direct entry/unflip, and shutdown preserve or
    safely retire the desired LUT.
20. [`CAP-1`, `CAP-3`, `GAMMA-PAYLOAD`] Missing cursor-plane coverage makes the
    cursor/primary pipeline incapable. Missing atomic `GAMMA_LUT` makes only
    `atomic_gamma_capable` false and cannot close C.1 qualification/readiness.
21. [`INV`, `GAMMA-PAYLOAD`] No C.0 gamma operation reaches `set_gamma`.
22. [`SCHED`] One device never has two submitted nonblocking live atomic commits,
    including for otherwise disjoint CRTCs. The only overlap is one per-plane
    coordinate reservation with an accepted contract-preserving primary whose
    recorded userspace request omitted the cursor and whose audited expansion
    hazard is false; it cannot dispatch another atomic transaction.
23. [`COMMIT-1`, `COMMIT-2`] A multi-CRTC commit remains hardware-pending after a partial out-fence set
    and retires exactly once after the complete expected set reports successful
    signalled status.
24. [`ID-1`, `COMMIT-2`] Page events require the exact incarnation,
    incarnation-unique `EventToken`, and CRTC; resolving the token recovers the
    record's device generation, lifecycle epoch, and `CommitId`. Duplicate,
    zero/unknown token, tombstoned, and old-incarnation events cannot advance or
    poison the pending commit; the current pending token with zero or a CRTC
    outside `KernelEventCrtcs`, or an unparseable payload, poisons immediately.
    A delayed old-generation event cannot match a newer active commit even after
    more than 64 tombstone evictions. Allocation never returns zero or reuses a
    token. Checked allocation has a debug assertion at `u64::MAX`; the design
    adds no production exhaustion branch, restart protocol, wrap, or automatic
    incarnation replacement for an unreachable-timescale condition. A missing
    `PresentEventCrtcs` member poisons only
    at its deadline, while a missing non-consumer kernel event cannot hold
    completion. Tombstone storage remains bounded.
25. [`COMMIT-2`] Cursor-only and gamma-only commits make progress through their canonical
    out-fences without depending on page-flip event delivery.
26. [`REC-1`, `COMMIT-1`] VT loss, device loss, hotplug, and bounded drain timeout terminalize a
    submitted commit as `CompletionUnknown`, quarantine both possible resource
    sets, and release them only after the teardown barrier.
27. [`SCHED`, `CAP-1`] `EBUSY`, `EACCES`, removed-object, `EINVAL`, and device-
   loss failures do not enter a generic completion-driven retry path. The sole
   exception is one deferred retry of an already-qualified coordinate
   transport after its recorded overlap-safe primary completes. None
   selects a new legacy ioctl; retry exhaustion closes that transport. A real
   visible coordinate `EINVAL` is distinct from an internal async-check result
   consumed by the helper. An internal rejection that produces a returned
   over-bound call closes the transport as a coordinate-policy/cohort defect
   rather than masquerading as a userspace errno or per-key suspension.
28. [`MULTI`, `CURSOR-LIFECYCLE`] Cross-device transfer submits destination attach only after source-detach
    completion; attach failure leaves no duplicate sprite and activates the
    software-cursor recovery path with the newest desired generation.
29. [`CAP-1`, `CAP-2`, `CAP-3`] A topology that loses cursor coverage or its
    completion mechanism quiesces C.1 and closes qualification/readiness before
    installation. Losing `GAMMA_LUT` only updates gamma capability/RANDR state.
    Only a client-visible protocol-domain change recomputes the advertised
    cursor/primary capability, while VT/DPMS never does.
30. [`GAMMA-PAYLOAD`] Gamma state follows stable RANDR CRTC identity across hardware reassignment,
    resamples on LUT-size changes, and cannot leak to an unrelated output.
31. [`GAMMA-PAYLOAD`] Unsupported gamma reports size zero and empty channels.
    `SetCrtcGamma` pins Xorg's exact precedence: truncated fixed header is
    `BadLength`; invalid CRTC is RANDR `BadCrtc`; leased CRTC is
    `BadAccess` before payload validation; a body shorter than the checked padded
    minimum for declared non-zero `size` is `BadLength`; a sufficiently long
    non-zero declaration is `BadMatch`; declared zero succeeds as a no-op; and
    trailing bytes are accepted for both zero and matching supported sizes.
32. [`SCHED`] Every ready maintenance identity receives a ticket even with an idle slot;
    it submits immediately when alone and becomes aged without changing ticket
    after losing one admission. A continuous synchronous direct-successor
    stream may take tier 3 only when it absorbs every otherwise-winning aged
    identity and satisfies primary round-robin; a primary that cannot do so
    yields to the aged identity. Each of `N` incompatible aged
    `(CRTC,class)` identities is admitted after at most the already-submitted
    commit plus `N - 1` older tickets. Latest-wins preserves the ticket, and
    cursor cannot starve gamma or another CRTC. In a qualified homogeneous
    group, two or more ready primary CRTCs enter tier 5 as one transaction before
    tier 6 can choose a singular round-robin replacement; no timer waits for a
    missing bundle member.
33. [`SCHED`] When maintenance wins admission, it absorbs a compatible ready
    synchronous primary on the same CRTC without crossing an unflip/topology
    barrier; retirement-promoted synchronous direct successors perform the
    symmetric absorption. Only changed persistent cursor/gamma generations may
    be absorbed; unchanged cursor planes and coordinate-only intents are absent
    from the request. Incompatible work remains separate and the combined commit
    retires every absorbed generation and ticket exactly once.
34. [`ID-1`, `COMMIT-1`] Speculative `TEST_ONLY` plus unchanged topology generation cannot authorize
    live installation after cursor/gamma/primary generation changes; final
    serialized test and live submit use identical persistent state while live
    sync properties/fds/holder addresses are freshly constructed.
    `ValidationOnly` creates no live record/out-fence and holds the exclusive
    validation lease; coordinate movement arriving during that lease is
    coalesced but cannot dispatch until the paired live installation completes
    or the lease is abandoned. It uses the executor's two- or 30-second
    watchdog, and timeout never becomes acceptance-unknown hardware state.
35. [`CURSOR-PAYLOAD`] Cursor edge tests pin full-source/signed-destination
    rectangles for every partial edge, i915-style `SRC_X/Y=0`, fully offscreen
    detach, checked signed encoding, hotspot extremes, overflow and no scaling.
    Crop-capable and crop-rejecting qualification never switches per movement.
36. [`CURSOR-PAYLOAD`, `MULTI`] Cursor-plane qualification rejects incompatible format, modifier, size, or
    CRTC coverage, and cross-plane transfer uploads a compatible destination FB
    without releasing the source generation early.
37. [`CURSOR-LIFECYCLE`] Destination attach failure during direct scanout closes direct admission,
    submits composed recovery, reports `Hidden` until its retirement, and only
    then reports software cursor visibility.
38. [`REC-1`, `REC-4`] Each `CompletionUnknown` trigger follows its transition-table result,
    preserves only permitted desired state, and drops quarantined owners in the
    required fd/Vulkan/GBM/shared-resource order. A normal-runtime failure starts
    exactly one recovery attempt without an external lifecycle event and enters
    `RecoveryFailed`, logically withdrawing/reporting affected outputs without a
    KMS disable, if any recovery stage fails or becomes unknown.
39. [`CAP-1`, `CAP-2`, `CAP-3`] Advertised capability remains stable through VT/DPMS, fd reopen,
    qualification failure, and incarnation poison while readiness closes; only
    a protocol-domain-changing hotplug/routing change recomputes advertisement,
    and C.1 consumes the incarnation- and topology-qualified ready gate.
40. [`REC-3`, `CURSOR-LIFECYCLE`] Every HW→SW cause follows `HwVisible -> HidePending -> SwRevealPending ->
    SwVisible`; pre-submit detach failure leaves only HW visible. Accepted
    unknown detach enters `HwDetachUnknown`, suppresses SW reveal and destination
    attach and every further live commit on the poisoned fd, and exits only after
    teardown plus a fresh proven attach/detach.
41. [`SCHED`] Primary intent categories meet their per-CRTC bounds: composed damage
    accumulates, the generic full-plane successor is latest-wins for both
    synchronized and async Present options, unflip cannot be superseded, and
    the core still scraps only target/CRTC/coverage-equivalent requests.
42. [`COMMIT-4`] Producer success releases each acquire wait exactly once before admission
    and submits with `IN_FENCE_FD` omitted or `-1`; producer
    error/timeout/cancel never calls the atomic ioctl, occupies no device slot,
    and performs local never-submitted cleanup. Reject and `TEST_ONLY` accept an
    output holder of `-1`, while live success plus `-1`, unexpected or partial
    output, and unknown paths fail without leak or double-close.
43. [`COMMIT-1`, `COMMIT-2`] Copied/direct atomic rejection preserves the existing
    `ReleasedButAtomicRejected`/FOREIGN recovery instead of pretending KMS
    completed an ownership return.
44. [`COMMIT-2`] Out-fence may signal before or after the tagged page event: hardware state
    retires only on its fence set, Present completes only on its event, and prior
    buffer release waits for its separate dependency in both orders.
45. [`CAP-3`, `COMMIT-1`, `COMMIT-5`] Initial and later incarnations insert no synthetic primary/cursor/gamma
    transition and keep readiness closed until the mandatory first real
    install/restore commit with a non-empty `ExpectedCompletionCrtcs` completes
    with successful out-fence status, including for a blocking ioctl at a
    `COMMIT-5`-allowed cold-start/final-offline boundary; an empty expected set
    cannot qualify vacuously.
    A completion-mechanism breach poisons the incarnation, while a classified
    topology rejection latches only that topology generation. Real DPMS-off,
    VT all-off and disable require successful old-active out-fence evidence
    without depending on a later physical vblank.
46. [`CURSOR-PAYLOAD`] Hotspot discovery enables atomic first and attempts the hotspot cap before
    plane enumeration. Success requires both properties, programs unmodified
    metadata, and performs one visual coordinate subtraction. `EOPNOTSUPP`
    selects native coordinate-only hardware cursor rather than rejecting it;
    every other cap error fails discovery. Property/capability contradictions
    reject the affected discovery without retrying a metadata-required plane as
    coordinate-only.
47. [`COMMIT-1`, `COMMIT-2`] Producer, hardware, and primary-event timers start at their specified
    monotonic milestones; producer waits inherit their source-specific policy,
    the other timers apply the exact clamp formulas, and a producer timeout is
    classified separately from post-accept KMS timeout.
48. [`COMMIT-1`, `COMMIT-2`] Accepted Present without `Presented` completes exactly once as `Skip`,
    unparks its client FIFO, emits no invented MSC/UST, and withholds idle,
    release syncobj, pins, and dma-buf release until the independent teardown or
    prior-buffer-release proof.
49. [`COMMIT-1`, `COMMIT-2`] Poll readability alone cannot advance completion: pending status remains
    armed, negative/error status or status-query failure enters
    `CompletionUnknown`, and one error in a mixed multi-CRTC fence set prevents
    every hardware retirement in that transaction.
50. [`ID-2`, `REC-1`] Topology epochs and rediscovery on the same open file description cannot
    clear a poisoned device incarnation. A fresh incarnation is impossible until
    alias creation stops, helpers are cancelled/reaped, and every
    `IncarnationFdSet` lease closes; its mandatory real install/restore commit
    must then qualify before readiness opens.
51. [`CAP-1`, `CAP-2`] `FailedBeforeSubmit`, topology rejection, qualification failure, fd reopen,
    and incarnation poison never rewrite an advertised capability bit; only
    construction of a changed client-visible protocol domain recomputes it.
52. [`COMMIT-1`, `COMMIT-2`, `COMMIT-5`] The completion-class matrix is exhaustive: nonblocking primary Present uses
    out-fences plus tagged events; nonblocking work without a Present consumer
    uses out-fences only; C.1 async direct transfers one unresolved input fence,
    carries no out-fence, and uses its sole tagged event for both typed hardware
    and presentation evidence; cold-start/final-offline ordinary blocking work
    uses ioctl return; and qualification at those same blocking boundaries uses
    ioctl return plus out-fences without an event unless it also carries a
    Present consumer. No blocking row is legal during seat-active service.
53. [`COMMIT-1`, `COMMIT-3`] Table-driven request construction computes the exact atomic CRTC closure
    for CRTC properties and old/new connector/plane bindings, including both
    powered endpoints of moves and the old endpoint of detach. Enable and
    disable both produce evidence; inactive-to-inactive may be empty and cannot
    qualify. Exactly that set receives `OUT_FENCE_PTR` for every non-C.1-async
    class; C.1 async receives none and must have a singleton set. For each CRTC
    in the closure, either global `PAGE_FLIP_EVENT` or a local `OUT_FENCE_PTR`
    creates kernel event state and therefore rejects old-inactive/new-inactive;
    an off-to-off member is accepted only with neither signaling source. A
    mutation between closure calculation and final serialization fails before
    dispatch, and adding an off-to-off fence for symmetry fails the test.
54. [`ID-2`] Holding a parent/helper duplicated KMS fd prevents old-incarnation retirement
    and fresh-incarnation numbering; cancellation, process reap, lease closure,
    and forbidden untracked duplication are exact and race-tested.
55. [`REC-1`] A failed or unknown qualification during automatic recovery cannot recurse:
    exactly one `RecoveryId` reaches `RecoveryFailed`, ordinary traffic and DPMS
    cannot retry it, and each authorized external boundary creates at most one
    fresh attempt.
56. [`REC-2`] Recovery failure performs logical withdrawal without issuing or claiming a
    hardware disable; shared/KMS resources stay quarantined until later healthy
    replacement/disable or another proven device/shutdown barrier.
57. [`REC-4`, `REC-6`] Every permutation of simultaneous shutdown, removal,
    VT release/acquire, device add/replace, administrative reprobe,
    identity-changing hotplug, same-identity topology, DPMS, and normal recovery
    elects exactly one transition by the normative precedence. Supersession
    transfers quarantine, terminalizes each Present once, preserves only
    remappable tickets/intents, and prevents the displaced transition from
    opening or publishing an fd.
58. [`COMMIT-5`, `COMMIT-6`, `REC-4`] The X11 core stays responsive while an
    executor call is deliberately delayed. Every seat-active live submit carries
    `NONBLOCK`; lifecycle supersession makes a late explicit rejection locally
    releasable but a late success accepted-stale and quarantined. Missing reply
    remains acceptance-unknown, and the lease prevents premature fd retirement.
    A delayed coordinate ioctl may coexist with an overlap-safe accepted
    primary and holds its per-plane `CoordinateSubmitting`; watchdog/IPC loss
    closes its transport but neither atomic fallback nor another KMS host call
    dispatches until actual helper reap, after which the newest point overwrites
    any acceptance-unknown old coordinate.
59. [`COMMIT-2`, `SCHED`] A qualified homogeneous multi-CRTC Present calculates
    and arms one event deadline per required Present CRTC. A pre-arrived event
    arms no timer for that CRTC, and expiry of any remaining deadline produces
    exactly one `CompletionUnknown` transaction. Mixed-refresh or unknown-period
    topology never reaches this C.0 transaction: it quiesces the owner and
    selects the merged baseline before installation.
60. [`COMMIT-5`, `COMMIT-6`] Live blocking commits are accepted only during cold startup before
    service or final offline/shutdown after prompt obligations. VT release and
    device removal never wait for one; current tagged results may be consumed at
    an allowed barrier. `ValidationOnly` is separately exercised seat-active
    and cold/offline with no live slot or ownership transfer. Stale rejection,
    success, and unknown live results follow their distinct `COMMIT-6` paths.
61. [`COMMIT-1`, `COMMIT-2`, `COMMIT-6`] `Submitting` is installed and occupies
    the device slot before IPC send. A matching page event arriving before the
    success reply is staged and later consumed; event plus explicit rejection is
    contradictory and poisons, while no second ioctl dispatches in the interval.
62. [`COMMIT-5`, `COMMIT-6`, `ID-2`, `REC-6`] Seat-active and allowed-blocking
    executor-watchdog expiry terminates protocol work without claiming rejection, requests helper
    death, and quarantines both states. Immediate reap permits the current
    transition to continue once; simulated uninterruptible reap enters
    `ExecutorStalled`, forbids reopen/release/retry, and resumes that same
    recovery incident under the `REC-6`-selected current transition only after
    the lease disappears.
63. [`REC-4`, `REC-5`] Every lower-priority event permutation during each active
    transition updates a bounded snapshot rather than a FIFO. On terminalization
    each representative has one current disposition; replacement terminalizes
    the displaced id immediately, while prerequisite-deferred state later reaches
    exactly one terminal outcome. Every still-valid unsatisfied field selects
    one next transition by precedence, with no stale hardware payload replay.
64. [`REC-1`, `REC-5`, `REC-6`] Equal topology events collapse to newest discovery epoch
    and force fresh rediscovery; seat and global DPMS generations are latest-
    wins, replace all current per-output projections, and immediately
    terminalize displaced ids; shutdown/removal are idempotent.
    An arbitrarily long storm retains only the bounded representatives;
    recovery fate is delegated exclusively to the `REC-6` matrix.
65. [`ID-3`, `COMMIT-6`] Ordinary `Ready` cursor/gamma/primary work carries the
    current lifecycle epoch with `transition_id=None`; transition-owned work
    carries `Some(id)`. Lifecycle arrival first closes admission: a clean drain
    remains authoritative under the old epoch, coalesced events cause one bump,
    and forced abandonment bumps before invalidation so a delayed normal or
    transition reply cannot publish across that boundary.
66. [`REC-5`, `REC-6`] Every possible unsatisfied desired field selects its
    named transition kind. `VTAcquire` and `DeviceAddedOrReplaced` obey their
    device-present/seat-owned prerequisites, converge after either arrival order,
    and give all representatives one terminal disposition. A global X11 DPMS
    request projects one epoch to every current stable output; hotplug inherits
    that level, removal invalidates only the removed projection, same-device
    outputs form one ordered atomic transition, and no partial best-effort loop
    or global boolean can claim completion.
67. [`REC-1`, `REC-6`] Cross every winning kind with an active recovery incident
    and prove the matrix's exact invalidated, fresh-id, transferred, paused, or
    continued outcome. No path both invalidates and transfers an id, allocates
    two ids, lets DPMS revive `RecoveryFailed`, or spends an attempt twice.
68. [`COMMIT-5`, `COMMIT-7`, `ID-2`] Executor launch reexecs before running
    non-async-signal-safe child code. During shutdown, signal/IPC EOF/watchdog
    without wait status leaves the parent teardown supervisor and quarantine
    alive in `ShutdownExecutorStalled`; only actual reap closes the lease and
    permits ordered resource destruction. At the teardown deadline the parent
    exits with the lease recorded as unreaped, and a start attempted while the
    device lock is still held by the orphaned helper waits or refuses instead of
    installing state.
69. [`CAP-1`, `COMMIT-2`] Structural capability requires both
    `DRM_CAP_CRTC_IN_VBLANK_EVENT=1` and `DRM_CAP_TIMESTAMP_MONOTONIC=1` and a
    sole raw event-stream parser that preserves `user_data`, `crtc_id`,
    `sequence`, `tv_sec`, and `tv_usec` independently for page-flip and sequence
    records. It validates zero, undersized, overflowing, over-buffer, truncated,
    unknown-well-formed, and concatenated event lengths without a second reader
    or compatibility fallback. The raw ioctl wrapper compile-checks its request
    type on Linux glibc, Linux musl, and FreeBSD, and the baseline `IoctlReq`
    alias is absent. Cap zero,
    query failure, or a compatibility parser prevents initial capability;
    the same cap failure on a later incarnation preserves advertisement but
    closes qualification/readiness. Current `EventToken` plus zero `crtc_id` after
    successful discovery poisons immediately instead of waiting for the event
    deadline. Multi-CRTC capability additionally requires equal exact mode-
    derived refresh rationals and the homogeneous bundle gate; mixed/unknown
    periods select the merged baseline before topology installation.
70. [`COMMIT-2`] `KernelSequence` page-event normalization covers
    `tv_usec=999_999`, invalid `1_000_000`, maximum `u32` seconds with checked
    UST conversion, a successful GET_SEQUENCE trusted 64-bit reference, later
    exactly token/CRTC/epoch-correlated sequence-event references, late samples,
    exact `2^31` ambiguity, no
    non-negative representative, and `u32` sequence wrap
    `0xffff_fffe -> 0xffff_ffff -> 0 -> 1`. A raw zero never changes the selected
    source. The test proves per-CRTC/per-clock-epoch isolation, reset/reprobe on
    clock replacement, no clock regression, and correct client-visible MSC/UST.
    Every sequence arm uses a fresh incarnation-wide `EventToken`; raw-CRTC
    `user_data`, unknown/duplicate/cancelled/old-epoch tokens, and a token/event-
    type mismatch cannot advance a clock or satisfy a Present target, while the
    active wrong-type mismatch poisons the incarnation.
71. [`COMMIT-2`, `COMMIT-5`] Clock-source probing selects `KernelSequence` only
    from current GET_SEQUENCE success. `EOPNOTSUPP`, other errors, malformed or
    stale results, timeout and executor death keep the source unresolved, close
    structural capability/qualification as applicable, and create no software
    protocol clock. Reopen and every new CRTC clock epoch begin `Unresolved`
    even for the same raw handle. Tests pin that generic event sequence zero and
    GET/QUEUE `EOPNOTSUPP` share the no-vblank core condition and are not
    independent evidence; a driver-specific non-zero event likewise cannot
    override the GET result.
72. [`SCHED`, `COMMIT-1`, `COMMIT-2`] Replacing a never-submitted primary-plane
    successor immediately releases and idles its buffer exactly once, but
    publishes its deferred `Skip` only after the in-flight predecessor's
    completion. Repeated replacement and teardown cannot duplicate either
    half, and Warframe-shaped producer pressure does not exhaust client buffers.
73. [`SCHED`, `COMMIT-2`] Retirement-time successor promotion enters the named
    owner tier, preserves the merged immediate dispatch instant, serializes a
    fresh event token plus canonical out-fences, and absorbs only compatible
    changed persistent cursor/gamma generations. It omits unchanged cursor state
    and every coordinate-only intent. No direct event handler issues a plane-
    only commit.
74. [`COMMIT-2`] Relative-idle and absolute-target sequence arms allocate fresh
    typed tokens. Scrapped, successor-displaced, skipped, old-epoch and
    consumerless arms cannot advance either clock; a valid absolute wake during
    an in-flight flip establishes only `ClockSample`/`PresentDueWake`, never KMS
    commit completion or buffer release.
75. [`CAP-2`, `MULTI`] `non-desktop` connectors are excluded from protocol
    membership, a non-desktop-only card remains outside the model, a runtime
    property flip rebuilds the domain, property-read failure remains fail-open,
    and completion-evidence discovery remains fail-closed.
76. [`INV`, `CURSOR-LIFECYCLE`, `REC-4`] VT suspend performs no best-effort
    legacy cursor hide. Every visible cursor plane is detached in the
    authoritative all-off atomic closure or by a canonically completed owner-
    ordered predecessor; detach failure closes readiness and preserves its
    framebuffer/quarantine truthfully.
77. [`CAP-1`, `CURSOR-PAYLOAD`, `SCHED`] NVIDIA policy validation covers the
    executor atomic host-call characterization, shipping software cursor, a single-site
    development-only yserver legacy-HW policy edit, and C.0 atomic HW on the
    same workload, all against the exact same stock published NVIDIA module.
    It additionally runs `SynchronousAtomicMove` under continuous composed and
    direct primary traffic even if that transport is not selected, proving its
    slot occupancy, completion, `EBUSY`, host-call, coordinate-rate and primary-
    FPS behavior. The exact driver/kernel/GPU cohort and audited module source
    are recorded; a version change invalidates the result rather than inheriting
    a family-wide classification.
    The required NVIDIA coordinate arm is `SynchronousAtomicMove` under continuous composed and
    direct primary traffic; neither omitted/absorbed legacy-move phase quotas nor
    a patched fast-hook arm apply to this stock cohort.
    Results apply only to the tested stock driver/kernel/GPU cohort. No patched,
    proposed, out-of-tree or unreleased NVIDIA module contributes evidence. No
    production environment, CLI or config lever selects an arm; failure justifies
    a degradation prior bounded by the tested driver version, without rewriting
    discovered structural capability and without preventing any other device from
    selecting `AtomicHardware`.
78. [`INV`] Production builds contain no C.0 rollout or diagnostic gate. Every
    injected capability, errno, fence/event stall, and executor fault is
    reachable only through `#[cfg(test)]` or a
    test-only backend; hardware-only source edits are documented and absent
    from the submitted diff. No patched DRM module is a qualification or merge
    input.
79. [`COMMIT-5`, `COMMIT-7`] Every host-call class uses the process-isolated
    executor. Synthetic cases prove `Submitting` or `CoordinateSubmitting` and
    the fd lease exist before IPC; explicit rejection, accepted-stale success
    and acceptance-unknown remain distinct; the two- and 30-second watchdogs
    enter the specified quarantine; IPC death or a helper that outlives
    termination enters `ExecutorStalled`; and neither resource release nor a
    fresh incarnation precedes actual reap. Logical VT/device-loss obligations
    remain prompt, while orderly physical exit prefers reap and falls back to a
    bounded exit at its teardown deadline.
    No test path permits a worker-thread, in-process, driver-specific or
    call-class-specific executor alternative.
80. [`COMMIT-5`, `SCHED`, `CURSOR-PAYLOAD`] The latency/concurrency recorder
    performs no measured-path filesystem I/O, allocation, flush or additional
    supervisor IPC and cannot wrap its checked preallocated record buffer. The
    required executor IPC is itself measured. On each stock cohort nominated for
    `OwnerMediatedLegacyMove`—the required Raphael iGPU in this matrix—for each
    composed/direct production-omitted stratum, qualified initial attempts reach
    at least 100,000 and
    every dispatch-to-`HardwareComplete` phase decile reaches 5,000. Non-overlap
    and retries cannot pad those counts. Each retry links to one initial
    `EBUSY`, follows that primary's completion, and precedes the next primary
    admission. An overflow,
    reaching the 250,000-cycle or 250,000-attempt stratum cap with incomplete
    coverage, an underfilled stratum/decile, zero interval, extra initial
    attempt, or ordering breach fails with its specified evidence or normative
    outcome. Optional absorbed characterization is labelled separately and no
    count from it can satisfy or fail this gate.
81. [`CAP-1`, `CAP-4`] `atomic_kms_cursor_policy` is runtime-derived: a device
    with structural capability and incarnation qualification selects
    `AtomicHardware` with no table entry of any kind, and a device lacking
    either selects `SoftwareComposited`. Table-driven cases prove no environment
    variable, command-line flag or configuration key reaches the value, that a
    demotion changes only that device identity's policy and never the advertised
    structural-capability bit, and that a second device on the same driver is
    unaffected by the first device's demotion.
82. [`CAP-4`, `CURSOR-LIFECYCLE`] Measured demotion requires the conjunction.
    Injected windows prove that a depressed `CursorServiceRate` alone does not
    demote, that an over-bound cursor host-call p99 alone does not demote, and
    that only both together across the required consecutive windows do. The
    tier-3 absorption case — a healthy device whose cursor rate is pinned by a
    slow client while its host-call p99 stays inside `CursorHostCallMax` — never
    demotes, and a window whose atomic slot was held by primaries unrelated to
    the cursor is discarded rather than counted. Demotion executes through the
    section 11.4 transition as an explicit policy change, emits no duplicate
    sprite, and cannot reopen within the process even across VT, DPMS, topology
    and incarnation replacement.
83. [`CAP-4`] The degradation prior is a starting posture, not a verdict. A
    matching entry starts the device in `SoftwareComposited` with no measurement
    window; an installed driver version above the entry's recorded bound does not
    match and the device starts optimistically; a missing or stale entry is
    indistinguishable from unknown hardware and is still protected by measured
    demotion. No path promotes a device out of a matching prior by measurement,
    and no synthetic cursor probe is inserted to attempt it.

### 16.3. Hardware validation

Merge-required physical evidence is the full passing C.0-ready matrix on the
author's Raphael iGPU (Ryzen 7 7700, RDNA2/DCN 3.1.5, `amdgpu`, PCI `1002:164e`)
plus the host-call probe, lifecycle/completion soak, and NVIDIA policy matrix on
the author's RTX 5060 Ti (GB206/Blackwell). Both devices are in one machine, so
no campaign depends on hardware the author does not own.

This revision replaces the previously required Radeon Raphael iGPU. The
substitution is deliberate and is **not** inherited evidence: Navi 21 is DCN 3.0
and Raphael is DCN 3.1.5, and every gate in this section exercises display IP
rather than shader IP. Cursor-plane behaviour, hotspot support, off-transition
fence delivery, LUT sizes and the atomic-check paths all live in that differing
code. The Raphael cohort therefore runs a complete new matrix, exactly as this
section already requires of any replacement board. In particular the
`AuditedCursorExpansionHazard` source audit and the `OwnerMediatedLegacyMove`
coordinate quota were written against Navi 21/DCN 3.0 and must be redone against
DCN 3.1.5 before that transport is allowlisted for this cohort. The historical
Polaris/RX 580 captures remain provenance only.

An executor, owner-completion, poison, watchdog, or off-transition-fence failure
on either device invalidates the shared design and blocks merge. Those are
architecture and completion-safety properties, not cohort properties.

No cursor-policy outcome blocks merge, and no device's unavailability produces
`EvidenceInsufficient` for the release, because under `CAP-4` no cohort depends
on a campaign in order to enable hardware cursor. Intel, Asahi, other AMD
generations and other NVIDIA cohorts need no campaign at all: they select
`AtomicHardware` optimistically at runtime and are protected by measured
demotion. A campaign is required only to allowlist `OwnerMediatedLegacyMove` for
a cohort, or to justify a degradation prior and its driver-version bound.

Evidence ownership is explicit: the author owns and schedules both campaigns.

NVIDIA first runs this four-arm gate on the same GPU, mode, desktop, stock
published module and XFCE/Thunar drag workload. Under `CAP-4` the gate no longer
decides whether this cohort may enable hardware cursor; it decides whether the
measured demotion catches this driver and whether the cohort earns a degradation
prior. No patched or proposed driver build is an arm:

| Arm | Construction | Question answered |
| --- | --- | --- |
| Executor atomic host-call characterization | The release-shaped C.0 build measures helper ioctl duration, IPC dispatch-to-reply, total input-to-dispatch overhead and `SynchronousAtomicMove`; there is no production `OwnerMediatedLegacyMove` phase quota on stock NVIDIA. | Does the fixed executor keep the X11 core responsive with acceptable IPC cost while exercising every stock NVIDIA atomic class? |
| Legacy hardware cursor | One documented development-only yserver source edit changes only the NVIDIA policy constructor to the normal legacy-HW path; the stock module is unchanged and the edit is never merged. | What is the magnitude of the driver's cursor wait on this cohort through the helper's ordinary fallback? The historical 11.5 ms mean / 16.3 ms max was an X11-core block; under the fixed executor the same wait is contained in the helper and structurally cannot reach the core, which is section 4.1's intended effect and not a measurement failure. The magnitude is consumed by the `SynchronousAtomicMove` expectation below and by section 2's justification. This is regression scale, not fast-hook qualification. |
| Software composited cursor | Unmodified merged baseline and shipping NVIDIA policy. | What smoothness, input latency and direct-scanout reachability does C.0 actually have to beat? |
| C.0 atomic hardware cursor | C.0 owner build using the required `KmsIoExecutor` and stock NVIDIA module, with no runtime override. Its only hardware coordinate transport is `SynchronousAtomicMove`; it is recorded under continuous composed/direct primaries even if software remains selected. | Does synchronous atomic motion make hardware cursor acceptable while unlocking direct scanout and preserving maintenance fairness, and what completion/slot cost does it impose? |

The atomic arm must beat the legacy arm's core stall and be no worse than the
software arm's drag smoothness/input latency within the recorded confidence
range, and must satisfy the direct-successor FPS and owner bounds below. If it
does, this cohort takes no degradation prior and runs hardware cursor like any
other device. If it does not, the recorded result justifies a prior entry
bounded by the tested driver version, and the same run must show the measured
demotion firing on its own — an arm that is subjectively bad while
`CursorServiceRate` and the cursor host-call p99 stay inside their thresholds is
a threshold-calibration failure and blocks the prior until the thresholds are
corrected.

The expected stock-NVIDIA result is confirmation of the fallback:
`SynchronousAtomicMove` is vblank-paced and occupies the sole atomic device
slot, so at low client frame rates tier-3 absorption pins hardware cursor
updates to client cadence and is unlikely to match the software arm's drag
smoothness and input latency. The four-arm gate attempts to refute this
prediction; confirming it is a valid cohort-local result, not a shared-design
failure.

- idle desktop with continuous cursor motion;
- 1000 Hz mouse motion and circular/diagonal movement;
- visible cursor over composed desktop and fullscreen direct scanout;
- animated cursors, image/name changes, hotspot changes, XFixes hide/show, and
  `Cursor=None` followed by restore;
- drag operations under MATE/Cinnamon without window lag;
- multi-output crossing, including different device owners where available;
- instrumented cross-device crossing verifies detach completion precedes attach
  submission and forced destination failure produces no duplicate sprite;
- Alt-Tab/direct unflip/re-entry;
- VT switch, DPMS cycle, hotplug, and shutdown;
- zero legacy cursor load/show/hide/disable calls on C.0-ready devices;
  in production C.0 arms, coordinate-only `MOVECURSOR` is separately counted
  and must occur only on a qualified owner-mediated plane; it may begin while
  the atomic slot is idle or held by the one accepted coordinate-overlap-safe
  primary class and retains its per-plane `CoordinateSubmitting` reservation through
  typed return or helper reap. The explicitly non-production NVIDIA legacy-HW
  scale arm is reported separately and cannot satisfy this rule;
- zero sustained `EBUSY` storm and no cursor freeze, trail, flash, or duplicate
  on an idle scene. Every exact cohort with qualified
  `OwnerMediatedLegacyMove` must preserve its shipping legacy latency;
  synchronous atomic motion is accepted only where its own composed/direct
  concurrency, completion, FPS and platform-baseline gates pass;
- the measured demotion mechanism itself, on both devices. Calibrate
  `DemotionRatio`, `CursorHostCallMax` and the consecutive-window count against
  the four-arm results; prove demotion fires on a cohort that needs it, executes
  through the section 11.4 transition with no duplicate sprite, flash or trail,
  and never reopens within the process. The mandatory negative case is the
  tier-3 absorption scenario: a healthy device under a deliberately slow client
  depresses `CursorServiceRate` while the cursor host-call p99 stays inside
  `CursorHostCallMax`, and must **not** demote. A demotion there is a
  false positive and blocks merge, because it would silently withdraw hardware
  cursor from correct hardware — the exact failure this revision exists to
  remove. Also prove a degradation prior is skipped when the installed driver
  version is above its recorded bound, and that neither table is reachable
  through any environment variable, flag or configuration key;
- on the Raphael iGPU, production builders prove every primary constructed while
  `OwnerMediatedLegacyMove` is selectable satisfies
  `NativeCursorCompositionContract`. Direct eligibility rejects scaled,
  partial-coverage, non-XRGB8888 including YUV/video and HDR/10-bit candidates
  before any KMS request. The construction suite also proves that composed
  `VkScanoutFb` registration and direct eligibility both remain XRGB8888, that
  an out-of-contract construction invalidates the transport before dispatch,
  and that re-entry requires detach/reattach plus complete plane-incarnation
  requalification with a contract-valid primary already in the state.
  Instrumentation records the exact userspace object set
  separately from `AuditedCursorExpansionHazard`, which is explicitly a
  conservative source-derived prediction. Table-driven construction tests cover
  every audited AMD hazard reason, including gamma/CTM/degamma, modeset, VRR,
  DSC-force and the plane-local triggers. Hardware gamma, modeset and primary
  traffic proves coordinate calls never overlap a hazard-classified accepted
  record. The documented nonmergeable raw-KMS harness may install an out-of-
  contract shape to demonstrate AMD's cursor-overlay and ordinary slow paths;
  those calls are mechanism characterization and cannot satisfy production
  evidence;
- for each required stock-driver cohort that nominates
  `OwnerMediatedLegacyMove`—currently the Raphael iGPU, not NVIDIA—100,000 phase-
  qualified production-omitted initial coordinate attempts in each
  composed/direct stratum,
  with at least 5,000 samples in every normalized dispatch-to-
  `HardwareComplete` decile. Record non-qualifying attempts, `EBUSY` ratio,
  longest run, effective updates/s and each linked retry. Any absorbed-shape run
  is optional characterization and cannot fill or fail a quota.
  Production ordering remains one retry after primary completion and before the
  next primary admission; retry samples never pad the overlap quota. An
  underfilled stratum/decile, exhausted phase cap, or fixed-buffer overflow is
  `EvidenceInsufficient`;
- a Warframe-shaped high-rate direct stream proving displaced successors idle
  immediately, deferred `Skip`s follow predecessor completion, each buffer is
  released once, and request/idle throughput does not collapse to refresh;
- synchronized direct successors at continuous load proving retirement-time
  promotion absorbs only changed compatible cursor/gamma state and neither class
  starves behind a self-refilling primary stream;
- the `c09358a1` VT scenario, historically reproduced and fixed on Polaris/RX
  580, replayed as a required gate on the current Radeon Raphael iGPU, proving
  cursor-plane detach is part of the canonical owner/all-off sequence and no
  legacy best-effort hide remains; a best-effort future Polaris replay is
  supplementary only;
- `xrandr --gamma`/RANDR SetCrtcGamma and GetCrtcGamma round trips, identity and
  non-identity ramps, repeated rapid changes, DPMS/VT/hotplug persistence, and
  zero legacy gamma ioctl calls on C.0-ready devices;
- on a deliberately gamma-less CRTC or test-only equivalent, `redshift`,
  `gammastep`, and one Proton title exercise `GetCrtcGammaSize=0`; any crash,
  divide-by-zero, unrecoverable loop or material regression reopens the design;
- cursor-only and gamma-only out-fence completion on each driver, multi-CRTC
  partial-fence plus tagged-primary-event accounting, qualification/readiness
  closure on a deliberately unsupported topology, and VT/device-loss teardown
  with no premature framebuffer or blob destruction;
- continuous primary traffic on one CRTC while moving the cursor, changing
  gamma, and presenting synchronized work on another CRTC, proving the C.0
  admission bounds and absence of starvation without depending on C.1;
- cross-device attach failure while the destination owns direct scanout,
  proving unflip precedes reported software visibility and direct scanout stays
  inhibited through recovery;
- injected atomic accept/reject and userspace fence/event wakeup reordering
  proving exact fd ownership, separate typed Present completion, prior-buffer
  release, and no framebuffer/blob/FOREIGN retirement from the wrong milestone;
  the test also proves both signals originate from the same mainline
  `drm_pending_vblank_event` and treats reordering as defensive observation;
- injected zero/unknown `EventToken`, current tagged zero/unexpected `crtc_id`,
  stale-generation and old-incarnation events, duplicate, contradictory-current,
  malformed, and missing page events proving the telemetry/poison boundary,
  no collision after tombstone eviction, and bounded tombstone behavior;
- mandatory initial and later real restore/install qualification plus forced
  timeout/failure on both drivers, proving no synthetic live-state probe is
  inserted, readiness remains closed until success, and capability is not
  derived from a driver allowlist;
- virtual/para-virtual cursor-plane coverage where available, verifying hotspot
  negotiation before enumeration, required paired properties, unmodified
  metadata, and deterministic failure; on NVIDIA/AMDGPU the expected
  `EOPNOTSUPP` path retains native coordinate-only hardware cursor;
- capability injection for `DRM_CAP_CRTC_IN_VBLANK_EVENT` and
  `DRM_CAP_TIMESTAMP_MONOTONIC`, proving a zero/query failure closes structural
  capability before exposure and an active tagged event with zero `crtc_id`
  poisons immediately after successful capability discovery;
- raw page-event replay around the `u32` sequence wrap and across a CRTC clock
  epoch change, proving normalized 64-bit MSC/monotonic UST never regress or
  leak between CRTCs/epochs; inject delayed `DRM_EVENT_CRTC_SEQUENCE` events to
  prove only a fresh sequence-arm token matching CRTC, epoch, purpose, target,
  and event type can advance the reference;
- injected and, when available, real GET_SEQUENCE `EOPNOTSUPP` proving C.0
  structural capability and qualification close, no software protocol clock or
  sequence arm is created, and source/state-machine evidence alone cannot enable
  that cohort;
- injected producer, out-fence, and page-event stalls at each deadline boundary,
  proving source-specific pre-submit producer handling, correct post-accept
  timeout classification, and Present `Skip` without premature idle/release;
- equal-refresh multi-CRTC Present proving each included CRTC receives its own
  event/fence evidence, a pre-arrived event needs no timer, and any remaining
  expiry poisons the whole transaction exactly once; separate mixed 60/240 Hz
  and unknown-period topology changes prove orderly owner quiesce, capability
  false, and merged-baseline installation without a C.0 combined commit;
- injected readable out-fences with pending, negative, and query-failure status,
  including mixed multi-CRTC results, proving only successful canonical status
  retires hardware;
- VT/DPMS, topology epochs, and fd reopen after a completion poison, proving the
  advertised bit remains stable, epochs cannot clear the poison, and only a new
  qualified incarnation reopens readiness;
- injected accepted cursor-detach completion loss on same- and cross-device
  transitions, proving `HwDetachUnknown` never reveals SW or attaches the
  destination, no further KMS commit reaches the poisoned fd, and autonomous
  recovery requires teardown plus a freshly proven state;
- injected normal-runtime completion failure with no VT/DPMS/hotplug, plus
  reopen/discovery/test/install failure at each recovery stage, proving immediate
  owner recovery or deterministic `RecoveryFailed` logical withdrawal/RANDR
  failure without a reopen loop;
- recovery while a route-probe helper deliberately holds a duplicated KMS fd,
  proving the helper is cancelled/reaped, every lease closes, and no fresh
  incarnation is created early;
- every lifecycle-precedence race while normal recovery is active, including
  shutdown/removal/device-replacement/VT acquire-release/reprobe/hotplug/
  topology/DPMS permutations, proving one arbiter owns teardown/reopen,
  quarantine transfers, and no superseded transition publishes;
- the complete `REC-6` winner/recovery cross-product, proving old versus fresh
  `RecoveryId` fate and attempt accounting at real VT, hotplug, reprobe, topology,
  and DPMS boundaries;
- topology/DPMS/seat storms during every higher-priority transition, proving
  storage stays bounded by protocol outputs, equal events merge by type, final
  installation consumes a fresh desired snapshot, and every event id obtains a
  truthful disposition without replaying stale DRM objects;
- DPMS/topology changes arriving while VT is released, proving they remain one
  prerequisite-deferred desired state, converge after reacquire, or are
  invalidated exactly once by removal/shutdown;
- a deliberately delayed executor ioctl concurrent with VT release and device
  removal, proving request/input dispatch remains responsive, neither lifecycle
  event waits for the host call, explicit stale rejection differs from accepted-
  stale success/unknown, and the fd lease prevents premature retirement;
- executor reply/page-event reordering plus forced IPC death before and after
  ioctl entry, proving `Submitting` precedes dispatch, stages early events,
  occupies the only slot, and never converts lost acceptance into rejection;
- a host-call helper that ignores termination long enough to cross its watchdog,
  proving `ExecutorStalled` withdraws outputs without blocking the core and
  forbids fd retirement, resource release, reopen, or retry until actual reap;
- orderly shutdown with delayed and simulated-uninterruptible executor helpers,
  proving client/seat shutdown is prompt but the parent remains as teardown
  supervisor with quarantine intact until wait status, after which fd,
  Vulkan/GBM/shared teardown and process exit occur in order; the uninterruptible
  case additionally runs past the teardown deadline and past a service-manager
  stop timeout, proving the parent exits with the unreaped lease recorded and
  that a restart attempted while the orphaned helper still holds the device lock
  waits or refuses before installing any state;
- enable, disable, connector/plane move, detach, and inactive-to-inactive
  commits proving the exact old/new atomic CRTC closure, one canonical out-fence
  per expected powered CRTC, and one userspace page event per such CRTC only
  when `PAGE_FLIP_EVENT` is set; rejection of an off-to-off CRTC carrying either
  that global flag or a local `OUT_FENCE_PTR`; acceptance only when that off-to-
  off member has neither kernel signaling source; draining non-Present CRTC
  events without protocol completion; no vacuous disable completion/
  qualification; and no misuse of flip out-fence as full device teardown
  evidence;
- real DPMS-off, VT all-off and CRTC-disable loops proving every old-active
  `ExpectedCompletionCrtcs` fence signals without a future physical vblank;
- cold-start/offline blocking and seat-active nonblocking real install/restore
  commits, plus a transaction with empty `ExpectedCompletionCrtcs`, proving
  qualification always consumes canonical out-fence evidence, never succeeds
  vacuously, and never blocks the active X11 core or VT/device-loss handling.
- a deliberately slow DP/MST/HDMI sink or controlled equivalent during hotplug,
  modeset, install and recovery, establishing
  `LifecycleCompletionObservedMax`, exercising the lifecycle-specific deadline,
  and proving a healthy completion beyond the fast two-second clamp does not
  poison the incarnation.

Without fault injection, the Raphael iGPU and RTX
5060 Ti each run a separate eight-hour seat-active soak containing
continuous desktop use, cursor motion, direct/composed transitions, periodic
DPMS, VT cycles and repeated fullscreen entry/exit. The release budget is zero
incarnation poison, zero `HwDetachUnknown`, zero executor/host-call watchdog
expiry, and zero missing/failed off-transition fence. The Raphael iGPU runs the
C.0-ready hardware-cursor policy. The RTX 5060 Ti runs the required executor
and complete owner/lifecycle path with its shipping software
cursor if the atomic-HW arm does not qualify, or with atomic HW if it does.
Any occurrence blocks C.0 merge because these are shared architecture and
completion-safety gates; it is not averaged into a cohort rate. Only failure of
the NVIDIA atomic-HW policy/performance comparison is cohort-local.
The soak additionally permits zero qualified coordinate return above
`CoordinateFastReturnMax`. Such a return no longer threatens X11-core
responsiveness because the ioctl is isolated, but it proves that the nominated
fast transport entered an ordinary blocking path. It closes
`OwnerMediatedLegacyMove`, stops the affected campaign, and fails the affected
AMD coordinate-policy, performance and soak rows. A repair reruns every row
made reachable by its section 18 dependency manifest, including both AMD phase
strata and quotas plus the complete AMD soak when coordinate policy or
construction changed. There is no count-based slow-return allowance.

For capacity planning only, at 60 Hz `PhaseCycleCap = 250,000` represents about
69 minutes 27 seconds per stratum; the two required composed/direct production-
omitted strata represent about 2 hours 19 minutes on the required Raphael iGPU at
their caps. Adding the two final eight-hour soaks gives about 10 hours 19
minutes on AMD and 8 hours on stock NVIDIA, or 18 hours 19 minutes of dedicated
device time across both, before setup and the remaining validation matrix.
Concurrent machines reduce wall time but not device time. Earlier quota
completion may shorten an AMD stratum; this accounting never permits a shorter
soak or alters an evidence threshold. Optional absorbed-shape characterization
is outside this gate and budget.

Record a reproducible before/after performance table using the current shipping
baseline and C.0 on identical hardware, modes, and workload. On a cohort whose
shipping baseline is legacy HW cursor, that exact transport is the comparator;
on the required NVIDIA cohort the comparator is the software arm above, with the
legacy-HW arm retained as historical-regression scale. Input-to-visible motion
uses the same external high-speed-camera or
scanout-sensor method and input marker in both builds; ioctl return is not a
visibility proxy. The table must contain at least:

1. input-to-cursor-submit, input-to-cursor-retirement and externally correlated
   input-to-visible-motion p50/p99 plus effective coordinate updates/s;
2. sustained physical device commits/s and logical retired generations/s per
   CRTC with `N=1` and homogeneous-qualified `N=2` active CRTCs;
3. direct-scanout FPS with an idle cursor and continuous cursor motion;
4. helper-measured ioctl and executor IPC p50/p99/p99.9/max for every section
   4.1 class, the count of dispatch-to-reply excursions above
   `ExecutorTransportExcursionCeiling` with their fraction of that arm's
   samples, total input-to-dispatch overhead,
   production-omitted coordinate overlap/actual-phase-decile counts, retry
   ordering, both composed/direct phase caps, fixed-buffer capacity and high-
   water; any absorbed-shape run is labelled optional characterization;
5. executor dispatch/reply/watchdog/reap totals, maximum lease duration,
   `ExecutorStalled`/`ShutdownExecutorStalled` outcomes, exact kernel/module/
   source identity and any evidence-insufficient reason; and
6. the RTX 5060 Ti `SynchronousAtomicMove` composed/direct arm's slot occupancy,
   completion, `EBUSY`, coordinate rate and primary FPS, whether or not that
   transport is selected.

For `N=1`, continuous cursor motion must retain at least 95% of the idle-cursor
direct-scanout FPS, sustained completed commits/s must reach at least 90% of the
active refresh rate, input-to-submit p99 must not exceed one output period plus
2 ms, and input-to-retirement p99 must not exceed input-to-submit p99 plus two
output periods and 2 ms. *Submit* is the instant the owner installs the
`Submitting` or `CoordinateSubmitting` record and dispatches the request to its
executor, consistent with sections 4.1 and 10.2; it contains neither the
executor round trip nor the ioctl. Those are bounded by the three transport
criteria below, and input-to-retirement covers the complete path end to end.

Each transport criterion names what a violation is attributable to.
`ExecutorTransportP99Max = 50 us` and `ExecutorTransportP999Max = 100 us` bound
dispatch-to-reply latency for every section 4.1 call class, measured with that
class's own helper ioctl duration excluded. Exceeding either fails the affected
performance row on that device. Both figures carry roughly five times margin
over a measured basis rather than being axiomatic: dispatch-to-reply p99 of 5 to
9 us under CPU load on a Ryzen 7 7700 class host at the `powersave` governor,
recorded in the finding cited in section 4.1. A cohort host whose CPU class is
materially slower records its own basis and bound by the same route the cohort
kernel range uses, with the derivation stated; it does not silently inherit a
number measured elsewhere.

`ExecutorTransportExcursionCeiling = 500 us` is a characterization ceiling, not
a tolerance budget. It applies to dispatch-to-reply with that class's own
helper ioctl duration excluded, on the same basis as the two bounds above; a
lifecycle class whose ioctl legitimately runs for hundreds of milliseconds
therefore produces no excursion from its own duration. An excursion above the
ceiling is transport and scheduling and is never attributable to the driver or
to the cohort; it records
the excursion with its scheduling evidence and does not by itself fail a row.
Excursions above the ceiling in more than 0.01% of an arm's samples fail that
arm's performance row, so consequence is proportional to frequency and no
isolated excursion carries a disproportionate one.

`ExecutorSchedulingSaturation` is an evidence-validity check, not an
architecture gate. A dispatch-to-reply p99 at or above one millisecond measured
with that class's own helper ioctl duration excluded, or an input-to-dispatch
p99 at or above one millisecond, means the measurement host had no timeslice to
give; an in-process core would have been equally starved, so the result is not
attributable to
`KmsIoExecutor` and cannot fail the architecture. It makes the affected arm
`EvidenceInsufficient` and requires a rerun on a host that is not CPU
saturated.

The 95% gate is a measured steady-state invariant: changed persistent cursor
state is absorbible, unchanged cursor state is omitted, and qualified
coordinate-only movement consumes no atomic slot while a contract-preserving,
no-hazard primary is pending. Telemetry must show that cursor-maintenance admissions
consume less than 5% of primary opportunities; otherwise the gate fails rather
than being excused as incompatible work.

On any cohort whose shipping baseline has a legacy immediate cursor path, the
qualified C.0 transport's input-to-visible p99 may regress by at most 5% and its
effective coordinate updates/s must retain at least 95% of that baseline under
the same 1000 Hz input and mode. Both comparisons run separately under
continuous composed-primary updates and continuous fullscreen direct-primary
updates; an idle-desktop result cannot satisfy either gate. Submit/return timing
alone cannot satisfy this gate.

For `N=2`, the throughput gate applies only to a qualified
`HomogeneousCompletionGroup` whose two active CRTCs have the same exact mode-
derived refresh rational. Report physical commits separately from logical
retirements. First measure `R1`, the sustained accepted-to-completed physical
transaction rate from the matching `N=1` run on the same device, mode and
workload. Define `SingleSlotCeiling = min(R1, refresh_1 + refresh_2)`. The
compatible `N=2` workload offers each CRTC distinct, monotonically numbered
primary generations at no less than 110% of that CRTC's refresh rate, uses a
60-second warmup, and measures a continuous ten-minute window. A logical
retirement is counted once, on the first canonical completion that proves a
particular workload-issued generation for that CRTC. Re-observing unchanged or
carried state, a no-op, `Skip`, rejected request, or superseded generation does
not count. A combined transaction counts one physical commit but at most one
new logical retirement for each CRTC whose distinct generation it first proves.
Each active CRTC must retire at least `0.45 * SingleSlotCeiling`, and their
logical-retirement sum must reach at least `0.90 * SingleSlotCeiling`. Bundle
gains above that accepted single-slot floor are reported but are not required.
Starvation, insufficient offered load, or substituting physical device commits
for logical per-CRTC progress fails the gate. This named
`SingleSlotMultiCrtcCeiling` is an accepted C.0 limitation; a future independent
**Multi-CRTC Parallel Retirement** design, not C.1, may lift it. Raw captures
and exact commands belong in the PR evidence. A mixed-refresh or unknown-period topology instead proves that
C.0 multi-CRTC capability remains false, the C.0 owner is quiesced before the
change, and the merged Phase A+B backend preserves functional Present, cursor,
DPMS, VT and hotplug behavior; it cannot claim the homogeneous throughput gate.

## 17. Acceptance criteria

This section is a release checklist derived from sections 5–16. It does not
redefine their terms or state transitions. C.0 is complete when:

- section 4.1's fixed `KmsIoExecutor` architecture covers every named host-call
  class with no in-process, worker-thread, driver-specific or runtime branch;
  release evidence records exact kernel/module/source identities, executor IPC
  and helper duration, fills every production coordinate phase quota without
  recorder overflow, and keeps absorbed-shape characterization optional;
- the complete visible cursor lifecycle and persistent state use universal
  cursor-plane atomic properties; the only permitted legacy transport is the
  qualified owner-mediated coordinate exception in section 7.1;
- one device-local owner orders every atomic mutation and that coordinate
  exception;
- one device-local lifecycle arbiter applies the normative precedence, so
  concurrent teardown/recovery causes coalesce or supersede deterministically,
  transfer quarantine, and cannot open or publish two replacement incarnations;
- lifecycle input converges through one bounded desired-state snapshot: typed
  equal-kind coalescing and post-transition revalidation give every event an
  exact terminal disposition (after any truthful prerequisite deferral), and a
  still-valid unsatisfied target creates at most one
  next transition without replaying stale object payloads or cloning recovery;
- every desired device-presence, seat, reprobe, topology, DPMS, and recovery
  state maps to a named transition; the total `REC-6` matrix assigns exactly one
  fate and attempt budget to an active recovery under every possible winner;
- `Submitting` or `CoordinateSubmitting` and every applicable fd lease are
  installed before executor IPC. The reservation occupies its specified slot,
  and explicit reject, accepted-stale success and acceptance-unknown remain
  distinct under every reply, event, IPC-death and watchdog ordering;
- every ordinary or transition-owned request carries the current lifecycle
  epoch; ordinary traffic uses no transition id, epoch advancement precedes
  invalidation, and stale epoch/optional-transition results cannot publish;
- every host-call class, including `ValidationOnly`, has the normative executor
  watchdog/evidence treatment. Failure to reap enters `ExecutorStalled`, prompt
  logical VT/device-loss work continues, and orderly exit preserves quarantine
  until actual wait/reap proof or its teardown deadline, after which the parent
  exits with the unreaped lease recorded and the device lock left held; signal
  delivery or IPC loss is never release proof, and no start installs state while
  that lock is held;
- each device has at most one dispatched-or-submitted nonblocking live atomic
  transaction, identified and
  retired through canonical out-fences, while tagged page events independently
  drive primary Present MSC/UST. Only one per-plane coordinate reservation may
  overlap the exact accepted contract-preserving, no-hazard primary class.
  Homogeneous multi-
  output physical commits and logical per-CRTC retirements meet the separate
  section 16.3 gates;
- cursor motion progresses independently of scene damage through its qualified
  coordinate transport or ordinary synchronous atomic path, including under
  continuous composed and direct primary traffic. Transport viability is
  measured for the exact cohort rather than inferred from vendor family;
  composed/direct state applies section 7.1's fixed preference table,
  checked construction keeps every installed primary within
  `NativeCursorCompositionContract`, complete-request
  `AuditedCursorExpansionHazard` classification prevents overlap with every
  audited driver-expansion path, out-of-contract construction invalidates the
  fast transport before dispatch and permits no same-incarnation re-entry
  without detach/reattach plus complete requalification, any returned
  over-bound call closes the
  transport for the plane incarnation, and every HW/SW change uses the ordered
  lifecycle transition;
- high-rate motion is bounded/coalesced without retry spinning or an `EBUSY`
  storm; C.0 preserves a qualified existing coordinate transport but Phase C.2
  owns its atomic/UAPI replacement and long-term fast-path decision;
- current and pending cursor framebuffer lifetimes are correct;
- unknown post-submit completion quarantines resources until a proven teardown
  barrier, and late events cannot retire a newer device generation;
- RANDR gamma uses atomic `GAMMA_LUT`, round-trips correctly, progresses without
  scene damage, and retains/destroys property blobs with correct lifetime;
- the merged Phase A+B contracts remain correct: core target equivalence,
  generic full-plane successor replacement, immediate one-shot idle/release,
  predecessor-before-`Skip` completion, direct/unflip pacing, VT, DPMS,
  hotplug, multi-output state, and shutdown. The single physical device slot
  meets the per-CRTC logical progress gates through the explicit homogeneous
  bundle tier; mixed/unknown-period topology quiesces C.0 and retains the merged
  baseline rather than running an unsatisfiable single-slot schedule;
- supported protocol domains advertise cacheable cursor/primary structural
  capability from cursor-plane coverage, coordinate transport, completion
  properties, per-CRTC event identity, monotonic timestamps and the exact
  platform cohort gate. A multi-CRTC domain additionally requires a qualified
  `HomogeneousCompletionGroup`; atomic gamma capability is independent. The advertised
  cursor/primary bit remains stable
  across transient runtime/incarnation failure, while
  mandatory real-commit incarnation qualification and per-submit readiness
  additionally check completion evidence, seat/output, generations, owner, and
  cursor-recovery state; a protocol-domain-changing topology safely recomputes
  advertisement before exposure;
- owner admission prevents a continuous single-CRTC primary stream from
  starving cursor, gamma, unflip, or another CRTC's ready primary work; global monotonic
  maintenance tickets are assigned at readiness, preserve age through
  latest-wins, age after losing one admission, and meet the normative `N - 1`
  bound;
- every synchronous admission, including retirement-promoted direct
  successors, absorbs only compatible changed persistent maintenance state,
  omits unchanged cursor planes and never absorbs coordinate intent; maintenance-
  selected admission symmetrically absorbs a compatible ready synchronous
  primary without crossing an unflip/topology barrier; a C.1 async primary
  neither absorbs nor carries cursor/gamma state;
- bounded primary categories preserve core target/coverage equivalence, use one
  generic full-plane latest-wins successor regardless of the async bit,
  accumulate composed damage, and preserve unflip barriers without an unbounded
  owner queue;
- final live topology validation uses an owner-serialized exact atomic snapshot,
  and no speculative `TEST_ONLY` result is promoted by topology generation
  alone;
- cursor clipping, signed property encoding, plane compatibility, and
  destination-specific framebuffer upload obey the checked rules in section 7;
- cursor hotspot negotiation occurs after atomic enable and before enumeration;
  success requires/programs paired metadata, native `EOPNOTSUPP` retains
  coordinate-only hardware cursor, and neither path double-applies the hotspot
  or silently omits required metadata;
- every HW→SW transition proves hardware detach before software reveal;
  accepted-unknown detach suppresses both software reveal and destination attach
  and forbids every new live commit on the poisoned incarnation until teardown
  plus fresh proof; direct scanout remains inhibited while a visible software
  cursor is required or hardware visibility is unknown;
- every C.0 producer dependency completes successfully under its
  source-specific pre-submit policy before admission, so producer
  failure/timeout is never-submitted and no unresolved C.0 `IN_FENCE_FD`
  crosses the ioctl; the sole C.1 async-direct exception transfers exactly one
  primary-plane fence, carries no out-fence, and requires its correlated event;
  output-fd ownership, copied/direct FOREIGN recovery, Present completion,
  hardware completion, and prior-buffer release obey section 10.2 independently
  under accept, reject, reordering, and unknown completion;
- completion evidence never treats live `OUT_FENCE_PTR=-1`, poll readability,
  or signalled error status as success, performs no synthetic production-state
  qualification, applies the normative per-CRTC monotonic event deadlines and
  device-incarnation poison scope, and terminalizes every Present/client FIFO
  without inventing presentation or releasing an unproven buffer;
- request construction rejects every old-inactive/new-inactive CRTC for which
  either global `PAGE_FLIP_EVENT` or local `OUT_FENCE_PTR` would create kernel
  event state; an off-to-off member carries neither, and no fence is added merely
  for symmetry;
- one length-checked raw DRM event-stream owner replaces both the compatibility
  page-flip parser and manual sequence parser, keeps `EventToken` and `crtc_id`
  independent, resolves the never-reused incarnation token to the complete
  logical record identity, rejects a zero CRTC for an active tagged commit, and
  normalizes checked monotonic UST plus wrap-extended kernel MSC in the probed
  CRTC clock epoch before Present. GET_SEQUENCE success preserves legitimate
  raw-zero wrap; `EOPNOTSUPP` closes structural capability and qualification,
  and C.0 creates no software clock or sequence arm for that CRTC;
  every queued sequence observation has a never-reused typed token bound to its CRTC
  clock epoch, so delayed or wrong-typed events cannot contaminate a new clock;
- the raw ioctl request boundary has no surviving baseline
  `IoctlReq`/`libc::Ioctl` alias and compiles with the real Linux glibc, Linux
  musl, and FreeBSD signatures; clock-source state has no device-wide or
  process-lifetime unsupported cache and begins unresolved after reopen or a new
  CRTC clock epoch;
- initial and later qualification, using blocking only at a `COMMIT-5`-allowed
  boundary and nonblocking during seat-active recovery, validates one canonical
  out-fence per CRTC in the exact old/new atomic CRTC closure, rechecks that
  closure from the final serialized request, accounts the matching kernel event
  set, and requires deadlines only for its Present-consumer subset; an empty set
  remains unqualified;
- `GAMMA_LUT_SIZE` is the sole gamma-size source, is representable and within
  the checked allocation limit before RANDR exposure or blob creation, and no
  legacy discovery or clamped cache value remains reachable;
- one global X11 DPMS level projects to every stable output under one epoch;
  same-device projections retire through an ordered atomic transition, hotplug
  inherits the current level, removal invalidates only its projection, and no
  partial loop or aggregate boolean claims completion;
- the section 16.3 performance table records all required executor/helper
  metrics and passes the single-CRTC FPS/input-latency, executor transport
  latency and excursion-frequency, accepted single-slot two-CRTC ceiling, soak
  and no-starvation thresholds; a saturated measurement host withdraws the arm
  as `EvidenceInsufficient` instead of failing it;
- RANDR gamma-unavailable replies and `SetCrtcGamma` validation reproduce Xorg's
  fixed-header, resource, lease, checked minimum-payload, size-match, and
  trailing-byte precedence exactly;
- every `CompletionUnknown` transition reaches its specified fresh-generation,
  failure, or truthful executor-stalled state without prematurely dropping
  quarantined owners; shutdown may remain physically stalled until reap while
  its logical obligations are terminal;
  normal-runtime failure initiates exactly one recovery attempt without an
  external lifecycle event, and failed/unknown recovery reaches
  `RecoveryFailed`, logically withdraws/reports outputs without claiming hardware
  disable, and waits for an authorized external retry boundary;
- incarnation retirement covers every registered fd alias and helper lease;
  fresh-incarnation numbering, poison clearing, and resource release cannot occur
  after merely closing the owner's duplicate;
- cross-device cursor movement uses proven detach-before-attach ordering, never
  displays two hardware sprites, and forces composed unflip before claiming
  software visibility when destination attach fails;
- relative and absolute sequence arms use fresh typed event identities; a
  delayed, cancelled, consumerless or old-epoch sequence cannot update a
  clock, and an absolute due wake during a flip cannot manufacture atomic
  completion or buffer release;
- connector-class policy participates in protocol-domain identity:
  `non-desktop` outputs remain excluded, non-desktop-only cards remain outside
  the owner model, and discovery uses the explicit fail-open/fail-closed split
  from section 6.2;
- VT all-off contains or follows a canonically completed atomic cursor detach;
  the `c09358a1` best-effort legacy hide is unreachable on a C.0-ready device;
- cursor policy is runtime-derived under `CAP-4`: every structurally capable and
  qualified device selects `AtomicHardware` optimistically, and only the measured
  conjunction of a depressed `CursorServiceRate` and an over-bound cursor
  host-call p99, over consecutive qualifying windows, demotes it. Tier-3
  absorption behind a slow client does not demote a healthy device. No
  environment variable, flag or configuration key selects a policy;
- the section 16.3 four-arm NVIDIA evidence decides whether measured demotion
  catches the stock published driver/kernel/GPU cohort and whether that cohort
  earns a degradation prior with its driver-version bound. It does not decide
  whether unlisted hardware may enable hardware cursor. No patched, proposed or
  unreleased module is considered; stock NVIDIA cannot select
  `OwnerMediatedLegacyMove`, and its arm instead exercises
  `SynchronousAtomicMove` under continuous composed and direct primary traffic
  even when software cursor remains selected;
- the Raphael iGPU and RTX 5060 Ti complete the required eight-hour soak with
  zero poison, unknown detach, executor watchdog expiry, or missing
  off-transition fence;
- C.0 uses `DispatchTimingPolicy::ImmediateOnRetirement`: owner admission runs
  immediately at direct retirement and submits the successor in that wake unless
  an aged maintenance identity or owed primary CRTC wins the bounded fairness
  rules. C.2 may replace this named policy; C.0 adds no deadline timer, margin,
  enable switch, environment variable, CLI flag or configuration key;
  production fault/qualification behavior is derived only from state and
  capability;
- C.0 is delivered as one PR and one squash-merge commit. Its four ordered
  implementation stages are review checkpoints, not merge boundaries. The
  section 18 evidence manifest classifies tip-sensitive physical evidence and
  reusable deterministic evidence; every required class is valid for the final
  integrated tip under that class's explicit invalidation rule;
- unsupported cursor devices use software composition and cannot enable Phase
  C.1; unsupported gamma CRTCs independently expose gamma unavailable and do
  not block Phase C.1. Neither path uses a legacy gamma or cursor visibility/
  framebuffer ioctl;
- `cargo +nightly fmt -- --check`,
  `cargo clippy --all-targets -- -D warnings`, and
  `cargo test --all-targets --locked` pass.

## 18. Implementation boundary

C.0 is delivered as one PR against the then-current `master` and squash-merged only
after maintainer confirmation. The following are ordered implementation and
review stages inside that PR, not separately mergeable PRs:

1. **Executor host-call, raw-event, identity, and evidence substrate.** Add the
   required process-isolated `KmsIoExecutor`, framing, fd passing, leases,
   reap, and the `COMMIT-7` device-scoped lock with its start-time check;
   `CommitId`, incarnation-monotonic
   `EventToken`, typed sequence-arm and clock-epoch identities; the one raw DRM
   event-stream parser; the portable raw-ioctl ABI boundary; epoch-local
   GET/QUEUE_SEQUENCE classification; validation/watchdog/evidence primitives;
   and pure state-machine/parser tests. The compatibility `Event::PageFlip`
   drain, manual `Event::Unknown` sequence parser, existing
   `IoctlReq`/`libc::Ioctl` alias, raw-CRTC/high-bit sequence encodings, and
   process-lifetime device-keyed unsupported cache are removed in this stage.
   The glibc, musl, and FreeBSD compile gates plus synthetic
   malformed/concatenated raw-event tests are required before this stage is
   considered reviewable.
2. **Device owner and merged-primary integration.** Add the device-local atomic
   slot, canonical completion evidence, bounded admission/fairness,
   direct-primary submission, `ScanoutM2State::queued_successor`, composed
   primary replacement, and their exact buffer/Present retirement rules. The
   owner precedes cursor/gamma conversion because every later transport must
   enter one established admission, completion and quarantine model; converting
   cursor first would temporarily recreate the split ownership that C.0 exists
   to remove.
3. **Lifecycle, modeset, DPMS, VT, and topology integration.** Convert every
   remaining atomic modeset/unflip/topology caller and every use of the merged
   all-output `dpms_set_outputs_active(bool)`/`kms_outputs_active` model into
   owner-held lifecycle intents. Global X11 DPMS is projected to stable output
   targets; VT release/acquire, hotplug/reprobe, off-transition fences,
   qualification, recovery, and shutdown use the single arbiter and quarantine
   rules.
4. **Atomic cursor and gamma conversion.** Add universal cursor payloads, VT
   detach, direct absorption, RANDR gamma blobs and atomic-only size discovery,
   HW/SW cursor transitions, multi-device coordination, and removal of every
   legacy gamma and cursor load/show/hide/disable call. The only possible
   survivor is the qualified owner-mediated coordinate transport.

Every stage has its focused parser/ABI, state-machine, primary, lifecycle or
cursor/gamma software tests, but stage boundaries are not merge boundaries.
Before evidence collection, the PR adds an evidence manifest containing the
exact source tip, tested source paths, dependency/build-artifact hashes,
module/kernel identities, workload, invalidation rationale and one of these
tip-sensitivity classes for every gate:

- **Tip-sensitive physical evidence:** executor IPC/helper latency and
  watchdog/reap behavior,
  production omitted-shape phase quotas, the performance table, slow-sink
  lifecycle completion, and the required soaks. A later change to any reachable
  submission, completion, scheduling, lifecycle, cursor/gamma policy,
  instrumentation, dependency, compiler output or module/kernel identity
  invalidates the affected row and requires its final-tip rerun.
- **Reusable deterministic evidence:** raw-event/parser corruption tests,
  portable ABI compile gates, RANDR validation/precedence tests and pure state-
  machine tests. Intermediate-tip results may be reused only when a documented
  diff-scope proof shows that the tested source paths, generated artifacts,
  dependencies and relevant build configuration are byte-identical or
  semantically unaffected. The cheap automated tests still run on the final tip;
  reuse avoids repeating unrelated external campaigns, not final CI.

A transport-criterion failure under `ExecutorTransportP99Max`,
`ExecutorTransportP999Max` or `ExecutorTransportExcursionCeiling` fails the
affected performance row on that device. The executor is shared architecture
rather than cohort-local, so a repair to the transport, its framing or its
instrumentation reaches every physical row through the executor path named
below. `ExecutorSchedulingSaturation` is not a failure and repairs nothing: it
withdraws the affected arm as `EvidenceInsufficient` and requires a rerun on a
host that is not CPU saturated.

Any qualified coordinate return above `CoordinateFastReturnMax` fails the
affected coordinate-policy, performance and soak rows. A repair to coordinate
policy, construction or instrumentation reruns both AMD phase strata and
quotas plus the complete AMD soak. Other physical rows are reusable only when
the manifest proves their submission, completion, executor and policy paths
unreachable from the change or byte-identical; unexplained or cross-cutting
impact invalidates every reachable tip-sensitive row.

The manifest combines the substrate, primary, lifecycle and cursor/gamma
matrices: portable builds and raw-event corruption; cold start; direct and
composed Present; successor replacement/`Skip`; sequence arms; event/fence
reordering; bounded fairness and owner drain; modeset/unflip; DPMS; VT;
hotplug/reprobe; topology inheritance; recovery and missing-fence injection;
cursor payload, clipping and HW/SW transitions; gamma discovery/programming;
multi-output; orderly shutdown; the complete performance table; and the
required eight-hour soaks. A review change invalidates only the evidence classes
whose declared scope it can affect; unexplained or cross-cutting changes default
to invalidating all tip-sensitive rows. No intermediate result may satisfy a
final-tip gate without either a valid reuse proof or its required rerun.

The one squash-merge commit changes C.0 from absent to the complete integrated
capability; no stage is a supported partial rollout or independently mergeable
state. The stage split exists only to keep the 39-thousand-line
`kms/render/backend.rs` reviewable by call-site family. No environment flag
selects a stage, and C.1 targets only the integrated result.

## 19. References

- `docs/status.md`, "HW cursor drag-lag fix" (2026-05-29)
- Normative merged Phase A+B baseline at `fc76b743`, the VT cursor-plane fix at
  `c09358a1`, and the damage-clipped repaint work merged at `02bafec3`. PR #129's superseded NVIDIA/Present
  measurements are not treated as current validation evidence.
- Post-merge adversarial review:
  `docs/superpowers/findings/2026-08-31-phase-c0-spec-vs-merged-phase-ab.md`
- Kernel-path adversarial review and approved disposition:
  `docs/superpowers/findings/2026-08-31-phase-c0-kernel-path-adversarial-review.md`
- Coordinate-concurrency adversarial review and approved disposition:
  `docs/superpowers/findings/2026-09-01-phase-c0-coordinate-concurrency-adversarial-review.md`
- Merged-code-baseline adversarial review and approved disposition:
  `docs/superpowers/findings/2026-09-01-phase-c0-code-baseline-adversarial-review.md`
- Post-incorporation signaling/returnability adversarial review and approved
  disposition:
  `docs/superpowers/findings/2026-09-01-phase-c0-post-incorporation-adversarial-review.md`
- Concurrency, evidence-satisfiability and deliverability adversarial review,
  verified corrections and approved disposition:
  `docs/superpowers/findings/2026-09-01-phase-c0-concurrency-evidence-deliverability-adversarial-review.md`
- Driver-specific coordinate eligibility review, helper call-chain correction
  and approved stock-driver disposition:
  `docs/superpowers/findings/2026-09-02-phase-c0-driver-coordinate-eligibility-review.md`
- Fixed process-isolated executor architecture decision and approved removal of
  the pre-approval selection campaign:
  `docs/superpowers/findings/2026-09-02-phase-c0-fixed-executor-architecture-decision.md`
- NVIDIA cursor policy evidence:
  `docs/superpowers/plans/2026-07-26-nvidia-cursor-sw-on-drag.md`
- Phase C.2 above-vblank cursor-motion design remains a successor-branch
  document; this branch does not claim a local path that is absent.
- Independent synchronized-scanout latency design:
  `docs/superpowers/specs/2026-08-31-deadline-scheduled-primary-plane-submission-design.md`
- Linux DRM/KMS documentation for universal cursor planes, atomic commits, and
  deprecated legacy cursor entry points
- Linux DRM color-management properties `GAMMA_LUT` and `GAMMA_LUT_SIZE`
- Linux DRM explicit-fencing properties `IN_FENCE_FD` and `OUT_FENCE_PTR`,
  including live-success versus reject/`TEST_ONLY` holder semantics
- Linux DRM cursor-hotspot properties and
  `DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT`
- Linux DRM capabilities `DRM_CAP_CRTC_IN_VBLANK_EVENT` and
  `DRM_CAP_TIMESTAMP_MONOTONIC`, plus raw `drm_event_vblank` field semantics
- Linux DRM atomic signaling rule: `prepare_signaling()` creates
  `drm_crtc_state.event` for either global `DRM_MODE_PAGE_FLIP_EVENT` or a
  per-CRTC `OUT_FENCE_PTR`; `drm_atomic_crtc_check()` rejects that event state on
  an old-inactive/new-inactive CRTC. `create_vblank_event()` writes the selected
  CRTC's object id to `event.vbl.crtc_id` unconditionally, making zero on a
  current tagged event a direct mechanism contradiction.
- Local Linux source read at `77cb8f24c2381a8abb7272d7bbdec548d6426a8a`, the
  tip of the `asahi-7.1.9-1` checkout, recorded as reading provenance rather
  than as a mainline reference. The concurrency premise is verified across
  Linux 7.1.9 through 7.2.2 per section 4.1; vanilla 7.2 was compared from a
  release tarball and the 7.2 to 7.2.2 stable delta touches no `drivers/gpu`
  file. Sources read include
  `drivers/gpu/drm/{drm_atomic_uapi.c,drm_atomic_helper.c,drm_plane.c}`
  and `drivers/gpu/drm/i915/display/intel_cursor.c`, for the exact
  holder, event, cursor-capability, and hotspot behavior, plus
  `drivers/gpu/drm/drm_atomic.c` for old/new CRTC state acquisition, validated
  by the general documentation audit. A stripped distribution kernel tree is
  not a source of record: it carries no `.c` or `.h` file and yields false
  negatives
- `drivers/gpu/drm/drm_auth.c` and `drivers/gpu/drm/drm_file.c` for the
  `COMMIT-7` placement argument: `drm_setmaster_ioctl()` returns `EBUSY` while
  `dev->master` is set, and `drm_master_release()` runs from `drm_file_free()`
  under `drm_is_primary_client()`. Both were compared across the same 7.1.9 to
  7.2.2 range as the concurrency premise and are identical in it. The seat-
  managed case is not covered by that kernel behavior because logind owns the
  `drm_file` and yserver's fd is a dup of it
- The same pinned tree's AMD display sources
  `amdgpu_dm/{amdgpu_dm.c,amdgpu_dm_plane.c}` for cursor-mode checks, async-
  check rejection, and affected-plane expansion from modeset, CRTC color
  management, VRR and DSC-force changes; `drm_atomic_uapi.c` for gamma/CTM/
  degamma `color_mgmt_changed`; and `drm_atomic_helper_check()` for the fact
  that an internal async rejection selects the ordinary legacy-update path
  rather than becoming a userspace errno.
- Stock NVIDIA open-module source `610.57.04` at
  `e4a5faa2567f28c8eabe0ebb6422b6d0abcf37eb`, whose
  `nv_plane_helper_funcs` has no cursor async callbacks. This is review
  provenance only; section 16.3 records and audits the exact stock module used
  by release evidence.
- Linux vblank core in `drm_vblank.c`: without `drm_vblank_init()`,
  GET/QUEUE_SEQUENCE return `EOPNOTSUPP` and the generic page-event sender uses
  `sequence = 0` plus monotonic `ktime_get()`; these outcomes share
  `drm_dev_has_vblank()` and are not independent qualification signals.
- Asahi kernel `asahi-7.1.9-1` at `~/Projects/linux`:
  `drivers/gpu/drm/apple/apple_drv.c` does not initialize vblank support, while
  audited commit `d15af95c52ec` makes `drivers/gpu/drm/apple/dcp.c` override
  flip-complete delivery, explicitly emit `sequence = 0`, and use its adjusted
  monotonic DCP timestamp.
- Linux man-pages `waitpid(2)`/`waitid(2)` and `_exit(2)`: child termination
  status is reap evidence and process termination closes its file descriptors;
  signal delivery alone is not substituted for either fact
- Linux man-pages `PR_SET_PDEATHSIG(2const)`: parent-death signalling is crash
  containment, may be cleared by credential transitions, and is not orderly
  child-reap evidence
- Xorg `randr/rrcrtc.c` gamma request semantics (`GetCrtcGammaSize`,
  `GetCrtcGamma`, and `SetCrtcGamma`)
- Successor spec (separate branch): Phase C.1 on-demand async direct-scanout
  tearing
