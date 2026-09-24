# Phase C.0 stage 3b — client modeset and the RANDR protocol on the Owner

**Status:** Revision 1, written by the coordinator on 2026-09-24 from a
brainstorming session with the user. Every decision below marked
**(user decision)** was taken in that session; the rest elaborates them or
applies the umbrella and C.0 without a new choice. It goes to codex review
before any plan.

**Authority:** [C.0](2026-08-26-phase-c0-atomic-kms-migration-design.md)
(§6.3, §6.4, §9.2, §9.4, §10 and its latch scopes, §10.3, §13, §16, §18 as
amended) and the
[stage 3 umbrella](2026-09-22-phase-c0-stage-3-lifecycle-design.md) revision 6,
whose section 3b (scope and the six RANDR obligations) and section 4 (client
contract) this spec elaborates. The
[3a design](2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md) is the
base it builds on. None of them is relaxed here.

**Baseline:** `8f0de9ea` (3a-ii accepted, upstream `2282c94f` merged).

**Structure (user decision).** One spec, two plans, as 3a: **3b-i** — backend
execution (sections 3–6); **3b-ii** — the RANDR protocol (section 7). 3b-i
comes first: 3b-ii orders and answers the transaction 3b-i defines. Each plan
stays at or under ~14 tasks and is split again without asking if it grows.

## 1. Outcome

At the end of 3b:

- an `RRSetCrtcConfig` that targets an output of an **Owner** device runs as
  **one atomic transaction per device**, driven by that device's lifecycle
  driver and its single commit slot — never the Legacy all-off / mutate /
  relight sequence;
- the enable, disable and mode-change writers (`enable_connector_inner`,
  `disable_output`, `replay_copy_free_scanout_plan`,
  `replay_copied_scanout_plan`) and the direct-scanout topology helpers
  (`teardown_direct_before_topology_requery`,
  `relight_after_direct_teardown`) have an Owner path; their Legacy path is
  unchanged and reachable only on a Legacy device;
- a modeset on one device issues no lifecycle commit on another and leaves
  the other devices' in-flight composed work intact;
- RANDR mutations are ordered server-wide by one named component, Legacy and
  Owner alike, and every parked request is answered within a stated bound;
- the `modeset` field of `TestWriterCoverage` is proven.

Production stays `Legacy` until stage 5 (umbrella §2.5); the Owner path runs
in fixtures and in the hardware tests.

## 2. Baseline observed

- **Legacy client modeset** (`KmsBackend::apply_crtc_config`,
  `backend.rs:24755`; asynchronous tail `finish_crtc_config`, `:24617`):
  validation and discovery, then `quiesce_before_topology_mutation`
  (`backend.rs:3649`), which disables **every CRTC of every device**
  (`dpms_set_outputs_active(false)`, `platform.rs:8328`, iterates all of
  `platform.outputs`), drains the whole scene (`scene.drain_all`,
  `scene.rs:3435`, over every `platform.devices` entry), resets every scanout
  BO and bumps the global CRTC-config epoch; then the platform mutation
  (`enable_connector` or `remove_connector_after_all_off`), a full scene
  rebuild (`scene.rebuild_outputs`, `scene.rs:1732`, which replaces the
  `OutputSceneState` of **every** output) and a relight of every output
  (`relight_after_direct_teardown`). Failure after the all-off goes through
  `recover_failed_crtc_config` (`:3703`); a failed relight or scene rebuild
  calls `request_exit()`.
- **Legacy and DPMS-off** (`kms_outputs_active_after_crtc_config`,
  `backend.rs:1297`): an enable while every output is dark returns `true`, and
  the relight lights **every** output while the protocol DPMS level stays Off.
- **Idempotency guard** (`apply_crtc_config`, `:24810`): a request
  equal to the current `platform.outputs` state answers `Ok(false)` — `Success`
  with no rebuild and no notification — and releases the remembered route.
