# Stage 3a-ii — DPMS execution: the arbiter meets production

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for GPU work) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters, each by its own command — `c0_3aii_`, `c0_3a_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm` — with `--include-ignored` (the GPU is used only with the user's approval, recorded in the prompt); **never** `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master: the hardware test of Task 9 is **written, never run**, by the implementer; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code or in C.0, stop and report it (F8); never silently substitute a test shape, never weaken an existing assertion.

**Revision 3 (2026-09-23)** — anchors re-pointed after the upstream merge
`a232d2af` (joske/master `14f5df87`), which shifted `render/backend.rs` by 80–120
lines; Task 1 names the second `MechanismFailed` site (the Legacy drain route).

**Revision 2 (2026-09-23)** — incorporates codex round 1
(`../findings/2026-09-23-stage-3a-ii-plan-review-round1.md`: 2 blocking, 4
major, all verified): the driver is kicked by the projection itself (B-1);
a new output inherits the global level before installation (B-2); tests for
a supersession behind a delayed executor call (M-1), a completion-only
off-CRTC queue (M-2), the destroy-while-off/while-lit equivalence with the
accepted 2c-iii addendum as prerequisite (M-3), and an Owner vblank arm
surviving a Legacy off (M-4); Task 7 mutates every inventoried read.

**Revision 1 (2026-09-23, coordinator).**

**Goal:** The pure lifecycle layer of plan 3a-i drives production on an
Owner device, and global X11 DPMS runs on it as one atomic transition per
device — the arbiter's first production caller. Production devices stay
`Legacy` (C0-R8): everything here is reachable only with an Owner transport,
i.e. in fixtures, until stage 5.

**Authority** (read before Task 1):
- Stage 3a design `docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`
  revision 6 (user-approved), whole — this plan executes its §3.3–§3.8, §4, §5.
- Stage 3 umbrella `docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md`
  revision 5, §2 and §4.1.
- C.0 `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
  §6.4, §9.2, §10 (terminal states, lifecycle table, latch scopes), §10.1,
  §10.3 including the **Bootstrap** paragraph (§16.3 revision 4), §16.2 items
  9, 39, 45, 65.
- The accepted pure layer: `docs/superpowers/findings/2026-09-23-stage-3a-i-plan-accepted.md`
  and `crates/yserver/src/kms/owner/lifecycle/`.

## Design decisions this plan fixes

1. **The driver** lives in the backend beside `admission_conductors`, one per
   Owner device, and is the only code that applies arbiter actions and feeds
   acknowledged outcomes back (umbrella §2.3.1). It runs at the owner-event
   routing site (`route_owner_event_batch`, `render/backend.rs:20884`). The
   coordinator is a backend field. A Legacy device has neither.
2. **Test names start with `c0_3aii_`**; Vulkan tests end in `_vulkan` with
   `#[ignore = "needs live Vulkan ICD"]` on the Owner live fixture
   (`for_tests_with_vk_live_scene_real_drm`, identity from
   `vk.selected_drm_identity.primary`); stub-executor fixtures where no Vulkan
   is needed. The hardware test is `c0_hw_3a_dpms_owner_on_card1_drm`.
3. **Tests come from production entries**: no hand-built `CommitResources`,
   `OwnerBuffer`, `PendingAck`, `BoPhase`, direct frame or owner record; owner
   milestones reach consumers only through `route_owner_event_batch`.
4. **Test cost**: the ordinary `--lib` suite stays near its 2 s; no factorial
   enumeration.

## Task 1 — the driver and the coordinator in the backend

**Deliver:** the backend owns one `LifecycleCoordinator` and, per Owner
device, the device's arbiter; the driver applies every arbiter action for that
device (close/reopen admission through the conductor, cancel pre-submit work,
terminalize Presents, request quarantine transfer, request topology work) and returns each **receipt tagged with the requesting transition** as an
arbiter input. **The driver is kicked by the projection itself** *(rev 2,
B-1)*: when the coordinator projects an event into an Owner device's arbiter,
the resulting actions are applied in the same call (or on a wake the same
call schedules), and every receipt re-enters the arbiter as it is produced —
a DPMS request never waits for an unrelated owner event to be routed. `MechanismFailed` on an Owner device (`render/backend.rs:21266`, today
`request_exit()`) is reported to the coordinator as a completion loss (C-5)
and enters `Poisoned` through Table U; the branch recording a failed Legacy handover (`legacy_handover_failed`)
keeps its behavior for a device that is not `Owner`, and the two are
distinguished **by the device's transport**. The **second** `MechanismFailed`
site, in `dispose_legacy_drain_event` (`render/backend.rs:20776`), is the Legacy
drain route and keeps its exit unchanged *(rev 3)*.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3aii_owner_mechanism_failure_poisons_and_keeps_running` | an Owner fixture, an owner `MechanismFailed` routed through `route_owner_event_batch`: the device's §6.4 state is `Poisoned`, admission closed, `request_exit` never requested; a Legacy-handover failure on a non-Owner device still exits as today | **D1** restore the unconditional `request_exit()` |
| `c0_3aii_driver_returns_receipts_with_their_own_tag` | a DPMS transition superseded by a second: the first transition's late receipts are reported with its tag and never open the second's gate | **D2** tag receipts with the device's current transition instead of the requester's |
| `c0_3aii_dpms_starts_on_an_idle_device` | an Owner device with no commit in flight and no pending owner event: `set_dpms_power(off)` alone leads to the topology request and the dispatch | **D34** apply the arbiter's actions only at `route_owner_event_batch` |
| `c0_3aii_legacy_device_has_no_arbiter` | a Legacy device: no coordinator projection, no driver, no behavior change (a named Legacy characterisation test stays green) | **D3** create a driver for every device |

## Task 2 — `Tier::Topology` carries the transition, freshness before submission

**Deliver:** `Admission::request_topology` and `Tier::Topology` carry the
`TransitionTag` instead of a bare `u64` (`admission/intents.rs:180`,
`admission/decide.rs:451`). The conductor's `Admitted::Topology` arm
(`render/admission.rs:2041`, today `Unsupported`) dispatches through a new
path modelled on `admission_dispatch_unflip` (`:1152`). **Before the final
`TEST_ONLY` and again before executor dispatch** it compares the tag with the
device's current incarnation, epoch and transition; a stale entry is cancelled
as never-submitted and reported to the arbiter. A supersession removes the
queued entry at once. Every terminal state of a topology commit crosses one
**result-disposition boundary** (3a design §3.6 table) before reaching the
arbiter: current tag → `Completed`/`FailedBeforeSubmit`/`CompletionUnknown`;
stale explicit success → accepted-stale (fds adopted or closed once, both state
sets quarantined, no promotion); stale rejection → never-submitted cleanup;
stale absent/invalid → acceptance-unknown, quarantined.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3aii_stale_topology_never_reaches_test_only` | a DPMS off queued, superseded by on before dispatch: the off never reaches final `TEST_ONLY` nor the executor (C.0 §16.2 item 9) | **D4** drop the pre-`TEST_ONLY` check; **D5** drop the pre-dispatch check (with the first kept, supersede between the two) |
| `c0_3aii_winner_waits_for_a_delayed_executor_call` | an off whose host call is deliberately delayed (the stub executor's delay control), superseded by on: the on stays queued until the off's call returns or is reaped, then dispatches; the off's late result goes through the boundary as stale | **D35** release the slot to the winner before the delayed call returns |
| `c0_3aii_result_boundary_rows` | every row of the 3a design §3.6 table through `route_owner_event_batch` | **D6** promote a stale explicit success |
| `c0_3aii_topology_payload_is_the_tag` | the compile-level shape: a `u64` can no longer be passed as a topology payload (compile-fail test in the existing harness) | — |

## Task 3 — the DPMS commit and the lifecycle deadline bootstrap

**Deliver:** the section 3.4 description of the 3a design: **off** serializes
`ACTIVE=0` for every CRTC of the device's powered-on projected outputs,
keeping `MODE_ID`, routing and each primary plane's `FB_ID`/`CRTC_ID`; every
such CRTC is in `ExpectedCompletionCrtcs` as old-active with a required
out-fence; **on** serializes `ACTIVE=1` over the retained state;
`ALLOW_MODESET` set; the final `TEST_ONLY` first; **no per-output fallback**.
The commit's completion class is `LifecycleInstallRestore` with the 2 s
`NONBLOCK` host-call watchdog and `HardwareComplete` as its milestone.

**Deliver also — a defect of 2b against C.0 revision 4 (found while writing
this plan):** C.0 §10.3's Bootstrap paragraph requires that, with no measured
`LifecycleCompletionObservedMax`, the lifecycle hardware deadline is the 30 s
ceiling. Today the owner **refuses to dispatch** any lifecycle-class commit
without a measurement (`owner/device.rs:1312`,
`DispatchError::LifecycleUnvalidated`), and `deadlines::lifecycle_hardware`
returns `LifecycleUnvalidated` for `None` (`owner/deadlines.rs:37`, whose unit
test asserts exactly that). Both follow the pre-amendment text. Implement the
bootstrap: `None` → 30 s; a measured value keeps today's formula and its 28 s
limit; nothing observed under the bootstrap is persisted. Update the unit test
to C.0's current text (this is the one existing assertion this plan changes,
and it changes because the authority changed — say so in the code comment).

**Unify** the DPMS level→target mapping of 3a-i (three copies: coordinator
twice, arbiter once) into one function used by all three.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3aii_dpms_off_is_active_only` | the built off request: only `ACTIVE=0` per affected CRTC, `MODE_ID` and plane properties untouched, every old-active CRTC in the expected set with an out-fence | **D7** detach the primary plane; **D8** drop an old-active CRTC from the expected set |
| `c0_3aii_combined_rejection_has_no_per_output_fallback` | a two-CRTC device whose combined off is rejected: no per-output commit follows | **D9** retry each CRTC alone |
| `c0_3aii_lifecycle_deadline_bootstrap` | no measurement → 30 s; 10 s, 28 s → today's formula; 29 s → unvalidated; a lifecycle-class dispatch with no measurement is accepted | **D10** keep refusing `None` at dispatch |
| `c0_3aii_dpms_commit_uses_the_lifecycle_class` | the DPMS commit's completion context: lifecycle class, `NONBLOCK` watchdog, no primary-event timer | **D11** build it as `FastUpdate` |
| `c0_3aii_one_dpms_level_mapping` | levels 0–3 through the one function, used by coordinator and arbiter | **D12** map level 1 to on in the shared function |

## Task 4 — the transport fork of `set_dpms_power`

**Deliver (3a design §3.3):** `set_dpms_power` (`render/backend.rs:31075`)
splits per device. Legacy devices keep today's code **restricted to Legacy
devices**: the loop of `dpms_set_outputs_active` (`render/platform.rs:8184`)
iterates only their outputs, and every other server-wide step of the off and
on paths is inventoried by the implementer and either scoped to Legacy devices
or proven harmless to an Owner device — at least `scene.drain_all`,
`platform.reset_scanout_bos_for_suspend()` (`render/backend.rs:31198`–`31200`),
vblank-target clearing, cursor re-arm, gamma reapply, `wake_for_damage`.
Owner devices go to the coordinator (projection, idempotence from the
arbiter's targets, never `kms_outputs_active`). **A new output inherits
before installation** *(rev 2, B-2; umbrella rev-2 M-2)*: at the site where
a stable protocol output joins an Owner device's domain (the implementer names
it; today's discovery/registration path), the coordinator's current level and
epoch are projected onto it **before** any installation can light it, and a
removed output's projection is invalidated exactly once. Executed hotplug is
3c's; the hook and its test are 3a's. **The resource service's
serviced-time clock** (`set_seat_active`, `render/backend.rs:31214`,
`resources/mod.rs:319`) runs while any served output of any device is lit.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3aii_dpms_off_on_a_mixed_server_does_not_exit` | one Legacy and one Owner device, off then on: neither exits; the Legacy outputs follow Legacy, the Owner device gets one atomic transition | **D13** let the legacy loop iterate Owner outputs |
| `c0_3aii_legacy_off_leaves_owner_scanout_state` | a Legacy off on a mixed server: the Owner device's scanout BOs and scene state are untouched | **D14** call `reset_scanout_bos_for_suspend` server-wide again |
| `c0_3aii_new_output_inherits_the_global_off` | global off applied; a new output joins the Owner device's domain through the production registration path: its projected target is off before any installation, so it is never installed active | **D36** add the output with target on |
| `c0_3aii_legacy_off_keeps_owner_vblank_arms` | *(rev 2, M-4)* a mixed server with a lit Owner CRTC holding an armed vblank target: a Legacy off leaves that arm in place | **D37** clear vblank targets server-wide again |
| `c0_3aii_resource_clock_runs_while_any_output_lit` | a pending resource batch, Legacy off, Owner off rejected: the clock keeps running; both off: it pauses | **D15** pause on Legacy off alone |

## Task 5 — while off, and back on (Owner)

**Deliver (3a design §3.5):** the transition closes admission (§6.4
`Quiescing`) and never overtakes a `Submitting`/accepted commit; after off is
`Applied` the device is `Ready` with its outputs powered off: composed offers
for an off CRTC wait with a new `WaitReason::OutputPoweredOff`, off CRTCs are
direct-ineligible, and the buffer bound to an off primary plane — composed or
a direct client buffer — stays current and pinned. After on is `Applied`,
admission reopens, the scene is marked for a full frame, and a direct unit that
is no longer eligible returns through the **ordinary** Ciii unflip. An unflip
requested while off is kept as the admission's intent and decided after on.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3aii_off_waits_for_the_accepted_predecessor_vulkan` | an accepted composed commit when off arrives: the off is dispatched only after it terminalizes | **D16** dispatch the off over an accepted commit |
| `c0_3aii_composed_waits_while_off_vulkan` | a composed offer on an off CRTC waits with `OutputPoweredOff`; after on it is admitted | **D17** admit it while off |
| `c0_3aii_retained_buffer_survives_off_vulkan` | the current composed buffer is the same object before off and after on, and is never released while off | **D18** release the current buffer at off |
| `c0_3aii_direct_stays_pinned_through_off_vulkan` | a direct client buffer current at off: no unflip before off; the client destroys its window while off and the allocation survives; no direct successor admitted while off; after on the ordinary unflip returns to composed | **D19** admit a direct successor on an off CRTC; **D20** release the direct allocation while off |
| `c0_3aii_destroy_while_off_equals_destroy_while_lit_vulkan` | *(rev 2, M-3; prerequisite: the 2c-iii addendum `3154115e`, direct entry needs a composed return)* the same direct scenario twice — the client destroys its window while off then the device is turned on, and the client destroys it while lit — reaches the same unflip readiness (exit retirement, composed return, shadow) and the same outcome | **D38** drop the retained composed return at off |
| `c0_3aii_unflip_requested_while_off_is_kept_vulkan` | an unflip requested while off is decided after on, not lost | **D21** drop the unflip intent at off |

## Task 6 — blackout per CRTC, both core sweeps

**Deliver (3a design §3.5, round-5 M-1):** the `Backend` trait's blackout
query (`yserver-core/src/backend/trait_def.rs:1086`, today one boolean)
answers **per target CRTC**; Legacy CRTCs keep Legacy's all-or-nothing
answer, Owner CRTCs answer from the arbiter's installed power. Both core sweeps
(`yserver-core/src/core_loop/process_request.rs:10299`, `:10379`) filter by
each entry's target: an entry on a blacked-out CRTC is executed or flushed as
today, an entry on a lit CRTC keeps its MSC due check; a `source_ready ==
false` entry is never forced; per-window order is kept (a window's entries are
flushed in order and the flush stops at its first non-flushable entry).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3aii_blackout_is_per_crtc` | a lit CRTC and an off CRTC, future-target Presents on both: only the off CRTC's flush; the lit one keeps its timing | **D22** answer blackout globally |
| `c0_3aii_blackout_flushes_a_completion_only_queue` | *(rev 2, M-2)* an off CRTC whose execution queue is empty but which holds a parked completion: completion, `IdleNotify` and release are delivered exactly once without waiting for its frozen clock — the core's early return at `process_request.rs:10290` must not skip it | **D39** keep the early return on "no pending execution and no global blackout" |
| `c0_3aii_blackout_keeps_window_order` | one window with Presents on both CRTCs: never a later one completed before an earlier one | **D23** flush per CRTC ignoring window order |
| `c0_3aii_blackout_never_forces_a_waiting_source` | a `source_ready == false` entry on an off CRTC stays parked | **D24** force it |
| `c0_3aii_legacy_blackout_unchanged` | a Legacy-only server: the core's bytes are identical to today's for a DPMS-off/Present script | **D25** drop Legacy's all-or-nothing answer |

## Task 7 — `kms_outputs_active` on Owner-reachable paths

**Deliver (3a design §3.8):** the implementer inventories every read of
`kms_outputs_active` reachable while the device is `Owner` (compositing
gates, wakeup computation, relight helpers, the direct eligibility input,
`present_scanout_blackout`) and routes each to the per-device power state;
reads reachable only on Legacy devices stay. The inventory goes in the report
with file:line and the decision for each.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3aii_owner_ignores_kms_outputs_active_vulkan` | an Owner fixture with `kms_outputs_active` forced to the wrong value in each state (off/on): composition, wakeups, eligibility and blackout behave as the arbiter's power state says | **D26** restore each inventoried Owner-reachable read in turn — one mutation per read, each caught by this test (the report lists them); a read no test catches is an F8 |

## Task 8 — failure edges and the seat

**Deliver (3a design §3.7, §3.3):** a rejected DPMS (`FailedBeforeSubmit`)
leaves the previous state authoritative, quarantines nothing, and the
representative is `Deferred(TopologyLatched(gen))` for an attributable
`EINVAL`/`EOPNOTSUPP` or `Deferred(ReadinessClosed)` otherwise, never retried
under the same generation; a completion loss enters `Poisoned` through Table
U; while `Poisoned` a DPMS change is logical only. The seat target is fed
read-only from `run_suspend`/`run_resume` (`render/backend.rs:13845`,
`:13973`) into the coordinator; a DPMS request while the seat is released is
`Deferred(SeatReleased)` and converges on reacquire. Capability stability:
the advertised cursor/primary capability does not change across DPMS cycles
or an injected completion loss (C.0 §16.2 item 39).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3aii_rejected_off_is_deferred_and_not_retried` | an attributable and a non-attributable rejection: the right prerequisite, no retry under the same generation, a newer request supersedes | **D27** retry immediately |
| `c0_3aii_lost_off_fence_poisons_vulkan` | an injected missing off fence and an injected late one: expiry at the lifecycle deadline → `Poisoned`; the late fence never promotes | **D28** time the off with the fast clamp |
| `c0_3aii_poisoned_dpms_issues_no_commit` | `Poisoned` device, off then on: no topology request reaches the conductor | **D29** request the commit while `Poisoned` |
| `c0_3aii_dpms_while_seat_released_is_deferred` | seat released (via the `run_suspend` feed), DPMS off: `Deferred(SeatReleased)`, no commit; on reacquire it converges | **D30** apply it while released |
| `c0_3aii_capability_stable_across_dpms_and_poison_vulkan` | capability captured before, compared after four off/on cycles and after an injected completion loss | **D31** recompute capability on DPMS |

## Task 9 — the differential gate and the hardware test

**Deliver:**
- **Backend-state differential** (umbrella §4.1 layer 1): one DPMS script
  (off → info → on → info ×4, standby, suspend, off while off, a request while
  the seat is released) on a Legacy fixture and an Owner fixture; compare each
  result's status and the resulting power state.
- **Protocol differential**: drive the **core** request path with both
  fixtures and compare the bytes written to the requesting client **and to a
  second, listening connection** — identical; DPMS has no event, so any byte to
  the listener is a defect.
- **The hardware test** `c0_hw_3a_dpms_owner_on_card1_drm`, beside
  `c0_hw_ciii_owner_route_on_card1_drm` (`render/backend.rs:67990`) with the
  same conventions, **written and compiled, never run by the implementer**:
  off/on × 4 on card1, each off's out-fence observed signalled with no later
  vblank, the retained buffer unchanged, composed frames admitted again after
  each on — the fourth cycle as well as the first. If NVIDIA rejects the
  `ACTIVE`-only shape or never signals the off fence, the test reports that as
  its result (the transition fails per Task 8), never as a pass. The report
  states the exact command line the coordinator runs from tty2.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3aii_dpms_differential_backend_state` | the script above, Legacy vs Owner | **D32** leave an Owner output lit after an applied off |
| `c0_3aii_dpms_differential_protocol_bytes` | the core path, requester and listener, Legacy vs Owner | **D33** emit any event to the listener on an Owner DPMS change |

## Gate (every task)

`cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings`, again with
`--features tcp-transport` and `--features xdmcp`; each permitted filter by its
own command (ignored families with `--include-ignored`, only with the user's
GPU approval); `cargo test -p yserver --test compile_fail`, then
`git status --short` (only the task's changes); `cargo test -p yserver-core`
when a task touches it (Task 6); `cargo test -p yserver --lib` (count and
time). New tests of a task also run in `--release` when the task touches
production code paths (a state change hidden in `debug_assert!` once passed
debug and failed release); grep new code for `debug_assert!(self.`. Mutations
by line, compiled, named failing test, restored exactly.

## Limits

Fixture level, plus one hardware test the coordinator runs with the user's
approval. Production stays `Legacy`. VT, hotplug and client modesets as
executed transitions are 3b/3c; recovery exits are 3d; cursor and gamma are
stage 4. The second device's DPMS delivery (the Raphael iGPU) is C.0's
final-tip bounded delivery check (3a design §5.3.1).
