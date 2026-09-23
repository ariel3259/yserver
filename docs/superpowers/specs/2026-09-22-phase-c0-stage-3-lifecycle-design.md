# Phase C.0 stage 3 — lifecycle, modeset, DPMS, VT and topology

**Status:** Umbrella design, **revision 2** (codex round 1:
[findings](../findings/2026-09-22-stage-3-umbrella-design-review-round1.md)).
Sections 1–4 approved section by section by the
user on 2026-09-22 (brainstorming session). It fixes the decomposition, the
shared contracts, the client-visible contract and the evidence regime of
stage 3. Each sub-stage (3a–3d) gets its own spec → codex review → plan cycle;
this document authorizes none of their plans. It also proposes an amendment to
C.0 §18 (section 6 below).

**Baseline inspected:** `1bdde511` on `feat/phase-c0-atomic-kms-migration`.
Stage 2 is complete: plan Cp accepted
([finding](../findings/2026-09-22-stage-2c-iii-plan-cp-accepted.md)), which
closed 2c-iii's writer coverage for the primary (composed, direct, unflip,
device-qualified identity, copied route).

**Authority:** [C.0 specification](2026-08-26-phase-c0-atomic-kms-migration-design.md),
especially §6.4 (device lifecycle matrix, `REC-1..6`), §9.2 (ordering classes),
§10, §13, §16 and §18.3. This document elaborates the stage boundary; it does
not replace or relax any governing contract.

## 1. Outcome and observed baseline

Stage 3 converts every remaining lifecycle writer — modeset, output disable,
DPMS, VT release/acquire, hotplug/reprobe/topology rebuild, device
add/remove, recovery and shutdown — into owner-held lifecycle intents that
run through one arbiter per device and the device's single commit slot.

What exists at the baseline:

- **Identities only.** `kms/owner/lifecycle.rs` holds `LifecycleEpochId`,
  `LifecycleTransitionId` and `ClockProbeId`. There is no `LifecycleDesired`,
  no transition, no arbiter and no §6.4 state type.
- **An empty admission seam.** `admission/decide.rs` reserves
  `Tier::Topology = 1`, which pre-empts every other tier, with
  `Admission::request_topology(generation: u64)`. Its only caller is a test
  (`backend.rs:59247`).
- **Qualification.** `owner/qualification.rs` `CompletionQualification`
  (`Unqualified → Awaiting → Qualified`, keyed by topology generation) covers
  the qualification half of §6.4 `Unqualified`/`Recovering`.
- **Writer gating.** `resources/transport.rs` defines `TransportState`
  (`Legacy/Quiescing/Owner/Closed`) and `WriterClass` including `Modeset`,
  `Dpms`, `Vt`, `Topology`. The legacy lifecycle writers already ask
  `allows_legacy(class)` and are refused on an `Owner` device
  (`platform.rs:7167`, `:7622`, `:8146`, `:8242`, `:8297`).
  `TestWriterCoverageEvidence` carries one field per class.
- **The legacy lifecycle model.** `platform.dpms_set_outputs_active(bool)`
  (`platform.rs:8184`) is a best-effort per-output loop, and the global
  `kms_outputs_active: bool` has ~83 sites in `backend.rs`. Entry points:
  `set_dpms_power` (`backend.rs:30965`), `run_suspend`/`run_resume`
  (`:13762`, `:13890`), `apply_crtc_config` (`:24016`),
  `on_display_hotplug`/`reprobe_connectors`/`run_display_rescan`
  (`:23546`, `:23563`, `:14653`), `relight_after_direct_teardown`/
  `teardown_direct_before_topology_requery` (`:3430`, `:3457`),
  `enable_connector_inner` (`platform.rs:7593`), `disable_output`
  (`platform.rs:8113`), `replay_copy_free_scanout_plan`/
  `replay_copied_scanout_plan` (`platform.rs:1967`, `:2233`),
  `shutdown_destroy_drawables` (`backend.rs:12621`).
- **Asynchronous RANDR already exists at the protocol boundary.**
  `Backend::begin_crtc_config` may return `CrtcConfigApply::Pending`; the core
  parks the client (`core_loop/run.rs:568` `park_crtc`) until a
  `Message::CrtcConfigReady` wake, then `drain_ready_crtc_configs` /
  `finish_crtc_config` / `cancel_crtc_config`.

