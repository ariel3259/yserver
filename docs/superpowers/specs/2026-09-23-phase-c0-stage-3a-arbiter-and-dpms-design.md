# Phase C.0 stage 3a — the lifecycle arbiter and global DPMS

**Status:** Revision 5 (codex rounds
[1](../findings/2026-09-23-stage-3a-design-review-round1.md),
[2](../findings/2026-09-23-stage-3a-design-review-round2.md),
[3](../findings/2026-09-23-stage-3a-design-review-round3.md) and
[4](../findings/2026-09-23-stage-3a-design-review-round4.md)), written by
the coordinator on 2026-09-23 under the user's instruction to continue the
stage 3 specs. The decisions marked
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

- **Legacy devices** keep today's code, restricted to Legacy devices. The
  legacy loop iterates only their outputs, so an Owner device's refusal can no
  longer reach `request_exit()`. The rest of Legacy's off path is also
  server-wide today and must be restricted the same way *(rev 2, author's
  own finding while verifying round-1 M-2)*: `scene.drain_all` and
  `platform.reset_scanout_bos_for_suspend()` (`backend.rs:31087`–`31090`)
  would otherwise drain and reset an Owner device's scanout state behind its
  owner's back. The plan inventories every server-wide step of both the off
  and the on path (cursor re-arm, gamma reapply, `wake_for_damage`, vblank
  target clearing) and scopes each to Legacy devices or proves it harmless to
  an Owner device.
- **The resource service's serviced-time clock** *(rev 2, round-1 M-2)*. Today
  a successful Legacy off pauses it and a successful on resumes it
  (`set_seat_active`, `backend.rs:31099`; `resources/mod.rs:319`), and that
  clock drives pending-batch expiry. The service is shared by every device, so
  its power domain becomes **any served output lit**: the clock runs while at
  least one output of any device — Legacy by `kms_outputs_active`, Owner by
  its installed power state — is lit, and pauses only when none is. A Legacy
  off on a mixed server therefore leaves the clock running while an Owner
  output is still lit (for example after its off was rejected).
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
- **Direct scanout stays current through off** *(rev 4 — replaces the
  two-phase off of revisions 2–3; round-1 M-1, round-2 B-2/M-1, round-3
  B-1)*. Revisions 2–3 ran an Owner unflip before `ACTIVE=0`. Round 3 showed
  that unflip can wait without bound: it is ready only when exit retirement is
  vacant, a composed return is established and the direct shadow is
  materialized (`render/admission.rs:1007`), and a missing composed return is
  a waiting state, not a failure, so no deadline covers it. An `ACTIVE`-only
  off does not need it. The plane keeps its framebuffer, so the buffer current
  at off — composed **or a direct client buffer** — stays current and bound:
  - a direct buffer is kept alive by its existing direct lease: Cfb §3.4 — a
    managed framebuffer is destroyed only when the service reports it
    releasable and no lease holds it, and a dropped source drawable leaves the
    allocation to follow its own release. A client that destroys its window
    while off therefore leaves the pinned buffer in place until a later commit
    replaces it;
    - while off, the off CRTCs are ineligible for direct scanout and no direct
    successor is admissible (`OutputPoweredOff`). A client's Presents take
    the path Legacy already uses when its outputs are dark: the scanout
    blackout (`present_scanout_blackout`, `backend.rs:23360`, today
    `!(scanout_allowed() && kms_outputs_active)`). On an Owner device that
    predicate is one of the section 3.8 sites and is answered **per CRTC** from
    the arbiter's installed power state, so completion, `IdleNotify` and
    release of a Present that arrives while off follow the existing blackout
    path exactly once and never wait for an off CRTC's KMS completion *(rev 5,
    round-4 M-1)*. The older direct frame stays pinned until a proven
    replacement, as above;
    - after on is `Applied`, the device's ordinary admission resumes; a direct
    unit that is no longer eligible (or whose source is gone) returns to
    composed through the **ordinary** Ciii unflip, under normal admission.
    *(rev 5, round-4 B-1)* DPMS changes none of that unflip's readiness
    inputs — exit retirement, the retained composed return
    (`retained_composed_framebuffer` / the owner's current composed
    framebuffer, `render/admission.rs:844`), the direct shadow — so after on
    the unflip is in exactly the state it would be in had the client
    destroyed its window while lit, with no DPMS at all. 3a proves that
    equivalence (section 5.2) and claims no more: whether Ciii bounds that
    wait is Ciii's property, examined in section 6 before the 3a plan is
    written.
  The off is thus a single commit, bounded by its own deadline (section 3.6),
  and depends on no composition. No transition-owned unflip and no exception
  to the §6.4 `Quiescing` closure exist.
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
section 3.4 description and dispatches.