- **Asynchronous RANDR today** (`begin_crtc_config`, `:24417`): only an enable
  that needs a disposable PRIME qualification probe returns
  `CrtcConfigApply::Pending`; the probe runs in the probe executor under a
  30 s watchdog (`probe_executor.rs:46`). The core parks the client
  (`PendingBackendRequests::park_crtc`, `core_loop/run.rs:568`), blocking only
  that client (`client_is_blocked`, `:564`; the ready ring skips blocked
  clients, `:702`), resumes it from `drain_ready_crtc_configs` (`:1055`) and
  cancels the token if the client is gone (`:1061`). Other clients' RANDR
  requests meanwhile proceed; a topology change makes the pending probe stale
  (`stale_crtc_config_probe_reason`, `backend.rs:4585`), and it answers
  `Failed`.
- **The core owns the reply and the publication.** `complete_crtc_config`
  (`process_request.rs:4934`) rebuilds RANDR state with the request's
  `set_time` (`refresh_randr_state_set_time`), emits
  `emit_randr_change_notifications` and the screen-resize notifications, and
  writes the reply (status 0 or 3, `RRSetConfigFailed`). Validation
  (`randr.rs`, `screen_encompasses`, the timestamps) runs in
  `process_request` before the backend is called.
- **The 3a projection hook.** `lifecycle_register_owner_device`
  (`render/admission.rs:641`) reconciles each Owner device's DPMS projections
  with `platform.outputs`: an output no longer present is removed from the
  coordinator, a present one is added with the current global level
  (`LifecycleCoordinator::add_protocol_output`, `coordinator.rs:166`). Its
  only callers today are conductor installation sites.
- **The 3a lifecycle commit.** `lifecycle_topology_description`
  (`render/admission.rs:1122`) builds an `ACTIVE`-only CRTC description for
  DPMS; the driver validates (`lifecycle_finish_topology_validation`, `:1605`)
  and submits (`lifecycle_submit_validated_topology`, `:1681`) under the
  30 s bootstrap lifecycle deadline.
- **Direct scanout eligibility** requires every output on the primary device
  with equal effective refresh (`direct_scanout_topology_eligible`,
  `backend.rs:4012`), a whole-root authoritative Present, and seven other
  gates (`direct_present_eligibility`, `:4052`). `has_current_direct`
  (`render/admission.rs:4373`) is device-blind — latent, recorded in
  [the Ciii finding](../findings/2026-09-23-ciii-direct-entry-without-composed-return.md);
  3b does not make direct multi-device and leaves it as recorded.
- **Renderer loss.** `renderer_failed` (a Vulkan `ERROR_DEVICE_LOST`) makes
  `maybe_composite` request a clean shutdown (`backend.rs:23209`) so the
  display manager respawns the server. This is global render-device policy,
  not modeset policy; 3b does not change it.
- **Carried into 3b** (3a-ii acceptance finding): call the projection hook at
  3b's installation sites; route the four value-dead `kms_outputs_active`
  reads (`teardown_direct_before_topology_requery`'s `relight`,
  `crtc_config_topology_signature`, `enqueue_prepared_crtc_config_probe`'s
  `was_active`, `apply_crtc_config`) in the change that first lets their path
  proceed on Owner; start a new epoch's clock probe at a client modeset (C.0
  §10).

## 3. The client modeset in the lifecycle model (3b-i)

### 3.1. Class-1 client work, not a `REC-4` kind

A client modeset is §9.2 ordering-class-1 work requested by a client. It is
**not** one of the ten `REC-4` kinds and **not** a `LifecycleDesired` field
(umbrella §3b). The lifecycle driver of each Owner device gains one slot
beside the `REC-4` transition: **at most one client modeset per device**, with
its own identity (`ClientModesetId`, allocated like `LifecycleTransitionId`:
monotonic, checked, never wrapping) and its own typed tag on
`Tier::Topology`, so the umbrella §2.4 result boundary compares its replies
with the device's current incarnation, epoch and modeset id exactly as it
does for a transition. A second modeset on the same device while one is in
the slot is refused with a typed error; with the section 7 gate in front it
is unreachable, and before 3b-ii it is reachable only in fixtures.