## 2. Architecture (approved: design A)

### 2.1. Two layers, one direction

```text
X11 core ──> LifecycleCoordinator (backend, global)
                 │ projects
                 v
            DeviceLifecycleArbiter (one per DeviceKey)
                 │ typed actions
                 v
            Admission::request_topology ──> Tier::Topology ──> device slot
                 ^                                                │
                 └──────────── slot outcomes / dispositions ◄─────┘
```

The coordinator never issues a DRM commit. The arbiter never talks to X11.
Lifecycle work is §9.2 ordering class 1 and enters the device's one slot; it is
never a second writer. Information flows back only as `REC-5` dispositions and
the §6.4 state. The coordinator aggregates them for the protocol: a global DPMS
request is `Applied` only when every device's projection retired or was
invalidated by output removal (`REC-5`).

### 2.2. The coordinator owns only global state

`shutdown_requested` (monotonic), `seat_target` + `seat_epoch`,
`protocol_dpms_level` + `dpms_epoch`, and the `LifecycleEventId` allocator
(monotonic, checked, never wrapping — the `LifecycleEpochId` pattern). Every
event first updates this snapshot, then is projected. There is no FIFO at any
layer. Its shape mirrors the §13 cursor-transfer coordinator: small,
backend-level, commit-free.

### 2.3. The arbiter owns everything per device

Per `DeviceKey`, beside `admission_conductors`: the device's
`LifecycleDesired` (presence + `identity_epoch`, administrative reprobe,
`topology_dirty` + `discovery_epoch` + change class, `dpms_target` per stable
protocol output, `recovery_incident`), at most one
`LifecycleTransition { id, kind, phase }`, the §6.4 state and the
`RecoveryId`.

The arbiter is **pure**: its inputs are projected events and acknowledged
outcomes, its outputs are typed actions (request a topology commit for a
transition, close admission, cancel pre-submit work, terminalize protocol work,
transfer quarantine, record a disposition). It owns no fd, no executor call
and no resource. That is what lets `REC-4/5/6` be proven by exhaustive tables
rather than samples (section 5.1).

### 2.3.1. The lifecycle driver applies the actions *(rev 2, round-1 M-1)*

A pure arbiter decides; something must act. Each device's **lifecycle driver**
is the effectful owner: it lives in the backend beside that device's admission
conductor, on the core loop, at the site that already routes the device's
owner events (`route_owner_event_batch`, `backend.rs:20801`). It is the only
code that applies arbiter actions: it closes and reopens admission, cancels
pre-submit work and helpers, terminalizes each Present exactly once, requests
`Tier::Topology` work, and moves retained resources into the winning
transition's quarantine. It reports each action's **acknowledged** outcome back
to the arbiter as an input; the arbiter never assumes an action succeeded.
Supersession (`REC-4`) is therefore a sequence the driver executes promptly on
the core loop — it never waits on an executor host call — and admission
priority alone is never relied on to perform it.

### 2.4. `Tier::Topology` carries the transition, not a `u64` *(rev 2, round-1 M-3)*

The tier's payload becomes the transition's identity — incarnation,
`LifecycleEpochId` and `LifecycleTransitionId` — in place of the bare
generation. A type cannot know whether a *later* reply is still current, so
the tag alone proves nothing about freshness. What it buys is that there is
**one owner-side result-disposition boundary**, in the lifecycle driver, which
every executor reply for lifecycle work must cross and which compares the
reply's identity with the device's current incarnation, epoch and transition.
The typed tag makes that comparison unavoidable (the installed-state promotion
takes the validated result, never the raw reply), as Ciii's `CommitKey` made a
device-blind lookup fail to compile.

At that boundary, per `REC-4`: a current explicit success may promote; an
explicit rejection permits generation-local never-submitted cleanup; a stale
explicit success is **accepted-stale** — every returned fd is adopted or closed
exactly once, protocol work is terminalized once, and both possible
state/resource sets are retained in the winning transition's quarantine; an
absent or invalid result is acceptance-unknown with the same quarantine.