**Freshness is checked before submission too** *(rev 3, round-2 B-1)*. A
result-side check cannot undo a physical off. C.0 `REC-4`: "no transition may
… run final `TEST_ONLY`, or install state unless its id and lifecycle epoch
are still current", and §16.2 item 9: stale DPMS generations cannot submit.
So the conductor's `Admitted::Topology` arm compares the queued tag with the
device's current incarnation, epoch and transition **immediately before the
final `TEST_ONLY` and again immediately before executor dispatch**; a stale
entry is cancelled as never-submitted and reported to the arbiter as such. A
supersession also removes the queued `Tier::Topology` entry at once. Once a
commit is `Submitting` or accepted it is never cancelled as never-submitted;
its result goes through the boundary below.

**Deadline class** *(rev 3, round-2 M-2)*. The `ACTIVE=0` and `ACTIVE=1`
commits are §9.2 class-1 topology work, so they use C.0 §10.3's lifecycle
hardware-completion deadline — the 30 s bootstrap value, since no cohort
measurement exists — never the fast primary clamp, which could poison a
healthy but slow link retrain. The host-call watchdog is the 2 s seat-active
`NONBLOCK` one. The acceptance milestone is `HardwareComplete` from the
old-active CRTCs' out-fences; there is no Present and no page-event timer.
Expiry is `CompletionUnknown` (section 3.7). Every terminal state crosses the
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
device's representative is `Applied` or was invalidated by output removal
(C.0 `REC-5`); **a representative that ended any other way never counts
toward `Applied`** *(rev 2, round-1 B-1)*.

### 3.7. Failure edges

As the umbrella's rev-3 3a bullet states, and concretely:

- **Rejection** (`FailedBeforeSubmit`) *(rev 2, round-1 B-1)*: the previous
  power state stays authoritative and nothing is quarantined. The target is
  **not** satisfied, so the representative is not `Applied` and not
  terminal; it stays desired under the classification C.0 §10 requires:
  - an `EINVAL`/`EOPNOTSUPP` attributable to this object combination latches
    **that topology generation** (C.0 latch scopes): the device returns to
    `Ready` for other work, and the representative becomes
    `Deferred(TopologyLatched(generation))`. It is not retried under the same
    generation — no retry loop — and is revalidated when the generation
    changes or a newer DPMS request supersedes it;
  - any other explicit rejection closes readiness for the device, and the
    representative becomes `Deferred(ReadinessClosed)`, resolved by the
    lifecycle transition that re-establishes readiness (3c, 3d).
  `Deferred` is C.0's nonterminal disposition for a target that "cannot run
  while a required external state … is absent"; a latched generation and a
  closed readiness are such states, so the authority is used, not amended.
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
off, and a request while the seat is released: the core's bytes to the
requesting client **and to a second, listening connection** (rev 4, round-3
m-1: DPMS has no event, so any byte to the listener is a defect) must be
identical, and the backend state must show each device's outputs in
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
retained buffer is the same object before off and after on — composed and
direct; and the section 3.8 invariant. Added in revision 2: a rejected off (attributable and not) —
the protocol request is never `Applied`, the target stays `Deferred` with the
right prerequisite, no retry under the same generation, and a newer request
supersedes it; off with a **direct** client buffer current (rev 4) — no unflip
runs before off, the client buffer stays bound and pinned, the client destroys
its window while off and the allocation survives until replaced, no direct
successor is admitted while off, and after on the ordinary unflip returns to
composed; the **equivalence** (rev 5): destroy-while-off followed by on reaches
the same unflip readiness and the same outcome as destroy-while-lit, both with
and without an established composed return; Presents sent while off (rev 5)
— each terminalized exactly once through the blackout path, `IdleNotify` and
release included, the direct frame still pinned; a
mixed server with a pending resource batch and a rejected Owner off — the
serviced-time clock keeps running; and a Legacy off on a mixed server leaving
the Owner device's scanout state untouched. Added in revision 3: supersession
while the topology work is still queued (the stale entry never reaches final
`TEST_ONLY` or dispatch) and while an earlier executor call is delayed (the
winner does not dispatch until that call returns or is reaped, and the stale
result is quarantined); an off behind an accepted primary predecessor that
must drain first; an injected missing and an injected
late off fence (expiry at the lifecycle deadline → `CompletionUnknown` →
`Poisoned`, and a late fence after it never promotes); and **capability
stability** (C.0 §16.2 item 39): the advertised cursor/primary capability is
captured before and compared after four Owner off/on cycles and after an
injected completion loss.