### 3.2. When it starts, and what supersedes it

A client modeset is **dispatched** only while the device is `Owner ∧ Ready`,
the arbiter has **no active transition**, and every CRTC the transaction
touches has a current clock (`KernelSequence` for this epoch; the 3a-ii
readiness wait, `lifecycle_clock_readiness`). The seat must be active: while
the VT is released the request fails at once (section 7.4); it never parks
on the VT.

- A `REC-4` event that arrives **before dispatch** supersedes the modeset:
  its prepared set is released exactly once, the slot empties, and the result
  is `Failed` with cause `Superseded(kind)`. Nothing was submitted, the old
  topology stays authoritative (C.0 §9.2: topology/ownership work invalidates
  queued intents of earlier generations).
- A `REC-4` event that arrives **after dispatch** does not overtake it (C.0
  §9.2: never overtakes a `Submitting` or accepted commit). The arbiter
  records the event as desired; its transition starts when the modeset's
  result has crossed the boundary. **A dispatched modeset is never cancelled
  as never-submitted.**

### 3.3. The transaction

One `ALLOW_MODESET | NONBLOCK` atomic commit through the device's executor
(C.0 §4.1, §9.2), carrying the device's **complete desired topology**:

- for the target output: connector `CRTC_ID`, the CRTC's new `MODE_ID` blob
  and the primary plane with the new framebuffer (enable or mode change); or
  the connector detached, the CRTC `ACTIVE=0` with `MODE_ID` cleared and the
  primary plane released (disable);
- every other output of the device: unchanged, and therefore omitted from the
  property set;
- **`ACTIVE` of an enabled CRTC = its output's `dpms_target`
  (user decision, option A).** Under a global DPMS-off the mode is installed
  with `ACTIVE=0`: configured and dark, the state 3a-ii's `ACTIVE`-only off
  produces. The next DPMS-on is 3a-ii's `ACTIVE`-only commit and lights it in
  the new mode;
- every CRTC that goes from active to inactive in the commit is in
  `ExpectedCompletionCrtcs` with a required out-fence (C.0 §16.2 item 45, as
  in 3a-ii); a commit that lights a CRTC is accepted on its `HardwareComplete`
  evidence as C.0 §6.3 requires for its class;
- deadlines: C.0 §10.3's lifecycle hardware-completion deadline (the 30 s
  bootstrap while no cohort measurement exists) and the 2 s seat-active
  host-call watchdog — never the fast primary clamp.

Freshness is checked, as in 3a (§3.6), immediately before the final
`TEST_ONLY` and again immediately before executor dispatch; a stale entry is
cancelled as never-submitted.

### 3.4. The composition with DPMS, and projections follow topology

- The transaction reads each output's `dpms_target` from the coordinator,
  never `kms_outputs_active`.
- At installation (section 4.2), the driver calls the 3a projection hook for
  the device: an output that appeared (enable) gets a projection from the
  current global level and epoch before it can be lit; an output that left
  (disable) has its projection invalidated exactly once. The umbrella's rule
  — a newly installed output is never active after a global DPMS-off — is
  proven at this site.
- **Named exception (Legacy parity, physical only):** Legacy lights every
  output on an enable while DPMS is off and keeps the protocol level Off; the
  Owner keeps them dark. Clients see the same bytes — the reply, the events,
  `GetCrtcInfo` and `DPMSInfo` are identical — because RANDR reports
  configuration, not power, and DPMS is not reported per output.
- **Named risk:** with `ACTIVE=0` a driver may validate less (for example
  link bandwidth only when active). A mode accepted dark can be rejected at
  the next DPMS-on. That rejection is 3a-ii's `FailedBeforeSubmit` path; the
  RANDR client already received `Success`. The hardware test measures this
  sequence (section 8.1).

## 4. Preparation before dispatch and promotion after (3b-i)

### 4.1. Preparation, with the old topology lit