### 2.5. The Legacy fork is unchanged until stage 5

In production every device stays `Legacy`. The legacy lifecycle writers keep
working behind `allows_legacy`. The Owner path is new and **must not read
`kms_outputs_active`**; each sub-stage proves this for the paths it adds.
Removing the boolean and the legacy writers is stage 5 work (section 6), not
something left for after the merge. Each sub-stage turns its
`TestWriterCoverage` field(s) to proven only with its own evidence.

### 2.6. `TransportState` and the §6.4 state are two types (approved)

`TransportState` answers *who writes*. The §6.4 state
(`Unqualified`, `Ready`, `Quiescing`, `Poisoned`, `Recovering(RecoveryId)`,
`RecoveryFailed`, `ExecutorStalled`, `ShutdownExecutorStalled`, `Removed`)
answers *what the Owner may admit*. It exists only while the transport is
`Owner`. Ordinary admission requires `Owner ∧ Ready`; the §6.4 exceptions
(install/qualification only under `Unqualified` and `Recovering`) are the only
others. §6.4 `Quiescing` and `TransportState::Quiescing` are different things
and keep different names in code (the sub-stage 3a spec chooses them).
`CompletionQualification` is consumed by the §6.4 state, not duplicated.

`TransportState` exists only while Legacy and Owner coexist. It is **not** dead
code at the end of stage 3: production devices stay `Legacy`, and the primary
(`drm/page_flip.rs`, `scene.rs`), modeset and cursor/gamma legacy writers
(`platform.rs:4211`, `:4343`, `:4465`, `:4664`) keep asking `allows_legacy`
until stage 4 converts the last of them. Removing it earlier would either strip
production of those writers or force production to `Owner` while the legacy
cursor is still refused there, and it would delete the Legacy oracle that the
section 4.1 differential gate of the later sub-stages compares against. It is
removed at the moment it is replaced: the first act of stage 5 (section 6).

## 3. Sub-stages (approved)

Every sub-stage: own spec → codex review → plan of at most ~14 tasks (split
again if it grows — measured rule), at least one **production caller** of the
arbiter, the section 4 differential gate, and hardware at N ≥ 4 cycles.
Order 3a → 3b → 3c → 3d: each adds transition kinds to the previous arbiter,
and 3d needs every kind to exist before it can transfer quarantine between
them.

### 3a — Arbiter and global DPMS

- The coordinator and arbiter of section 2, with the full `REC-4` precedence,
  `REC-5` coalescing and dispositions, and the full `REC-6` recovery-fate
  matrix (all of it, even though only DPMS executes in 3a); the §6.4 state
  type; `Tier::Topology` typed.
- Production caller: `set_dpms_power` on an Owner device → one global epoch →
  projection per stable output → **one atomic transition per device** covering
  every compatible output on it (never a per-output best-effort loop).
- DPMS-off requires old-active out-fence evidence (C.0 §16.2 item 45).
- A DPMS target arriving while the seat is released is `Deferred`, not
  dropped (C.0 §16.3, lifecycle hardware list).
- **The failure entry edge is executed in 3a, not deferred** *(rev 2, round-1
  B-1)*. No live lifecycle commit exists without it. A DPMS transition whose
  commit is rejected, loses completion evidence or breaches its deadline goes
  through the device owner's existing poison (`DeviceCommitOwner::is_poisoned`
  closes admission; `CompletionUnknown` records are quarantined by the 2b
  ledger), and the driver then: terminalizes the transition, moves the §6.4
  state to `Poisoned`, records the `REC-6` outcome for any incident, gives
  every affected event id its disposition, and retains the quarantine under the
  device. What 3d adds is the **exit** from `Poisoned` — the sole `REC-1`
  recovery attempt, `RecoveryFailed`, `ExecutorStalled` and teardown. Between
  3a and 3d a poisoned Owner device simply stays closed; that window exists
  only in fixtures, because production is `Legacy` until stage 5.
