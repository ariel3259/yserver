# Stage 3b-i-1 — client modeset execution on the Owner: the transaction

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back), run with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters, each by its own command — `c0_3bi_`, `c0_3aii_`, `c0_3a_`, `c0_2b_add_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm` — with `--include-ignored` only when the prompt records the user's GPU approval, otherwise without it; **never** `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master: the hardware test of Task 9 is **written, never run**, by the implementer; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code or in C.0, stop and report it (F8); never silently substitute a test shape, never weaken an existing assertion.

**Revision 1 (2026-09-24, coordinator).**

**Goal:** an `RRSetCrtcConfig` that enables, changes the mode of, or disables
an output of an **Owner** device runs as one atomic `ALLOW_MODESET | NONBLOCK`
transaction on that device, prepared with the old topology lit and promoted
infallibly — the modeset arbiter's production caller. Same-device scanout
route only; the copied route, a direct frame on the device, position-only
changes and the mixed-server Legacy scoping are plan 3b-i-2's. Production
devices stay `Legacy` (C0-R8).

**Authority** (read before Task 1):
- Stage 3b design `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
  revision 8, whole — this plan executes §3.1–§3.5, §4, §5.1 (same-device
  part), §6 and §8.1's matching evidence.
- Stage 3a design `docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`
  revision 6 (§3.6 freshness and the result boundary, §3.7).
- Stage 3 umbrella `docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md`
  revision 6, §2 and §3b.
- C.0 `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
  §6.3 (closure and `ExpectedCompletionCrtcs`), §9.2, §9.4, §10 (terminal
  states, latch scopes, error classes at line 1985, clock probe at line
  1769), §10.2, §10.3.
- The 3a-ii acceptance `docs/superpowers/findings/2026-09-24-stage-3a-ii-plan-accepted.md`.

**Out of this plan (3b-i-2):** the copied route (preparation on two devices,
the retired copied pool table of design §5.1); the composed unflip before
dispatch (§5.3); position-only changes (§3.3); the Legacy scoping in a mixed
server (§5.5); the two-device tests. Until 3b-i-2, an Owner modeset that
needs any of them answers `Failed` with
`OwnerRefused(NotYetSupported(<which>))` — a named, temporary refusal the
3b-i-2 plan removes; it is reachable only in fixtures. **Out of 3b-i
entirely (3b-ii):** the RANDR gate, publication without a requester, the
bound. In this plan the core's existing `Pending` path carries the request;
a requester that disconnects after dispatch loses the publication until
3b-ii (the installed backend state is still correct).

## Design decisions this plan fixes

1. **Test names start with `c0_3bi_`**; Vulkan tests end in `_vulkan` with
   `#[ignore = "needs live Vulkan ICD"]` and use the Owner live fixture
   (`owner_live_fixture`, `render/backend.rs:53291`); everything that needs no
   Vulkan uses the stub-executor fixture of 3a-ii
   (`lifecycle_dpms_backend_with_output_count`, `:59387`). The hardware test
   is `c0_hw_3b_modeset_owner_on_card1_drm`.
2. **Tests come from production entries**: a modeset is started through
   `Backend::begin_crtc_config` (or a helper that calls it); no hand-built
   `CommitResources`, `OwnerBuffer`, `PendingAck`, `BoPhase` or owner record;
   owner milestones reach consumers only through `route_owner_event_batch`.
3. **One client-modeset slot per device, in the lifecycle driver**, beside
   the `REC-4` transition (design §3.1). Its identity is `ClientModesetId`
   (monotonic `u64`, checked, never wrapping — the `LifecycleTransitionId`
   pattern, `owner/lifecycle/ids.rs:50`).
4. **`Admitted::Topology` carries a `TopologyWork`**, not a bare
   `TransitionTag` (`owner/admission/decide.rs:462`):
   `enum TopologyWork<I> { Transition(TransitionTag<I>), ClientModeset(ClientModesetTag<I>) }`
   with `ClientModesetTag<I> { incarnation: I, lifecycle_epoch: LifecycleEpochId, topology_generation: u64, modeset: ClientModesetId }`.
   `Admission::request_topology` takes a `TopologyWork`. The result boundary
   (umbrella §2.4) compares a client tag with the device's current
   incarnation, epoch, topology generation and slot occupant exactly as it
   compares a transition tag.
