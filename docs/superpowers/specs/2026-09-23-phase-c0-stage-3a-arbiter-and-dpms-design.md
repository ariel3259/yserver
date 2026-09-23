# Phase C.0 stage 3a — the lifecycle arbiter and global DPMS

**Status:** Revision 1, written by the coordinator on 2026-09-23 under the
user's instruction to continue the stage 3 specs. The decisions marked
**(coordinator decision)** were taken without a brainstorming exchange and
are open to the user's veto before the plan is written.

**Authority:** [C.0](2026-08-26-phase-c0-atomic-kms-migration-design.md)
(§6.4 `REC-1..6`, §9.2, §10 and its lifecycle table, §10.3, §16, §18 as
amended 2026-09-22) and the
[stage 3 umbrella](2026-09-22-phase-c0-stage-3-lifecycle-design.md)
revision 5, whose sections 2 (architecture), 3a (scope) and 4 (client
contract) this spec elaborates. Neither is relaxed here.

**Baseline:** `c7d55ec0`.

## 1. Outcome

At the end of 3a:

- the lifecycle **coordinator**, the pure per-device **arbiter** and the
  per-device **lifecycle driver** exist, with the complete `REC-4` precedence,
  `REC-5` coalescing and dispositions and the complete `REC-6` recovery-fate
  matrix;