Everything that can fail without touching the screen happens before the
slot is taken, while the old topology keeps scanning out and composing:

1. discovery of the target connector with the other outputs of the same
   device reserved (`discover_output_for_connector` with `reserved_routes`,
   as Legacy) and the advertised-mode check;
2. scanout route selection (same device, copy-free, copied); the disposable
   PRIME qualification probe is kept unchanged — it runs in the probe
   executor and never touches KMS;
3. allocation of the output's new scanout pool through the 2c-i resource
   ownership (for the copied route, on the source and on the sink device).
   The **prepared set** — pool, framebuffers, mode blob — is owned by the
   transaction and released exactly once if it is not installed;
4. `TEST_ONLY` of the complete device transaction through the 3a-ii
   validation path.

**First framebuffer content:** as Legacy — the first BO of the new pool,
uncomposed; the scene repaints at once after installation. Not a
client-visible surface.

**Named exception — memory.** Legacy disables everything before allocating,
so the old and new pools of the output never coexist. On the Owner they
coexist from preparation until the old pool retires, so the output's peak
is old + new (for example ~100 MB for a 4K triple-buffered pool). An
allocation failure answers `Failed` and leaves the old topology intact where
Legacy might have succeeded. There is no safe alternative: freeing a pool
the kernel may still scan out is what C.0 forbids.

### 4.2. Promotion after a current success

At the result boundary, a current explicit success is promoted in this
order:

1. **KMS-state promotion — infallible.** `platform.outputs` (enable, mode
   change or removal), the output's scanout pool (the new pool installed; the
   **old pool retires only when completion evidence proves the kernel no
   longer scans it**, C.0 §6.3 — never at promotion), the RANDR registry entry
   (`config`, `crtc_associated`, `client_configured`, `connected`,
   `last_enabled` exactly as Legacy sets them), the root extent
   (`fb_w`/`fb_h` recomputed from every layout, as Legacy), the input extent,
   the device's topology generation (queued intents of the older generation
   are invalidated, C.0 §9.2, §13) and a new clock epoch with its probe for
   every CRTC whose mode changed (C.0 §10). This step moves data only; it has
   no fallible call.
2. The projection hook (section 3.4).
3. **Scene promotion — per device** (section 5.1). Fallible; its failure is
   section 6's last rows.

### 4.3. The value-dead reads

The four `kms_outputs_active` reads carried from 3a-ii are routed to the
device's installed power (`owner_dpms_installed_active`) in the task that
first lets their path proceed on Owner, each with its D26-style mutation.

## 5. Other devices are not touched (3b-i)

### 5.1. Per-device scene rebuild, keyed by output identity

The scene rebuild becomes per device: the `OutputSceneState` of every output
of the modeset's device is rebuilt; every other output's state — including
its `owner_buffers`, `pending_acks` and damage history — is **kept**.

The scene state and `platform.scanout_pools` are indexed **by position** in
`platform.outputs`. Adding or removing an output shifts the positions of the
others, including those of other devices. The per-device rebuild re-associates
kept state **by `OutputKey`**, never by index; an index shift must not move an
in-flight buffer of another device to another output. This is an invariant
with its own mutation (section 8.3).

### 5.2. The root extent

Recomputed as Legacy (logical state, client-visible). Outputs of other
devices are not rebuilt; if the root storage identity changed they receive
full damage and repaint through an ordinary composed frame — a class-2
Present, not a lifecycle commit.

### 5.3. Direct scanout

- A direct frame current **on the modeset's device** is returned to
  composition first: preparation requests the Owner composed unflip (2c-iii)
  and the modeset is dispatched only after it retired.
- A modeset **that makes the topology direct-ineligible** — it adds an output
  on another device, or changes the root extent or an effective refresh so
  that `direct_scanout_topology_eligible` or the whole-root match fails —
  requires the composed unflip on the device holding the direct frame, retired,
  before dispatch. This is a class-2 composed commit on that device, not a
  lifecycle commit.