5. **Test cost**: the ordinary `--lib` suite stays near its 2 s; no factorial
   enumeration.
6. **Every gate** (per task, by the implementer, and again by the
   coordinator): `cargo +nightly fmt --check`; `cargo clippy --all-targets
   -- -D warnings` in default, `--features tcp-transport` and `--features
   xdmcp`; the filters above; `cargo test -p yserver --lib -- --skip c0_2ci`;
   `c0_2ci` separately; `cargo test -p yserver-core`; and **every integration
   file** `cargo test -p yserver --test <each crates/yserver/tests/*.rs>`.

## Review Focus

Inputs the design implies that no single task's table would otherwise hit:

1. A modeset requested while a DPMS transition of the same device is in
   flight — the modeset waits for the transition and does not dispatch
   early (Task 2 test `c0_3bi_modeset_waits_for_the_active_transition`).
2. The same `SetCrtcConfig` sent twice in a row (the MATE re-assert) on an
   Owner device — the second is idempotent: `Ok(false)`, no dispatch
   (Task 9 test `c0_3bi_idempotent_request_dispatches_nothing`).
3. An enable of the output that was the **last** one disabled — the device
   goes from zero lit CRTCs to one; the root extent and input extent follow
   (Task 6 test `c0_3bi_enable_from_headless_device`).
4. A mode change to the mode already installed but a different refresh —
   treated as a mode change, not idempotent (Task 4 test
   `c0_3bi_refresh_only_change_is_a_modeset`).
5. A modeset whose executor call never replies — the 2 s host-call watchdog
   ends it `CompletionUnknown`, the device `Poisoned`, the request `Failed`
   (Task 8 test `c0_3bi_host_call_timeout_poisons`).

---

## Task 1 — clocks of CRTCs that were dark (design §3.5)