- **Projections follow topology** *(rev 2, round-1 M-2)*. Any installation of a
  topology — here, and in 3b and 3c — first refreshes each stable output's
  `dpms_target` from the coordinator's current global level and epoch, so a
  newly discovered or re-created output is never installed active after a
  global DPMS-off; a removed output's projection is invalidated exactly once.
  3a defines the rule and its test; 3b and 3c each prove it at their
  installation sites.
- Exit: `dpms` coverage proven. Hardware: DPMS off/on × 4 on card1.

### 3b — Modeset and RANDR

- `apply_crtc_config` through the `begin_crtc_config` → `Pending` →
  `CrtcConfigReady` path; `enable_connector_inner`, `disable_output` and the
  two `replay_*_scanout_plan` as lifecycle intents (`TopologyRebuild` on the
  same identity).
- The direct-scanout all-off/relight helpers around a topology change,
  `teardown_direct_before_topology_requery` and
  `relight_after_direct_teardown` (`backend.rs:3457`, `:3430`), are converted
  here; their RANDR callers are 3b's, and 3c reuses the converted helpers for
  the output-topology change (`backend.rs:14103`).
- **Publication is independent of the requester** *(rev 2, round-1 B-2)*.
  Today only the PRIME qualification probe is asynchronous: disables and
  same-device changes return `Applied` synchronously
  (`backend.rs:23685`–`23700`), and a ready token whose client has gone is
  **cancelled** (`core_loop/run.rs:1061`) — correct for a disposable probe,
  wrong for an Owner commit that has already reached the hardware. On Owner,
  every real mutation is parked, including disables and same-device changes,
  and its outcome is split in two: the **publication** (RANDR state refresh and
  the change notifications of `complete_crtc_config`,
  `process_request.rs:4934`) happens when the transition reaches `Applied`
  whether or not the requester is still connected; the **reply** goes only to
  a requester that is still waiting. A requester's disconnect cancels its reply,
  never an accepted transition or its broadcast. This touches `yserver-core`'s
  continuation and is part of 3b's scope.
- Topology epochs invalidate earlier queued intents (C.0 §9.2, §13).
- Exit: `modeset` coverage proven. Hardware: a RANDR mode change and an
  output disable/enable, × 4.

### 3c — VT, hotplug and devices

- `run_suspend`/`run_resume` as `VTRelease`/`VTAcquire`, with prompt logical
  obligations that never wait on the executor (`REC-4`).
- `on_display_hotplug`, `reprobe_connectors`, `run_display_rescan` as
  `IdentityChangingHotplug`, `TopologyRebuild`, `AdministrativeReprobe`;
  `DeviceAddedOrReplaced` and `DeviceRemoved`.
- DPMS and topology while the VT is released are `Deferred(prerequisite)`.
- Exit: `vt` and `topology` coverage proven. Hardware: VT switch × 4 and a
  real connector hotplug (HDMI-2).

### 3d — Recovery, quarantine and shutdown

- `REC-1`: exactly one automatic attempt per incident, `RecoveryFailed`
  thereafter; `Poisoned`, `Recovering`, `ExecutorStalled`,
  `ShutdownExecutorStalled`, `Removed` executed, not just typed.