- `has_current_direct` stays as recorded; 3b does not make direct
  multi-device.

### 5.4. The copied route

Preparation allocates on both devices (section 4.1); the rebuild is per
output, so the source-side copy state travels with its output. The commit is
issued only on the output's (sink) device.

### 5.5. The Legacy path in a mixed server

A modeset on a **Legacy** device while another device is `Owner` scopes
Legacy's server-wide steps (all-off, event drain, scene drain, BO reset,
epoch bump, relight) to the Legacy devices, as 3a-ii Task 4 did for DPMS
(`legacy_devices`, `backend.rs:31789` onward). Without this a Legacy
`xrandr` would all-off an Owner device through a writer it does not own. A
server with no Owner device behaves exactly as today.

## 6. Failures (3b-i)

| Where it fails | Effect | Client status |
| --- | --- | --- |
| Preparation: discovery, unadvertised mode, route, allocation, `TEST_ONLY` | prepared set released; old topology authoritative; device stays `Ready` | `Failed` |
| Superseded by a `REC-4` event before dispatch | same | `Failed` |
| Real commit rejected with `EBUSY` | bounded retry per C.0 §9.4; exhausted → next row | `Failed` if exhausted |
| Real commit explicitly rejected (other errno) | `FailedBeforeSubmit`: nothing became current, prepared set released, no poison. An `EINVAL`/`EOPNOTSUPP` attributable to the object combination latches **the requested topology generation** (C.0 §10 latch scopes): an identical request answers `Failed` without dispatch until the device's topology generation changes; the current topology stays `Ready` | `Failed` |
| Completion loss: missing/invalid/error fence, deadline, contradiction | `CompletionUnknown` → `Poisoned`, both state sets quarantined (3a-ii) | `Failed`, nothing published |
| Stale result (accepted after its identity stopped being current) | accepted-stale: fds adopted or closed once, quarantine retained by the winning transition, nothing installed | `Failed` |
| Accepted, then scene promotion fails with renderer device loss | existing `renderer_failed` clean shutdown (global policy, unchanged) | none — the server exits cleanly |
| Accepted, then scene promotion fails otherwise (for example the composition ring cannot be allocated) | the topology is installed and published; that device's readiness closes until 3d's recovery; other devices continue; **no new `request_exit`** | `Success` (it was installed) |

A `TEST_ONLY` rejection does not latch: nothing reached the kernel's commit
path, and each request runs its own `TEST_ONLY`.

### 6.1. Diagnostics (user decision)

The RANDR reply carries only a status byte, and the umbrella §7 forbids new
protocol, so the cause is recorded server-side. Every `Failed` carries a
typed `ClientModesetFailure` that distinguishes each row above:
`Preparation(stage)` (with the `TEST_ONLY` errno where there is one),
`KernelRejected { errno }` — **only** when the kernel answered —,
`OwnerRefused(cause)` (the owner did not dispatch: slot conflict, clock not
ready, readiness closed), `Superseded(LifecycleKind)`, `CompletionUnknown`,
`Stale`, `Latched`, `PostInstallRender`. The owner never synthesizes an errno
(the 3a-ii lesson: an owner refusal reported as a kernel `EINVAL` cost a
`drm.debug` session).

Each failure logs one line with the client and request sequence, the output,
the requested mode and position, the device, the modeset id and the typed
cause; each success logs the same identity at `debug`. The Legacy log line
(`RRSetCrtcConfig apply failed: {e}`) stays for Legacy.

## 7. The RANDR protocol on the Owner (3b-ii)

### 7.1. `RandrMutationGate` — one mutation in flight server-wide (user decision, obligation 4)

A core-side component, `RandrMutationGate`, admits **at most one RANDR
mutation in flight in the whole server**, Legacy and Owner alike. A RANDR
mutation is every RANDR request that changes configuration —
`SetScreenConfig`, `SetScreenSize`, `SetCrtcConfig`, `SetOutputPrimary`,
`SetProviderOutputSource`; the plan enumerates them from
`handle_randr_request` and states each inclusion. `SetCrtcGamma` is color,
class 4, stage 4, and is excluded. Queries (`GetScreenResources`,
`GetCrtcInfo`, …) never wait and read the published state.