**Deliver:** a lifecycle-class commit (`CompletionClass::LifecycleInstallRestore`,
`owner/completion.rs:14`) requires a ready `KernelSequence` clock **only for
the expected-completion CRTCs that are active before the commit**
(`old_active`). This changes both the owner's check
(`validate_completion_context`, `owner/device.rs:1341`, `required_clock_crtcs`)
and the driver's wait (`lifecycle_clock_readiness`, `render/admission.rs:1203`,
and its caller's `expected_crtcs`, `:1387`). The fast-update class is
unchanged. A clock probe is **never** sent for a CRTC whose installed power is
inactive; a CRTC that becomes active with an `Unresolved` epoch gets its probe
at the promotion that lit it (the probe activation of the 2b addendum,
`activate_admission_clock_probes`, is the mechanism; this task adds the
"only when active" guard and the "at the lighting promotion" trigger for the
DPMS-on path; Task 6 adds it for modeset promotion). Existing tests that
encode the stricter rule for an inactive-to-active CRTC are listed in the
report with the reason each changes; no other assertion is weakened.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_dark_crtc_needs_no_clock_to_light` | an Owner device whose CRTC is off by proof and whose clock epoch is `Unresolved` (a fresh epoch): DPMS-on validates and dispatches without waiting and without a probe host call before the commit | **E1** require clocks for every expected-completion CRTC again |
| `c0_3bi_lit_crtc_still_waits_for_its_clock` | an active CRTC with its probe pending: a DPMS-off waits (no validation send) until the probe reply, then dispatches once | **E2** drop the requirement for `old_active` CRTCs too |
| `c0_3bi_no_probe_to_an_inactive_crtc` | a new epoch on an inactive CRTC: no `GET_SEQUENCE` host call is sent; after DPMS-on is `Completed`, exactly one probe is sent | **E3** probe at epoch creation regardless of power; **E4** never probe after the lighting commit |

## Task 2 — the client-modeset slot and `TopologyWork`

**Deliver:** design decision 3 and 4. The driver holds at most one client
modeset per Owner device. It is **dispatched** only while the device is
`Owner ∧ Ready`, the arbiter has **no active transition**, and every clock
Task 1 requires is ready. A second `begin` for a device whose slot is
occupied answers `OwnerRefused(SlotOccupied)`. **Supersession** (design
§3.2): a `REC-4` event projected to the device before the modeset is
dispatched cancels it as never-submitted (its prepared set, when Task 5
exists, released exactly once; here the slot empties and the result is
`Superseded(kind)`); after dispatch the arbiter records the event as desired
and its transition starts only once the modeset's result crossed the
boundary. `REC-4` events never wait for the slot. Freshness is checked
immediately before the final `TEST_ONLY` and again before executor dispatch
(3a design §3.6), comparing the whole `ClientModesetTag`. The work itself is
still a stub here (Task 4 builds the description): a test-only description
source may be used, provided no production path can reach it.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_topology_work_is_typed` | a transition and a client modeset queued on two fixtures: each `Admitted::Topology` carries its own `TopologyWork` variant, and a client result cannot be consumed as a transition result (compile-level: the boundary matches on the variant; runtime: a client tag with a stale generation is refused) | **E5** compare only incarnation and epoch for a client tag |
| `c0_3bi_modeset_waits_for_the_active_transition` | DPMS-off in flight (stub `NeverReply` until released), then a modeset begin: no modeset validation is sent until the DPMS transition's result is at the boundary | **E6** dispatch the modeset while a transition is active |
| `c0_3bi_rec4_supersedes_before_dispatch` | modeset queued, a DPMS event arrives before dispatch: the modeset ends `Superseded(DPMS)` with no validation or live send, and the DPMS transition proceeds | **E7** let the queued modeset dispatch first |
| `c0_3bi_rec4_waits_after_dispatch` | modeset dispatched (stub holds the reply), DPMS event arrives: the modeset is not cancelled; the DPMS transition's first send happens only after the modeset's reply crossed the boundary | **E8** cancel a dispatched modeset as never-submitted (design mutation 2) |
| `c0_3bi_second_modeset_on_occupied_slot` | two begins on one device: the second answers `OwnerRefused(SlotOccupied)`, the first is untouched | **E9** replace the occupant |

## Task 3 — output state addressed by identity; the retired-output bundle

**Deliver:** design §5.1 (same-device part). Today scene output state
(`OutputSceneState`, `render/scene.rs:485`) and `platform.scanout_pools` are
indexed by position in `platform.outputs`. After this task:

- a promotion that inserts or removes an output **re-associates every kept
  scene state and pool by `OutputKey`** from a staged identity map, and the
  kept objects are the same objects before and after (no rebuild);
- the scene rebuild used by the Owner modeset is **per output**
  (`rebuild_output(key, staged_state)`), never `rebuild_outputs` (which stays
  for Legacy);
- a replaced or removed output's scene state and its old pool leave the
  indexed vectors together as a `RetiredOutputBundle { key: OutputKey,
  retirement: RetirementId, scene: OutputSceneState, pool: OutputScanout }`
  held on a per-device retirement list; every operation on it — deferred
  release polling (`drain_deferred_scene_resources`, `scene.rs:394`),
  owner-buffer displacement and leave (`retire_owner_displaced`, `:5062`),
  `KmsRelease` handling — has a bundle-addressed form, and the index form is
  never called for a retired output;