### 5.3. Hardware (user approval, tty, GPU free)

`c0_hw_3a_dpms_owner_on_card1_drm`, modelled on the Ciii/Cp hardware tests:
off/on × 4 on card1, each off transition's out-fence observed signalled with
no later vblank, the retained buffer unchanged, and composed frames admitted
again after each on — the fourth cycle as well as the first. If NVIDIA rejects
`ACTIVE=0` with a retained plane, or never signals the off fence, that is the
result: the transition must fail as section 3.7 says, and the finding decides
whether section 3.4's shape needs a driver-specific alternative.

### 5.3.1. The second device *(rev 4, round-3 M-1)*

C.0 §16.3 asks every available device — here the Raphael iGPU (`amdgpu`) as
well as the RTX 5060 Ti — for the real off-fence loop and the bounded delivery
check. 3a's hardware gate is card1 only; the iGPU's DPMS delivery belongs to
C.0's **final-tip bounded delivery check** (stage 5, §18), which needs a
monitor on the iGPU. If none is connected there, that device is reported as
unexercised by exact identity, as §16.3 requires, never claimed. A
completion-safety failure observed on either device is classified under C.0's
release disposition rules, not waived.

### 5.4. Mutations the plan must name (criteria, not edits)

The plan attaches one to each invariant; at minimum: the legacy loop again
iterating Owner outputs; idempotence read from `kms_outputs_active`; an
off commit without the old-active CRTC in the expected set; a per-output
fallback after a combined rejection; a rejection treated as poison; a
`MechanismFailed` on Owner reaching `request_exit()`; a stale success
promoted; the retained buffer released while off; a composed offer admitted
on an off CRTC; the representative marked `Applied` before every projection
retired; a DPMS request while `Poisoned` issuing a KMS mutation; a rejected
representative counted toward `Applied`; a latched generation retried;
the serviced-time clock
paused by a Legacy off while an Owner output is lit; `drain_all` or
`reset_scanout_bos_for_suspend` reaching an Owner device; a superseded
topology entry reaching final `TEST_ONLY`; an unflip made a precondition of
the off; the direct allocation released while its CRTC is off; a direct
successor admitted on an off CRTC; the off commit timed by the fast primary clamp; the advertised capability changing
across DPMS or poison.

## 6. Question to answer before the plan *(rev 5, round-4 B-1)*

Rounds 3 and 4 both reached the same wait: the Ciii unflip is ready only with
exit retirement vacant, a composed return established and the shadow
materialized (`render/admission.rs:1007`), and the tick returns before
composing while an unflip is requested (`backend.rs:22551`). If a direct unit
can be current while **no** composed return is established, a client that
destroys its window leaves its last frame on screen until something else
establishes one — with or without DPMS. Before the 3a plan: determine whether
any path leaves a direct unit current without a retained composed return. If
none does, record the proof and the case cannot arise. If one does, it is a
defect of Ciii — ours, by the project's ownership rule — fixed in its own
commit with its own test and mutation, named as a stage 2c-iii addendum, and
not absorbed into 3a's design.

## 7. Out of scope

VT, hotplug, reprobe, device add/remove as executed transitions (3c); client
modesets and the RANDR obligations (3b); recovery out of `Poisoned`,
`ExecutorStalled`, shutdown (3d); cursor and gamma on Owner (stage 4); legacy
removal (stage 5).