While a mutation is in flight, a client whose **next** request is a RANDR
mutation is not dispatched: it is blocked through the existing ready-ring
mechanism (`client_is_blocked`), keeping its position, and its later
requests wait behind it by the per-client FIFO. The mutation leaves the gate
when its publication (section 7.2) is complete.

Why one in flight and not concurrent per-device work published in order:
`randr.rs` validation (`screen_encompasses`, the config timestamps) and the
backend's idempotency guard run **before** the backend. With one mutation in
flight, each request is validated and compared against the state the
previous one published — exactly Legacy's serial behaviour. Concurrent work
would validate against a state a pending request is about to change: an
idempotent `Success` for a change that never applies (the MATE re-assert
pattern) with a different final state than Legacy, spurious or missed
`BadMatch`/`BadValue`, and a reply that precedes the previous request's
events. Publication order therefore equals dispatch order by construction,
across devices and across transports (obligation 4), and parity of the
idempotent request follows (obligation 1).

**Named exception — the concurrent PRIME probe.** Today a topology change by
another client while a PRIME probe is parked makes the probe stale and it
answers `Failed`. Behind the gate the second request waits and the first
completes. Client-visible only when two clients issue `SetCrtcConfig`
concurrently and the first goes through the PRIME probe; the result is the
serial one.

A server with no Owner device never has anything in flight except the PRIME
probe, so pure Legacy behaves as today apart from that exception.

### 7.2. Publication outlives the requester (obligation 3)

The parked request is split into the **publication** — the RANDR state
rebuild with the request's `set_time`, `CrtcChange`/`OutputChange`/
`ScreenChangeNotify` to selecting clients, the screen-resize notifications —
owned by the gate, and the **reply**, owned by the client. When the
requester disconnects while parked:

| Where the request is | Effect |
| --- | --- |
| Legacy PRIME probe | cancelled as today (disposable, nothing installed) |
| Owner, not dispatched | cancelled as never-submitted; nothing to publish |
| Owner, dispatched | continues; if installed, published to every other client; only the reply is dropped. The gate stays occupied until then |

`Success` means installed (obligation 2): a request is answered `Success`
only from a current success at the boundary (section 4.2) or as idempotent;
every other outcome answers `Failed` and publishes nothing of the request.
`lastSetTime` takes the request's timestamp as Legacy does, with no
monotonicity rule (umbrella obligation 1).

### 7.3. Bounded wait (obligation 5)

There is no separate global watchdog: one that answered `Failed` and then saw
the change install would contradict "`Success` means installed". The bound is
the sum of existing deadlines, each with its own timeout wake of the core
loop (no I/O needed to observe expiry):

| Stage | Deadline | On expiry |
| --- | --- | --- |
| PRIME qualification probe | 30 s (`probe_executor.rs:46`) | `Failed` |
| Wait to start: an active lifecycle transition, or the clock probe | that transition's or probe's own deadlines; a `REC-4` event supersedes the modeset at once | `Failed` (`Superseded`) or the probe's outcome |
| Executor host call | 2 s (C.0 §10.3 timer 2) | `CompletionUnknown` → `Failed` |
| Hardware completion | 30 s bootstrap, or the measured cohort value | `CompletionUnknown` → `Failed` |

Worst case ~95 s; bounded, and each stage is tested separately.

### 7.4. What never parks

- **VT released:** the request answers `Failed` at once, as Legacy's
  `begin_crtc_config` does today (`Interrupted`). The user's VT golden capture
  is the reference for what a requester sees.
- **DPMS off:** not a prerequisite — the mode installs dark (section 3.3).

### 7.5. Changes no request caused (obligation 6)