- every asynchronous completion that today looks up output state by index
  (composition submits, deferred releases; the copied completion path is
  3b-i-2's) resolves through its job's identity to a kept output's
  `OutputKey` or to its bundle; a completion routed to a bundle only services
  proofs and never offers a generation or submits;
- a bundle is destroyed only after every resource in it has its own proof;
  one whose proof never arrives stays on the list (the 2c-i teardown handoff
  owns it at shutdown).

This task changes no RANDR behaviour; the Legacy path keeps its indices and
its whole-scene rebuild.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_index_shift_keeps_other_outputs_vulkan` | three outputs on the Owner fixture; the middle one removed through the promotion helper: the third output's scene state and pool are the same objects (pointer/identity check) now at index 1, its pending acknowledgement and owner buffers intact | **E10** re-associate by index (design mutation 3) |
| `c0_3bi_retired_bundle_waits_for_its_fence_vulkan` | an output with a deferred pool release behind an unsignalled fence is replaced: the slot is not freed until the fence signals, the ring outlives it, the new state never receives it | **E11** drop the replaced state at promotion (design mutation 17) |
| `c0_3bi_retired_bundle_survives_index_shift_vulkan` | disable output A with an unsignalled deferred release, then B shifts into A's index: A's release is serviced from its bundle; nothing of B is touched | **E12** call an index-addressed helper for the retired output (design mutation 23) |
| `c0_3bi_late_completion_routes_by_identity_vulkan` | a composition submit for output B completes after an index shift: it resolves to B by key; one for a retired output resolves to its bundle and offers nothing | **E13** resolve the completion by its recorded index (design mutation 25) |

## Task 4 — the transaction description and the staged projection

**Deliver:** design §3.3 and §3.4. A builder produces the device commit's
`CommitDescription` for an enable, a mode change or a disable of one output:

- target output, enable or mode change: connector `CRTC_ID`, the CRTC's
  `MODE_ID` (a new mode blob, owned by the prepared set) and the primary
  plane (`FB_ID`, `CRTC_ID`, `SRC_*`, `CRTC_*`) with the prepared
  framebuffer; `ACTIVE` = the output's **staged** `dpms_target`;
- target output, disable: connector `CRTC_ID` = 0, primary plane `FB_ID` = 0
  and `CRTC_ID` = 0, the CRTC `ACTIVE` = 0 and `MODE_ID` = 0;
- every other output of the device: absent from the property list (C.0 §6.3
  minimal list);
- `crtc_state` gives each touched CRTC's `old_active`/`new_active` from the
  owner's installed-power record, so `ExpectedCompletionCrtcs` follows C.0
  (active before or after; inactive-to-inactive excluded, no out-fence);
- the **staged projection**: for an output the commit adds, its target is read
  from the coordinator's current global level and epoch (a pure read) at
  preparation and recorded in the prepared set with that epoch; the freshness
  check treats a change of the global DPMS epoch as stale.

A refresh-only change (same size, different `vrefresh`) is a mode change.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_description_is_minimal` | two outputs on one device, mode change on the first: the property list names only the first output's connector, CRTC and primary plane | **E14** restate the other output |
| `c0_3bi_modeset_under_dpms_off_is_dark` | global DPMS-off applied, then an enable of a disabled output: the description carries `ACTIVE=0` for its CRTC and the CRTC is not in `ExpectedCompletionCrtcs` | **E15** `ACTIVE=1` regardless of `dpms_target` (design mutation 4); **E16** stage the projection at promotion only (design mutation 28) |
| `c0_3bi_dpms_change_after_staging_is_stale` | an enable staged under DPMS-off, then DPMS-on before dispatch: the entry is stale at the pre-dispatch check and the re-prepared description carries `ACTIVE=1` | **E17** skip the DPMS epoch in freshness |
| `c0_3bi_disable_description` | a disable: connector, plane and CRTC cleared as listed, the CRTC in `ExpectedCompletionCrtcs` when it was lit | **E18** leave the primary plane attached |
| `c0_3bi_refresh_only_change_is_a_modeset` | same `WxH`, different refresh: a new `MODE_ID` is described; the idempotency check does not fire | **E19** compare size only |

## Task 5 — preparation with the old topology lit (same-device route)

**Deliver:** design §4.1 steps 1–5 for the same-device route: discovery of
the connector with the device's other outputs' routes reserved
(`discover_output_for_connector`, as `apply_crtc_config` does,
`render/backend.rs:24885` onward); the advertised-mode check; route
selection — a non-same-device route answers
`OwnerRefused(NotYetSupported(CopiedRoute))`; allocation of the output's new
pool **through the 2c-i resource ownership** (never a raw allocation); the
first framebuffer (the first BO of the new pool, uncomposed, as Legacy); the
target output's staged scene state (Task 3); the staged projection (Task 4);
`TEST_ONLY` of the complete description through the 3a-ii validation path
(`lifecycle_finish_topology_validation`, `render/admission.rs:1605`). The
**prepared set** owns the pool, the framebuffers, the mode blob and the staged
scene state; it is released **exactly once** on every non-installing end
(preparation failure, `TEST_ONLY` rejection, supersession, stale entry,
explicit commit rejection) and never while installable. A device with a
current direct frame answers `OwnerRefused(NotYetSupported(DirectActive))`
before allocating (3b-i-2 replaces this with the unflip precondition).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_preparation_keeps_old_topology_lit_vulkan` | a mode change prepared: until dispatch no KMS call except `TEST_ONLY` is sent, the old pool and scene state are untouched and composition keeps ticking | **E20** quiesce or drain before dispatch |
| `c0_3bi_prepared_set_released_once_vulkan` | each non-installing end in turn (discovery failure, unadvertised mode, allocation failure injected, `TEST_ONLY` `EINVAL`, supersession, stale, explicit rejection): every allocation of the prepared set is released exactly once and the resource service reports no leak | **E21** skip the release on one end; **E22** release twice |
| `c0_3bi_new_pool_goes_through_resource_ownership_vulkan` | the prepared pool's allocations are registered with the resource service before `TEST_ONLY` | **E23** allocate through the raw scanout path |

## Task 6 — dispatch and infallible promotion

**Deliver:** design §4.2. On a current explicit success at the boundary:
(1) KMS-state promotion — `platform.outputs`, the output's pool (the new
pool installed; the old one into the output's retired bundle, never
released here), the RANDR registry entry (`config`, `crtc_associated`,
`client_configured`, `connected`, `last_enabled` exactly as Legacy's
`apply_crtc_config` / `finish_crtc_config` set them), the root extent
(`recompute_fb_extent_from`, `render/platform.rs:3064`), the input extent,
the device's topology generation (queued older-generation intents
invalidated), a new clock epoch for each CRTC whose mode changed, probed
when active (Task 1); (2) the staged projection committed and a removed
output's projection invalidated once, through a coordinator form that cannot
fail (its preconditions verified in preparation); (3) scene promotion
(Task 3). **None of the three steps has a fallible call.** After promotion
the result is announced through the existing asynchronous CRTC path so the
core's `finish_crtc_config` answers `Success` (`Ok(true)`).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_mode_change_promotes_vulkan` | a mode change accepted and completed: `platform.outputs`, the registry entry, root and input extents equal Legacy's for the same request; the old pool is in the retired bundle; the new epoch is probed once | **E24** release the old pool at promotion (design mutation 19) |
| `c0_3bi_enable_from_headless_device_vulkan` | the device's only output disabled, then enabled: the promotion lights it (DPMS on), extents follow, a probe follows the lit promotion | **E25** skip the extent recomputation |
| `c0_3bi_disable_promotes_vulkan` | a disable: the output leaves `platform.outputs`, its projection is invalidated once, its scene state and pool are in a bundle | **E26** invalidate the projection twice or never |
| `c0_3bi_promotion_has_no_fallible_call` | the promotion entry point's signature returns no `Result`, and a test double that makes each promoted component's setter observable shows all three steps ran after one accepted result | **E27** place a fallible call after acceptance (design mutation 14) |

## Task 7 — the old pool's release

**Deliver:** design §4.2 "The old pool's release" and "A CRTC dark before and
after". At dispatch, each allocation of the old pool that the commit
displaces registers `KmsRelease` against the commit and CRTC
(`ResourceService::register_kms`, `render/resources/mod.rs:998`). On the lit
path the commit's `CompletionRetired` discharges it (the 2c rule, as at
`backend.rs:54313`). On an explicit rejection every registration is cancelled
exactly once (`ResourceService::cancel`, `resources/mod.rs:920`). For a CRTC
inactive before and after, the typed `DarkCrtcDisplacement { off_commit, crtc }`
proof discharges it when the owner's installed-power record shows an
`ACTIVE=0` commit on that CRTC retired with its successful out-fence in the
current incarnation and every owner commit on it since kept it inactive, and
the displacing commit is `Completed`; otherwise the obligation waits for the
next displacing commit with an out-fence on that CRTC or the device barrier.
GPU and FOREIGN proofs stay separate. Under `CompletionUnknown` nothing is
discharged.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_displaced_pool_waits_for_retirement_vulkan` | mode change on a lit CRTC: the old allocations hold `KmsRelease` after acceptance, discharged only by `CompletionRetired`, destroyed only after the GPU/FOREIGN proofs | **E28** discharge at acceptance |
| `c0_3bi_rejection_cancels_the_displacement_vulkan` | a rejected mode change then a successful one: the rejected registrations are cancelled once, the old pool is discharged by the successful commit | **E29** leave the rejected registrations (design mutation 26) |
| `c0_3bi_dark_crtc_displacement_vulkan` | DPMS-off, then three mode changes and a disable: each displaced pool is discharged by `DarkCrtcDisplacement` at its successor's `Completed` | **E30** issue the dark proof without a proven off (design mutation 21) |
| `c0_3bi_unproven_off_issues_no_dark_proof_vulkan` | the off commit made `CompletionUnknown` in the fixture, then a mode change: no dark proof; the pool stays retained | **E31** treat an absent fence as a proof |

## Task 8 — the failure table and the typed cause

**Deliver:** design §6 and §6.1. `ClientModesetFailure` with exactly:
`Preparation(PreparationStage)` (discovery, mode, route, allocation, scene
state, unflip, `TestOnly { errno }`), `KernelRejected { errno }`,
`OwnerRefused(OwnerRefusal)` (`SlotOccupied`, `ClockNotReady`,
`ReadinessClosed`, `NotYetSupported(..)`), `Superseded(LifecycleKind)`,
`CompletionUnknown`, `Stale`, `Latched`, `SeatReleased` (the `GateExpired`
variant is 3b-ii's). Errno classification: `EINVAL`/`ERANGE`/`ENOSPC` at
`TEST_ONLY` leave the device `Ready`; `EINVAL`/`EOPNOTSUPP`/`ERANGE`/`ENOSPC`
on the real commit are `FailedBeforeSubmit` and an attributable
`EINVAL`/`EOPNOTSUPP` latches `(installed topology generation, requested
configuration)`; `EBUSY` is never retried — readiness closed, evidence
recorded (C.0 §9.4); `EACCES`/`EPERM`/`ENOENT`/`ENODEV` and every
unclassified errno close readiness (C.0 §10, line 1985). Completion loss →
`Poisoned` through Table U (3a-ii). Every failure logs one line with the
client, request sequence, output, requested mode and position, device,
modeset id and cause; each success logs the same at `debug`. The owner never
synthesizes an errno.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_each_failure_has_its_cause` | one case per design §6 row reachable in this plan: the returned cause is that row's, and the log line carries it | **E32** collapse two causes (design mutation 7); **E33** report an owner refusal as `KernelRejected` (design mutation 8) |
| `c0_3bi_ebusy_is_not_retried` | the stub answers `EBUSY` to the live commit: one live send only, readiness closed, evidence recorded | **E34** retry (design mutation 12) |
| `c0_3bi_errno_classification` | `EACCES`, `ENOENT` and an unclassified errno at `TEST_ONLY` and at the real commit each close readiness, and a following modeset and a following composed frame are refused admission; `EINVAL` at `TEST_ONLY` keeps `Ready` and a following modeset proceeds | **E35** leave the device `Ready` on `EACCES` (design mutation 27) |
| `c0_3bi_latch_is_keyed_by_generation_and_request` | real-commit `EINVAL`: the identical request answers `Latched` with no send; a different request proceeds; after an installed-generation change the identical one is sent again | **E36** latch the device; **E37** never clear the latch |
| `c0_3bi_rejection_answers_failed_not_success` | a rejected commit: `finish_crtc_config` returns an error, never `Ok(true)` | **E38** answer `Success` for a rejection (design mutation 1) |
| `c0_3bi_host_call_timeout_poisons` | the live commit never replies: after the 2 s watchdog the device is `Poisoned`, the request fails `CompletionUnknown`, nothing is published | **E39** wait forever |