- the §6.4 device state is a type, and an Owner device moves between `Ready`,
  `Quiescing` and `Poisoned` through the driver (the exits from `Poisoned` are
  3d's);
- `Tier::Topology` carries the transition identity and has one result-
  disposition boundary;
- **global X11 DPMS on an Owner device** runs as one atomic transition per
  device — the arbiter's first production caller;
- no code path on an Owner device reads `kms_outputs_active`, and an owner
  `MechanismFailed` on an Owner device no longer ends the server.

Production devices remain `Legacy`; nothing here changes what production does.

## 2. Baseline observed

- `set_dpms_power` (`backend.rs:30965`) collapses levels 1–3 to off, skips
  when `!scanout_allowed()`, treats `want_active == kms_outputs_active` as a
  no-op, and calls `platform.dpms_set_outputs_active(bool)`
  (`platform.rs:8184`) — a loop over **every** output of **every** device.
  Off disables each output; on re-commits a modeset with the last `OnScreen`
  BO or, failing that, any BO with an fb. A partial off ends in
  `request_exit()`. Wake also re-arms the cursor, reapplies gamma and calls
  `scene.wake_for_damage()`.
- The legacy loop asks `allows_legacy(device, WriterClass::Dpms)` per output
  (`platform.rs:8242`, `:8297`). On an Owner device that is refused
  (`drm::modeset::disable_output`/`commit_modeset` return the transport-gate
  refusal), so the loop returns an error: on the off side that error reaches
  `request_exit()`, on the on side it is returned and the outputs of Legacy
  devices stay as they were. **Today a DPMS-off on any server holding an Owner
  device ends the server.**
- The conductor answers `Admitted::Topology` with
  `AdmissionOutcome::Unsupported` (`render/admission.rs:2019`); the only
  `request_topology` caller is a test.
- The closest existing class-1 dispatch is `admission_dispatch_unflip`
  (`render/admission.rs:1130`): members, description, resources, reservation,
  dispatch. Terminal states return through `admission_handle_terminal`
  (`:2079`).
- 2b already builds CRTC power changes: `CrtcPower` is retained metadata,
  `ACTIVE` is serialized only when it changes, `old_crtc_id` contributes to
  the closure, and `FencePolicy::Required` puts one `OUT_FENCE_PTR` per
  expected CRTC (`owner/closure.rs`). C.0 §10.1: an off transition's expected
  set includes the old-active CRTC, so its out-fence must signal although no
  later vblank comes.
- Owner `MechanismFailed` routes to `request_exit()` (`backend.rs:21183`).
- The core answers `DPMSInfo` from its own `state.dpms.power_level`, updated
  when the request is accepted; DPMS emits no event (Legacy golden,
  2026-09-22).
- Kernel (`~/Projects/linux`, 7.1.9): `drm_atomic_plane_check` requires a
  plane's CRTC and FB together but never that the CRTC be active; the only
  CRTC rule is `active ⇒ enable` (`drm_atomic.c:412`); helpers with
  `DRM_PLANE_COMMIT_ACTIVE_ONLY` skip planes of inactive CRTCs
  (`drm_atomic_helper.c:2992`).

## 3. Design

### 3.1. Identities and types

- `LifecycleEventId`: monotonic, checked, allocated only by the coordinator.
- `LifecycleKind`: the ten `REC-4` kinds, in precedence order.
- `LifecycleDesired`: the `REC-5` snapshot, per device, with the fields of
  umbrella §2.3; the global fields live in the coordinator (umbrella §2.2).
- `Disposition`: `Applied(transition)`, `AbsorbedByEvent(event)`,
  `AbsorbedByTransition(transition)`, `Invalidated(reason)`,
  `SupersededBy(event)` — terminal and immutable — and `Deferred(prerequisite)`,
  nonterminal. `Invalidated`'s reasons are exactly C.0's (shutdown, device
  identity loss, protocol-output removal, newer generation).
- `DeviceLifecycleState`: the nine §6.4 states. **(coordinator decision)** The
  variant names are C.0's verbatim, including `Quiescing`, for traceability;
  the distinction the umbrella requires from `TransportState::Quiescing` is
  carried by the type, and the two are never imported unqualified into the
  same module.
- The `Tier::Topology` payload: `TransitionTag { incarnation, epoch,
  transition }`.

### 3.2. The arbiter is pure and total

The arbiter takes `(LifecycleDesired, active transition, §6.4 state,
recovery incident)` plus one input — a projected event or an acknowledged
outcome — and returns the next state plus a list of actions. It implements
every row of `REC-4`, `REC-5` and `REC-6` for all ten kinds, **including kinds
that 3a never executes**. For those, 3a has no production event source (their
sources are still the Legacy VT, hotplug and recovery paths until 3c/3d); the
arbiter's claims about them are proven only by its pure tables, and 3a claims
nothing about their execution. This is deliberate: splitting `REC-4..6`
across sub-stages would let each sub-stage's partial table disagree with the
next.

### 3.3. The coordinator and the DPMS fork

`set_dpms_power(level)` splits by transport, per device:

- **Legacy devices** keep today's code, with one change: the legacy loop
  iterates only outputs of Legacy devices. An Owner device's outputs are never
  offered to `dpms_set_outputs_active`, so its refusal can no longer reach
  `request_exit()`.
- **Owner devices**: the coordinator replaces `protocol_dpms_level`, advances
  `dpms_epoch`, allocates one representative `LifecycleEventId` per affected
  Owner device, and projects the binary target (levels 1–3 are off, as
  Legacy) onto every current stable output of that device. Idempotence comes
  from the arbiter's own targets, never from `kms_outputs_active`.

`set_dpms_power` returns once the projection is recorded; hardware completion
arrives through the driver. The protocol level the core reports is unchanged
and immediate, as today.

The legacy `!scanout_allowed()` early return stays for Legacy devices. For
Owner devices the seat is a prerequisite: while the coordinator's
`seat_target` is released, the arbiter gives the DPMS representative
`Deferred(SeatReleased)`. 3a's production feed for `seat_target` is a
read-only notification from the existing `run_suspend`/`run_resume` paths;
3c converts VT itself.

### 3.4. The DPMS commit **(coordinator decision)**

DPMS changes power, not configuration:

- **Off** is one atomic commit per device that serializes `ACTIVE=0` for every
  CRTC of the device's powered-on projected outputs, keeping `MODE_ID`,
  connector routing and each primary plane's `FB_ID`/`CRTC_ID` untouched.
  Every such CRTC is in `ExpectedCompletionCrtcs` as old-active and carries a
  required out-fence (C.0 §10.1).
- **On** serializes `ACTIVE=1` for the same CRTCs over the retained state.
- `ALLOW_MODESET` is set; the request passes the final `TEST_ONLY` first, like
  every live class-1 commit. There is **no per-output fallback**: if the
  combined shape is rejected, the transition fails as a whole (section 3.7).

The alternative, a full disable that detaches planes and clears `MODE_ID`,
matches Legacy's `disable_output` but forces DPMS-on to become a full modeset
that needs a framebuffer — which is exactly how Legacy ends up relighting with
"any BO with an fb", possibly stale. Keeping the plane bound means the buffer
current at off is still current at on. Whether a given driver accepts and
completes `ACTIVE=0` with its out-fence is decided on hardware (section 5.3),
not assumed (C.0 §10.1: no advance claim about a driver not run).

### 3.5. While off

- The DPMS transition first closes admission for the device (§6.4
  `Quiescing`); it never overtakes a `Submitting` or accepted commit (C.0
  §9.2), which drains or terminalizes under §10 first.
- After off is `Applied`, the device returns to `Ready` with its outputs
  **powered off**: primary work on an off CRTC is not admissible, and composed
  offers for it wait with a new reason, `OutputPoweredOff`. The scene keeps
  running off screen.
- The buffer bound to an off CRTC's primary plane **stays current**: it is
  referenced by KMS state and is not released or reused while off.
- After on is `Applied`, readiness reopens and the next composed frame
  replaces the retained buffer through ordinary admission; the scene is marked
  for a full frame, the Owner counterpart of Legacy's
  `scene.wake_for_damage()`.
- **Cursor** (coordinator decision): an Owner device has no hardware cursor
  before stage 4 (the legacy cursor writer is refused on Owner), so 3a's off
  commit carries no cursor-plane detach. **Stage 4 obligation**: once a
  hardware cursor exists on an Owner device, the off commit includes its
  detach or follows a canonically completed owner-ordered detach (C.0 §10,
  the `c09358a1` conversion).
- **Gamma**: `GAMMA_LUT` stays in atomic state across an `ACTIVE` toggle, so
  the Owner path does not reapply it. Owner gamma is stage 4's.

### 3.6. The driver and the result boundary

The driver (umbrella §2.3.1) receives the arbiter's actions and applies them
at the owner-event routing site. For DPMS it: closes admission, requests
`Tier::Topology` with the transition tag, and — in the conductor's new
`Admitted::Topology` arm, modelled on `admission_dispatch_unflip` — builds the
section 3.4 description and dispatches. Every terminal state crosses the
result boundary (umbrella §2.4) before it reaches the arbiter:

| Result at the boundary | Arbiter input |
| --- | --- |
| current tag, `Completed` | `Applied` for the representative when every projection on the device retired |
| current tag, `FailedBeforeSubmit` | rejection (section 3.7) |
| current tag, `CompletionUnknown` | completion loss (section 3.7) |
| stale tag, explicit success | accepted-stale: fds adopted or closed once, both state sets quarantined, no promotion |
| stale tag, rejection | generation-local never-submitted cleanup |
| stale tag, absent or invalid | acceptance-unknown, quarantined |

The coordinator marks the protocol request `Applied` only when every Owner
device's representative has a terminal disposition (umbrella §2.1).

### 3.7. Failure edges

As the umbrella's rev-3 3a bullet states, and concretely:

- **Rejection** (`FailedBeforeSubmit`): the previous power state stays
  authoritative, nothing is quarantined, the device returns to `Ready`, and
  the representative is terminalized with the classified result. DPMS does
  not retry on its own; the next protocol request is a new generation.
  **(coordinator decision)** A rejected DPMS-off leaves the outputs lit, as a
  failed Legacy `disable_output` would, but unlike Legacy the server keeps
  running.
- **Completion loss** (`CompletionUnknown`): the owner poisons (existing),
  the ledger quarantines (existing), the driver moves the device to
  `Poisoned`, closes admission, terminalizes the transition and records the
  `REC-6` outcome and dispositions. While `Poisoned`, a later DPMS request
  changes only logical power state and issues no KMS mutation (C.0 §10's DPMS
  row).
- **`MechanismFailed` on an Owner device** enters the same `Poisoned` path
  instead of `request_exit()`. The branch that records a failed Legacy handover
  (`legacy_handover_failed`) and exits keeps its behavior for a device that is
  not `Owner`; the plan must show the two branches are distinguished by the
  device's transport, not by guesswork about the cause.

### 3.8. `kms_outputs_active` on Owner devices

The plan inventories every read of `kms_outputs_active` reachable while the
device is `Owner` — compositing gates, wakeup computation, relight helpers —
and routes each to the per-device power state the arbiter maintains. Reads
reachable only on Legacy devices stay. The invariant is observable: an Owner
fixture with `kms_outputs_active` forced to the wrong value behaves the same.

## 4. Client contract for 3a

DPMS has no reply and no event; `DPMSInfo` reports the protocol level. The
3a differential therefore compares, on a Legacy fixture and an Owner fixture,
the script off → `DPMSInfo` → on → `DPMSInfo` × 4, standby, suspend, off while
off, and a request while the seat is released: the core's bytes to the client
must be identical, and the backend state must show each device's outputs in
the projected power state (Owner) or `kms_outputs_active` (Legacy). The
2026-09-22 Legacy golden's DPMS steps (24–45) are the reference for what a
real client sees.

## 5. Evidence

### 5.1. Pure arbiter tables

Exhaustive: `REC-4` precedence over every ordered pair of the ten kinds;
every lower kind arriving during each active kind (C.0 §16.2 item 63);
equal-kind coalescing per `REC-5` (item 64); every unsatisfied field selecting
its kind (item 66); every winning kind × an active incident (item 67); every
event id reaching exactly one terminal disposition and never two.

### 5.2. Fixture integration

On the existing Owner fixtures: the DPMS request through the coordinator, the
arbiter, the driver, `Tier::Topology`, the dispatch and the terminal state, for
each row of the section 3.6 table; the three failure edges of 3.7 including
`MechanismFailed` not exiting; a mixed server (one Legacy device, one Owner
device) where DPMS reaches both and neither exits; `Deferred` while the seat
is released and its resolution on reacquire; the projection refresh of the
umbrella's rev-2 M-2 rule when an output appears after a global off; the
retained buffer is the same object before off and after on; and the section
3.8 invariant.

### 5.3. Hardware (user approval, tty, GPU free)

`c0_hw_3a_dpms_owner_on_card1_drm`, modelled on the Ciii/Cp hardware tests:
off/on × 4 on card1, each off transition's out-fence observed signalled with
no later vblank, the retained buffer unchanged, and composed frames admitted
again after each on — the fourth cycle as well as the first. If NVIDIA rejects
`ACTIVE=0` with a retained plane, or never signals the off fence, that is the
result: the transition must fail as section 3.7 says, and the finding decides
whether section 3.4's shape needs a driver-specific alternative.

### 5.4. Mutations the plan must name (criteria, not edits)

The plan attaches one to each invariant; at minimum: the legacy loop again
iterating Owner outputs; idempotence read from `kms_outputs_active`; an
off commit without the old-active CRTC in the expected set; a per-output
fallback after a combined rejection; a rejection treated as poison; a
`MechanismFailed` on Owner reaching `request_exit()`; a stale success
promoted; the retained buffer released while off; a composed offer admitted
on an off CRTC; the representative marked `Applied` before every projection
retired; a DPMS request while `Poisoned` issuing a KMS mutation.

## 6. Out of scope

VT, hotplug, reprobe, device add/remove as executed transitions (3c); client
modesets and the RANDR obligations (3b); recovery out of `Poisoned`,
`ExecutorStalled`, shutdown (3d); cursor and gamma on Owner (stage 4); legacy
removal (stage 5).