Hotplug, VT acquire and recovery publish from their own transitions (3c, 3d).
3b-ii defines the interface: such a change **enters the same gate** as a
mutation without a requester, so its publication is ordered with the
clients'; `lastConfigTime` keeps its rule. 3b-ii proves the interface with a
test producer; 3c and 3d connect the real producers.

## 8. Evidence

Every test cites a C.0 §16.1 group. Gates per umbrella §5.3, with every
`crates/yserver/tests/*.rs` file run (not only `--lib`).

### 8.1. 3b-i

- **Fixture path:** `begin_crtc_config` on an Owner device → driver →
  `Tier::Topology` → slot → boundary → promotion, for enable, mode change and
  disable.
- **One test per section 6 row,** each asserting its typed cause.
- **Supersession:** each `REC-4` kind before dispatch (cancelled, `Failed`,
  prepared set released once); after dispatch (the event waits for the
  result).
- **DPMS composition:** a modeset under DPMS-off carries `ACTIVE=0`, the
  projection hook ran, and a later DPMS-on lights the new mode.
- **Two devices:** a modeset on A issues zero lifecycle commits on B and B's
  in-flight composed commits complete; enabling/removing an output shifts
  indices without moving B's buffers; A in direct plus an enable on B
  returns A to retired composition before dispatch; in a mixed server a
  Legacy modeset issues no write to the Owner device.
- **Backend-state differential:** one script of `SetCrtcConfig`s through
  `begin`/`drain`/`finish` on a Legacy and an Owner fixture; the resulting
  RANDR state compared, except the named exceptions.
- **Hardware** (card1, tty, user approval, N ≥ 4 each): HDMI-2 mode change
  and back; HDMI-2 disable/enable; a modeset under DPMS-off followed by
  DPMS-on (the section 3.4 named risk).

### 8.2. 3b-ii

- **Protocol-order gate** (umbrella §4.1 layer 1): two connections (requester,
  listener) on Legacy and Owner fixtures, bytes compared. Cases: the
  idempotent request; supersession by a `REC-4` event; two concurrent clients
  (the gate, including the section 7.1 MATE sequence); cross-device and
  cross-transport order; the requester disconnecting while parked; each
  section 7.3 stage expired; the VT released; a requester-less mutation from
  the test producer. Identical except the named exceptions.
- **Hardware:** the 3b-i sequences driven by an in-test protocol client
  (C.0 §18 forbids an environment flag selecting the Owner, so real `xrandr`,
  MATE and CS2 are stage 5's layer-3 battery).

### 8.3. Mutations the plans must name (criteria, not edits)

1. `Success` answered for a rejection → a section 6 row test fails.
2. A dispatched modeset cancelled as never-submitted → the supersession test
   fails.
3. Scene state re-associated by index instead of `OutputKey` → the
   index-shift test fails.
4. `ACTIVE=1` regardless of `dpms_target` → the DPMS-composition test fails.
5. The gate admitting a second mutation → the concurrent-clients case fails.
6. Disconnect dropping the publication → the disconnect case fails.
7. Two failure causes collapsed into one → the cause test fails.
8. An owner refusal reported as `KernelRejected` → the cause test fails.
9. The Legacy all-off in a mixed server not scoped to Legacy devices → the
   mixed-server test fails.
10. Each routed value-dead read restored to `kms_outputs_active` → its test
    fails.

### 8.4. Named exceptions (complete list)

1. Modeset under DPMS-off: the Owner keeps the outputs dark (section 3.4).
2. Memory: an allocation failure while the old pool is live answers `Failed`
   (section 4.1).
3. The concurrent PRIME probe no longer goes stale (section 7.1).
4. Other devices do not blank during a modeset (section 5; physical, not
   client-visible).

## 9. Out of scope

VT, hotplug, reprobe and device add/remove as executed transitions (3c);
recovery out of `Poisoned`, `ExecutorStalled`, shutdown (3d); cursor and
gamma on the Owner (stage 4); activation by capability (stage 5); making
direct scanout multi-device. No new protocol surface (umbrella §7).