- The retained owners earlier stages left "for stage 3's teardown barrier"
  (2c-i resource terminalization §3 and its teardown handoff contract, 2c-i
  debt §4.3, Cfb §3's `CompletionUnknown` and unflip-unknown rows) are transferred into the winning
  transition's quarantine.
- Orderly shutdown with the teardown supervisor; `shutdown_destroy_drawables`.
- Exit: the §6.4 matrix is total; nothing is retained without an owner.
  Hardware: injected completion loss, and shutdown with a delayed helper.

## 4. Client-visible contract (approved as a gate)

Stage 2 only changed what clients see as frames. Stage 3 touches X11 protocol
behavior, so a client can break. The surfaces:

| Surface | Risk | Sub-stage |
| --- | --- | --- |
| `RRSetCrtcConfig` reply | status (`Success`/`Failed`/`InvalidTime`) and timing | 3b |
| RANDR events (`ScreenChangeNotify`, `CrtcChange`, `OutputChange`) | order, and whether they precede the hardware change | 3b, 3c |
| DPMS (`DPMSForceLevel`, `DPMSInfo`) | low: no reply; `DPMSInfo` reports the protocol level, updated at once | 3a |
| VT switch | today clients see nothing (mirrors Xorg `xf86Events.c:358`); must stay so | 3c |
| Hotplug | `GetScreenResources` contents and timing relative to events | 3c |

Xorg is the **protocol oracle**, not the design: what clients expect is what
Xorg does, but its per-output `xf86DPMSSet` loop is the model C.0 forbids, and
neither Xorg nor wlroots has an isolated asynchronous executor.

**Timing rule.** RANDR events and the `RRSetCrtcConfig` reply are emitted only
when the transition reaches `Applied` — never at submission. A rejected or
acceptance-unknown transition answers `Failed`. Any other behavior that differs
from Legacy is written into the sub-stage spec as a named exception with its
justification.

**VT invariant.** A VT release/acquire generates no client-visible event; the
sub-stage 3c spec names the test.

### 4.1. Three layers of client evidence

Real X11 clients cannot reach the Owner before stage 5: C.0 §18 forbids an
environment flag that selects a stage, so until activation the Owner runs only
inside `cargo test` fixtures.

1. **Differential gate, per sub-stage, no display** *(rev 2, round-1 M-4)*.
   Two levels, because the notifications and the reply are written by the
   core, not the backend (`complete_crtc_config`, `process_request.rs:4934`):
   - **Backend state.** Drive the `KmsBackend` through the `Backend` trait as
     `process_request` does (`set_dpms_power`; `begin_crtc_config` →
     `drain_ready_crtc_configs` → `finish_crtc_config`; VT suspend/resume;
     hotplug) with one script on a Legacy fixture and on an Owner fixture, and
     compare each result's status and the resulting RANDR/protocol state.
   - **Protocol order — the exit gate.** Drive the *core* request path with the
     same two fixtures: the request is dispatched, the client is parked, the
     ready wake is drained and completed, and the bytes written to each client
     connection are compared — the reply, its status, and the RANDR events on
     the requester's connection and on a second, listening connection, in
     order. The script includes a requester that disconnects while parked
     (section 3b's publication rule: the listener still receives the events).
   Identical, except for the named exceptions. `randr.rs` validation stays
   upstream of the backend and is not touched.
2. **Legacy golden, captured before 3a is implemented.** The current
   production server from a tty (user approval required), under `x11trace`,
   running: `xrandr` (`--mode`, `--off`, `--auto`, `--verbose`, a position or
   rotation change); `xset dpms force off/on` + `xset q`;
   `xdpyinfo -ext RANDR -ext DPMS`; `xev -root -event randr -1` in parallel;
   VT switch × 4; HDMI-2 unplug/replug. Serials and timestamps are normalized
   before any diff. xev and xrandr are separate connections, so their relative
   order proves nothing; same-connection reply/event order is covered by layer
   1's protocol-order gate and by MATE in layer 3.
3. **Real clients against the Owner, in stage 5.** The layer-2 battery rerun
   and diffed against the golden (only the named exceptions may differ), plus:
   **MATE** (`mate-settings-daemon`'s xrandr plugin and the display settings
   dialog, which select RANDR events and issue `SetCrtcConfig` on the same
   connection — captured under `x11trace`), and **Counter-Strike 2** entering
   and leaving fullscreen with a mode change.

## 5. Evidence regime (applies to 3a–3d)

### 5.1. Tests

Every test cites a C.0 §16.1 requirement group (`REC-1..6`, `CAP`, `COMMIT`,
`ID`).

- **Pure arbiter: exhaustive, not sampled.** `REC-4` precedence over every
  pair of the ten kinds; `REC-5` coalescing for every kind × every lower kind
  while active (C.0 §16.2 item 63); the `REC-6` matrix for every winning kind ×
  an active incident (item 67); every `LifecycleEventId` reaches exactly one
  terminal disposition (items 64, 66).
- **Fixture integration.** Production caller → coordinator → arbiter →
  `Tier::Topology` → slot → outcome → disposition, on the existing Vulkan/DRM
  fixtures.
- **The section 4.1 differential.**
- **Mutations** with a ledger per task. Findings and plan criteria state the
  invariant that must hold and the mutation that must fail, never the lines to
  edit.

### 5.2. Hardware

- N ≥ 4 cycles for every lifecycle transition. Plan Cp's two production
  defects appeared on the fourth frame, not the first.
- The user is asked before every hardware run (the box is also the VR/gaming
  rig); anything that modesets the screen runs from a tty with approval.

### 5.3. Gates

Per task and per sub-stage: `cargo +nightly fmt`; clippy
`--all-targets -- -D warnings` in default, `tcp-transport` and `xdmcp`;
`c0_*` and `--lib` in full; the helper-spawning suites repeated (one green run
is not evidence).

### 5.4. Roles

Codex `gpt-6-luna` xhigh implements; codex `gpt-6-sol` xhigh reviews specs and
plans; the coordinator verifies, applies mutations and commits. Plans give
interfaces, invariants, tests and mutations — not prototype code.

## 6. Amendment to C.0 §18: stage 5 *(rev 2, round-1 M-5)*

C.0 §18.3 assigns to stage 3 the conversion of "every use of the merged
all-output `dpms_set_outputs_active(bool)`/`kms_outputs_active` model". This
design keeps the Legacy uses alive behind `allows_legacy` until activation, and
stage 4 does the same for cursor and gamma; leaving them for after the squash
would hand the maintainer dead code. Appending a stage does not by itself
change stage 3's exit contract, so the amendment is applied **to C.0 §18
itself** (C.0 revision note of 2026-09-22): stage 3 converts every lifecycle
caller's **Owner path** and proves that no Owner path reads the legacy model;
the Legacy uses and the model itself are deleted in stage 5. A 3d exit with
Legacy uses still present is therefore compliant with the amended §18.3, and
not before the amendment is committed. The fifth stage inside the same PR:

> **5. Activation and legacy removal.** Switch every device to `Owner` in
> production. Remove the legacy lifecycle, cursor and gamma writers, the
> `allows_legacy` gates, `kms_outputs_active` and its sites,
> `TransportState::Legacy` and every structure that exists only for
> Legacy/Owner coexistence, and the `TestWriterCoverage` scaffolding. Run the
> section 4.1 layer-3 client battery, the upstream-fixes revalidation
> (`docs/phase-c0-upstream-fixes-revalidation.md`) on Owner, and the final-tip
> gate. Stage 5 has its own spec and plan.
>
> Its first act, once the last legacy writer is gone, deletes `TransportState`
> entirely — the handover (`Quiescing` → `publish_owner`, `HandoverPermit`,
> `LegacyDrained`) and `allows_legacy` — and folds `Closed`/`force_close` into
> the §6.4 state, which becomes the only device state.

**Entry preconditions of stage 5** (owed debts, named here so they are not
loose findings):

- the managed-storage access debt, Cfb §2.5 (`backend.rs:18716`);
- the Ci F8 stops: Legacy dormancy (upstream's, meets none of the ownership
  tests — to be re-examined once Legacy is removed), the missing restore
  `TerminalState`, device loss without an owner signal (3d is expected to
  close the last one);
- coverage gaps T24, T27 and Cfb F31;
- `cargo check --workspace` for Linux musl and FreeBSD at the final tip;
- Q33 and Q34 applied on hardware in the final-tip rerun.

## 7. Out of scope

- Cursor and gamma conversion — stage 4.
- Production activation and legacy removal — stage 5.
- Phase C.1 (including the commit-correlated completion sample for composed
  commits that plan Cp asked C.1 for).
- **No new protocol surface.** Stage 3 adds no extension, request, event or
  RANDR property. X11 DPMS stays one global control (`REC-5`); the per-output
  projection is internal and never exposed to clients (for example as a
  per-output `DPMS` property — today RANDR publishes only `EDID`, `EDID_DATA`
  and `ConnectorType`, `process_request.rs:3202`). What changes is execution,
  not the control: one global request becomes one atomic transition per
  device, replacing the per-output best-effort loop, the global
  `kms_outputs_active` boolean and the `request_exit()` fail-stop on a partial
  all-off. Extensions unrelated to the lifecycle are outside C.0, neither
  forbidden nor governed by it.