## Task 9 — the production caller, the value-dead reads, the differential and the hardware test

**Deliver:** the Owner fork of `Backend::begin_crtc_config`
(`render/backend.rs:24417`) and `apply_crtc_config` (`:24755`): for an output
of an Owner device, after the existing validation that precedes discovery and
the idempotency check (which keeps Legacy's exact behaviour: `Ok(false)`,
registry sync, no dispatch), the request becomes a client modeset and
`begin` returns `CrtcConfigApply::Pending(token)`; `drain_ready_crtc_configs`
announces the token when the modeset reaches a terminal result;
`finish_crtc_config` answers `Ok(true)` after promotion or the error with its
typed cause. `cancel_crtc_config` on a modeset not yet dispatched cancels it
as never-submitted; on a dispatched one it only drops the token (the
modeset completes and installs; publication without a requester is 3b-ii's).
The Legacy path is byte-for-byte unchanged. The four value-dead
`kms_outputs_active` reads carried from 3a-ii (`teardown_direct_before_topology_requery`'s
`relight`, `crtc_config_topology_signature`,
`enqueue_prepared_crtc_config_probe`'s `was_active`, `apply_crtc_config`) are
routed to the device's installed power (`owner_dpms_installed_active`) where
the Owner path now proceeds past them, each with its mutation. The
`modeset` field of `TestWriterCoverageEvidence` is **not** flipped here (3b-i-2
completes the writer set).

The hardware test `c0_hw_3b_modeset_owner_on_card1_drm` (`#[ignore]`, run only
by the coordinator from a tty with the user's approval): on card1 with
`nvidia-drm vblank=1`, HDMI-2 mode change to another advertised mode and back
×4; HDMI-2 disable and enable ×4; DPMS-off, a mode change, DPMS-on ×4 (the
design §3.4 named risk); each step asserts the promoted state, a composed
frame completed after each lit step, and every displaced pool discharged.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_begin_on_owner_is_pending` | `begin_crtc_config` on an Owner device returns `Pending`; the stub accepts; the token is announced once; `finish` answers `Ok(true)` | **E40** apply synchronously on Owner |
| `c0_3bi_idempotent_request_dispatches_nothing` | the same request twice: the second answers `Ok(false)` with no validation or live send and the registry synced as Legacy | **E41** dispatch an idempotent request |
| `c0_3bi_cancel_before_and_after_dispatch` | cancel before dispatch: no send, prepared set released; cancel after dispatch: the commit completes and the backend state is promoted | **E42** cancel a dispatched modeset as never-submitted |
| `c0_3bi_value_dead_reads_routed` | for each of the four reads, an Owner fixture with `kms_outputs_active` forced to the wrong value: the Owner path behaves as the installed power says | **E43** restore each read in turn (design mutation 10; one mutation per read) |
| `c0_3bi_modeset_differential_backend_state_vulkan` | one script — enable, mode change, refresh-only change, idempotent repeat, disable, re-enable — through `begin`/`drain`/`finish` on a Legacy and an Owner live fixture: each result's status and the resulting RANDR registry/`platform.outputs` state are equal, except the named exceptions (none of which this script reaches) | **E44** skip the registry update on Owner |
| `c0_3bi_legacy_path_unchanged` | a Legacy fixture runs the same script with the Owner code present: every existing Legacy RANDR characterization test stays green, and a named one asserts the all-off/relight sequence still runs on Legacy | **E45** route a Legacy device through the Owner fork |
