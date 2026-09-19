# Stage 2c-iii, plan Ci — owner entry, composed producer, damage transaction

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time. Tick steps (`- [ ]` → `- [x]`) only with the evidence each one names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox. Stop with the tree dirty after each task; the coordinating session verifies and commits. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, stop and report it (F8).
>
> **Your sandbox has no `/dev/dri` and no Vulkan.** From Task 4 on, most named tests are `#[ignore = "needs live Vulkan ICD"]` `_vulkan` tests. Write them, make them compile and pass clippy, and run the deterministic ones. You **cannot** run the `_vulkan` ones: they report an environmental skip. Say so in your report; do not claim them. The coordinator runs them outside the sandbox, with the user's go-ahead.

**Revision 3 (2026-09-19)** — incorporates codex round 2 (`../findings/2026-09-19-stage-2c-iii-plan-ci-review-round2.md`: 1 blocking, 1 major, 1 minor, all verified and accepted; round 1: four APPLIED, B-1 TRADED and closed here).
- **B-1:** decision 10 gains the post-`begin` pre-IPC refusal (Submitted → Desired) and the rule that only owner outcomes and resource readiness move a buffer; R33–R34.
- **M-1:** gates 1 and 2 are coupled by the owner (`HardwareComplete` precedes `CompletionRetired`; the discharge runs in its handler), so the scene test covers the reachable orders and the outstanding-obligation guard is proven at the service level. The spec's §8.2 row is corrected to match.
- **m-1:** the normative rule is `C0-R8`; `R1`–`R34` are this plan's mutations.
- Also (author): the live Vulkan fixture has unmanaged pools; Task 4 now builds the owner-route fixture, and no test may build its state by hand.

**Revision 2 (2026-09-19)** — incorporates codex round 1 (`../findings/2026-09-19-stage-2c-iii-plan-ci-review-round1.md`: 1 blocking, 4 major, all verified against the tree and accepted).
- **B-1:** decision 7 and Task 7 contradicted each other for a generation displaced before admission, and the owner route had no buffer-phase mapping once the legacy flip is gone. Decision 10 is now the one authoritative owner buffer lifecycle. A never-admitted buffer returns to `Free` behind its GPU work alone, with no KMS gate and no fabricated event.
- **M-1:** per-member old state is now tested on rejection, `ResourcesStillCurrent` and direct dispatch too (R27–R28).
- **M-2:** `Owner` eligibility is tested for unmanaged pools and for mixed-output devices (R29–R30).
- **M-3:** the apply mutation is `Presented`, as spec §8.2 names it (R15), and every reachable invalidation source is tested (R31).
- **M-4:** transaction members carry `OutputKey` and generation, not an index; a topology change invalidates before reindexing (R32).

**Goal:** Convert the shared managed composed route to the owner. That means the owner entry that lets a dispatch register its old-state dependencies, the conductor registering them per member, a real composed `CommitDescription`, the prepare/submit fork in the scene, the real composed admission source, the damage transaction driven by owner milestones, buffer reuse behind its three gates, and tier-5 bundles with real members.

**Architecture:** The owner entry is in `crates/yserver/src/kms/owner/device.rs`. The conductor's dispatch is in `crates/yserver/src/kms/render/admission.rs`, and the consumer's old-state handling in `crates/yserver/src/kms/render/resources/commit.rs`. The composed producer is in `crates/yserver/src/kms/render/scene.rs`; its submit half forks on the output's device transport state. The damage transaction lives with the scene's per-output state and is reached through `route_owner_event_batch` (plan B2), the only path owner events take. Production is unchanged (C.0 rule C0-R8): no conductor is installed there, so every production device is `Legacy` and takes today's flip.

**Spec:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md` at revision 4 plus its §4.6. The sections this plan implements: §2.3 (the fork), §3.1 (the real source, composed half), §3.2 (registration; the first owner-API gap), §3.3 (first bullet: composed commits are non-Present), §3.4 (event routing), §4.0–§4.6 (all of Ci), and the §8.2 rows marked for sections 3.2, 3.3 (composed), 4.x and DMG-1..5 (composed half). Read the implemented 2c-ii code (the conductor in `admission.rs`, the decider in `kms/owner/admission/`) and the acceptance findings `2026-09-18-stage-2c-ii-plan-a2-accepted.md` and `2026-09-19-stage-2c-ii-plan-b2-accepted.md` before Task 1.

## Design decisions this plan fixes

Items 1–4 come from the spec (§4.6, user-approved on 2026-09-19); items 5–12 are this plan's.

1. **Real-tick evidence under Vulkan.** Every criterion involving the composed producer, the damage transaction or buffer reuse is proven with the real scene tick in a `_vulkan` fixture. Tasks 1–3 have no tick and stay deterministic.
2. **Only the shared managed route is converted.** A device with an output on the copied route, or on an unmanaged scanout pool, cannot enter `Owner` (Task 4). The copied route is Ciii's.
3. **No producer fence crosses the ioctl.** The owner route carries no `IN_FENCE_FD`. A composed generation is `Ready` only after its render completion has signalled, as the platform's existing render-completion drain (`register_scanout_render_completion`, `drain_scanout_render_completions` in `platform.rs`) observes it.
4. **Old state per member.** A commit's old state is exactly the current resources of the members it covers; every other member's current state stays current, through dispatch, retirement, rejection and unknown.
5. **Composed commits are non-Present primaries** (spec §3.3). Their description has `page_flip_event = false` and no `present_consumers`, and they need only `Accepted` and `HardwareComplete`. Composited Presents keep completing from their GPU batch (`engine.rs`, `PendingPresentBatch`). Nothing in this plan touches that path.
6. **What "the pool slot" means.** The scene has two pools. The **scanout buffer** (a `ScanoutBo` in the output's pool) is what spec §4.2's three gates govern: `CompletionRetired`, the `KmsRelease` discharge, and the GPU batch of its compose. The **descriptor-pool slot** (`pool_slots`, `pool_ring`) keeps its existing compose-fence gate, released after `HardwareComplete` instead of the page event. It was never a KMS resource.
7. **Displacement without a queue.** Under `Owner` the tick may compose a newer generation for an output whose previous generation is still rendering or desired (not admitted) when a free buffer exists. That is latest-wins, as spec §4.1 and 2c-ii §3 require. The displaced buffer follows decision 10's never-admitted path. The tick never composes into a buffer that is not `Free`.
8. **One production description builder**, in a new file `crates/yserver/src/kms/render/composed_commit.rs`. It builds the minimal persistent list from the `Output`s' plane, CRTC and property ids. `ACTIVE`'s property id is discovered once per CRTC and cached; `Output` does not carry it today.
10. **The owner buffer lifecycle — the one authoritative table** (round-1 B-1). Under `Owner`, a composed scanout buffer is in exactly one of these logical states. How they map onto `BoPhase` (`vk/scanout.rs:114`) is yours: reuse phases whose meaning fits, add phases where none does, and say which. Legacy `Submitted`/`Pending` carry the `IN_FENCE_FD`/`OUT_FENCE_FD` of the legacy flip, and neither fence exists on this route. The `Legacy` machine is untouched.

    | State | Entered by | Left by | Fences and gates |
    | --- | --- | --- | --- |
    | Free | start; the exits below | the tick records into it → Rendering | none |
    | Rendering | the tick's GPU submission; its render completion is registered with the drain | drain → Desired; displaced → Displaced | the render-completion fd belongs to the drain, which closes it |
    | Desired | the render completion drained; the generation is offered | dispatch (inside Task 2's closure) → Submitted; displaced → Displaced. A `begin` refusal or a ledger-closure error never gets it out of Desired | none |
    | Displaced | a newer generation for the output while Rendering or Desired | its compose's GPU batch has retired → Free | **no KMS gate, no `CommitId`, no fabricated event**: it was never in a commit |
    | Submitted | the ledger closure moved it into the commit's new state | `Accepted` → Accepted; a **pre-IPC `send_on` refusal** (`NeverDispatched`, after `begin`) → **Desired**, restored from the refusal's returned ledger exactly once, the generation still desired in the decider (2c-ii §6–§7); `FailedBeforeSubmit(IoctlRejected)` (post-IPC kernel rejection) → Displaced, the generation is withdrawn and the output owes a repaint; `CompletionUnknown` → Quarantined | the owner holds the out-fence as canonical evidence; the buffer holds no fence |
    | Accepted | `Accepted` | `CompletionRetired` of its commit → Current; `CompletionUnknown` → Quarantined | — |
    | Current | its commit's `CompletionRetired` | the displacing commit's `CompletionRetired` → Releasing | — |
    | Releasing | the displacing commit's `CompletionRetired` | the resource service reports its allocation ready (its `KmsRelease` discharged **and** its compose's GPU batch retired) → Free | spec §4.2's three gates |
    | Quarantined | `CompletionUnknown` | not in Ci: recovery is stage 3; the buffer is never reused | — |

    Rows 5–8 are the KMS path. The three gates govern only Releasing. A buffer that never reached Submitted is freed by its GPU work alone.

    **Only owner outcomes and resource-service readiness move a buffer** (round-2 B-1). The other invalidation sources spec §4.2 names — incarnation poison, recovery, topology, VT release, device loss — invalidate *damage* (Task 6), never buffer state. A buffer in Submitted, Accepted, Current or Releasing when one of them occurs stays there and is never reused until stage 3's teardown proves it (the fd-family barrier, `KmsDisposition::Superseded`). The legacy any-state reset (`transition_to_free_after_modeset_reset`, `vk/scanout.rs:445`) is never applied to an owner buffer; on an `Owner` device it would be a legacy writer anyway.

    **Gates 1 and 2 are coupled by the owner** (round-2 M-1). `Completed` requires `HardwareComplete` (`owner/record.rs`, "cannot complete without HardwareComplete"), so `HardwareComplete` always precedes `CompletionRetired`. The consumer records it (`hardware_completed_commits`) and discharges the `KmsRelease` obligation inside the `CompletionRetired` handler (`resources/commit.rs:227`–`262`). "Retired with the obligation outstanding" is therefore unreachable on the production order. Task 7 proves the reachable separations, and the outstanding-obligation guard is proven where it lives, in the resource service (see the exit table).
11. **Transaction members carry identity, not position** (round-1 M-4). Each transaction member is `(OutputKey, CRTC id, buffer index, generation)`, the way `register_scanout_render_completion` already keys by `OutputKey`. A milestone is applied to a member only if that output still exists with the same generation. A topology change invalidates every open transaction **before** any output is reindexed.
12. **Test names start with `c0_conv_ci_`**, so one filter selects the whole plan. Vulkan tests end in `_vulkan` and carry `#[ignore = "needs live Vulkan ICD"]`.

## Limits stated

- Fixture level only; no production caller (C0-R8). The hardware run of spec §6.4 is Ciii's.
- Direct, unflip, maintenance producers: unchanged (Cii, Ciii, stage 4). The 2c-ii test source keeps answering for them.
- The copied route and unmanaged pools: refused for `Owner` (decision 2).
- One device (multi-device is Ciii).

## Global Constraints

- **Production is byte-for-byte unchanged:** on a device without an active conductor, the tick takes today's `submit_flip_with_fences` path with the same arguments, the same staging and the same page-event ack. Task 4's test pins this.
- Every token is consumed exactly once; confirmation is at the send; no retry on refusal (2c-ii §6).
- Resources travel by value; nothing is bare-dropped. A scanout buffer is in exactly one of: free, rendering, desired, submitted, current, releasing.
- No side effect inside `debug_assert!`; fail closed, never panic, in non-test code; no invented state; no test-only hook that bypasses the path it is named after.
- Owner events reach the scene only through `route_owner_event_batch`; tests deliver milestones through it (a stub helper behaviour or crafted events handed to it — say which), never by calling a scene handler directly.
- **Honesty rule (F8).** An unreachable scenario, a seam that does not behave as stated, a fixture that cannot carry what a test needs, or a real design choice the plan left open: stop and report. A suggested test shape that turns out unreachable must be reported, never silently replaced.

## Exit criteria

| Criterion (spec) | Tests | Mutation that must fail them |
| --- | --- | --- |
| A failed ledger closure leaves the owner as before `begin` (§4.0) | `c0_conv_ci_owner_failed_ledger_leaves_nothing` | R1: keep the slot reserved on a closure error |
| Obligations carry the record's own `CommitId` (§4.0) | `c0_conv_ci_owner_ledger_sees_the_record_commit` | R2: pass the closure a different `CommitId` than the record's |
| `begin_with_ledger` keeps its Present refusal (§4.0) | `c0_conv_ci_owner_fallible_entry_refuses_present` | R3: drop the `page_flip_event`/`present_consumers` refusal from the new entry |
| Dispatch registers old-state dependencies (§3.2) | `c0_conv_ci_dispatch_registers_displaced_obligations` | R4: build the ledger with `Submitted::new` instead of registering |
| A registration failure consumes no admission state (§4.0; 2c-ii §6) | `c0_conv_ci_registration_failure_aborts_the_token` | R5: `confirm` instead of `abort` after a ledger error |
| Old state is per member (§4.6) | `c0_conv_ci_other_crtc_current_survives_retirement`, `c0_conv_ci_other_crtc_current_survives_rejection`, `c0_conv_ci_other_crtc_current_survives_direct_dispatch` | R6: take the whole current state as old again at dispatch; R27: replace the whole current vector in the `ResourcesStillCurrent`/rejection arm; R28: keep the whole-state `take_current` in the direct dispatch |
| The description is the minimal list, non-Present (§3.3, decision 8) | `c0_conv_ci_description_single_output`, `c0_conv_ci_description_bundle` | R7: set `page_flip_event` on a composed description |
| `Owner` is refused for copied or unmanaged outputs, on any output of the device (decision 2) | `c0_conv_ci_owner_refused_for_copied_route`, `c0_conv_ci_owner_refused_for_unmanaged_pool`, `c0_conv_ci_owner_refused_for_one_bad_output_of_many` | R8: allow `Owner` with a copied output; R29: allow it with an unmanaged pool; R30: check only the first (or initiating) output |
| `Legacy` is unchanged; `Owner` never flips from the tick (§2.3, §6.3) | `c0_conv_ci_legacy_tick_flips_as_before_vulkan`, `c0_conv_ci_owner_tick_offers_instead_of_flipping_vulkan` | R9: force the legacy branch under `Owner`; R10: offer under `Legacy` |
| No producer fence crosses the ioctl; readiness waits for render completion (decision 3) | `c0_conv_ci_ready_only_after_render_completion_vulkan` | R11: report `Ready` before the render completion drains |
| A displaced generation stages and acks nothing (§4.1) | `c0_conv_ci_displaced_generation_acks_nothing_vulkan` | R12: ack the displaced generation's snapshots |
| The transaction is installed before any milestone is routed (§4.2) | `c0_conv_ci_transaction_installed_inside_the_closure_vulkan` | R13: install it at `confirm` |
| Staging at `Accepted`, not at dispatch (DMG-1) | `c0_conv_ci_stage_at_accepted_vulkan` | R14: stage at the offer/dispatch |
| Apply at `HardwareComplete` only, never again at `Presented` (DMG-2) | `c0_conv_ci_apply_at_hardware_complete_vulkan` | R15: also apply at `Presented` (spec §8.2's mutation) |
| `Dispatched` retains; `FailedBeforeSubmit` and `NeverDispatched` close without staging (§4.2) | `c0_conv_ci_dispatched_retains_the_transaction_vulkan`, `c0_conv_ci_rejection_closes_without_staging_vulkan` | R16: close the transaction at `Dispatched` |
| Unknown and every invalidation source invalidate, and the output owes a repaint (DMG-3) | `c0_conv_ci_unknown_invalidates_vulkan`, `c0_conv_ci_invalidation_sources_invalidate_vulkan` | R17: restore on `CompletionUnknown`; R31: drop the invalidation for one source (each reachable source mutated separately) |
| Members are keyed by output identity; a stale milestone after a topology change touches nothing (decision 11) | `c0_conv_ci_stale_milestone_after_topology_change_vulkan` | R32: key members by output index |
| Damage after capture survives the ack (stage 2c §5) | `c0_conv_ci_new_paint_survives_the_ack_vulkan` | R18: ack from the live store instead of the captured snapshots |
| Buffer reuse waits for its gates, on the reachable orders (§4.2; decision 10; round-2 M-1) | `c0_conv_ci_buffer_reuse_waits_for_every_gate_vulkan` | R19: free the buffer at `HardwareComplete`, with `CompletionRetired` withheld; R21: free it at `CompletionRetired`, before the GPU batch retires |
| The outstanding-`KmsRelease` gate (§4.2 gate 2) | the resource service's readiness guard for an outstanding `KmsRelease` (`resources/mod.rs`, around the `pending_obligations` check), already proven by the 2c-i debt census; Task 7 names the census site it relies on and re-runs its test | R20: the census mutation of that guard — reported as proven at the service level, not claimed for the scene |
| A pre-IPC refusal after `begin` returns the buffer to Desired (decision 10; round-2 B-1) | `c0_conv_ci_pre_ipc_refusal_returns_the_buffer_to_desired_vulkan` | R33: withdraw the generation (Displaced) on `NeverDispatched` |
| Non-owner invalidation sources never move a buffer (decision 10; round-2 B-1) | `c0_conv_ci_invalidation_sources_invalidate_vulkan` (extended) | R34: free an owner buffer on an invalidation source |
| A bundle is one transaction (DMG-4) | `c0_conv_ci_bundle_is_one_transaction_vulkan` | R22: stage one bundle output at a separate milestone |
| Composited Presents complete once, from the GPU batch (§3.3) | `c0_conv_ci_composited_present_completes_once_vulkan` | R23: also complete it at the composed commit's `HardwareComplete` |
| Scene contracts preserved (§4.4) | `c0_conv_ci_owed_repaint_wakes_without_new_paint_vulkan`, `c0_conv_ci_off_output_damage_not_acked_vulkan`, `c0_conv_ci_skipped_output_stays_armed_vulkan`, `c0_conv_ci_two_outputs_permuted_completions_vulkan` | R24: drop `owes_repaint` from the owner-route wake predicate; R25: ack an `OffOutput` snapshot; R26: drop a skipped output's retained pieces from `dormancy_inputs` |

---

### Task 1: The fallible, CommitId-aware owner entry

**Files:** `crates/yserver/src/kms/owner/device.rs`; its tests.

**Interfaces.** Produces a public owner entry, named by you (say which), that is `begin_with_ledger` with a fallible closure: `FnOnce(CommitId) -> Result<Submitted<R>, E>`. On `Err(e)` it returns an error carrying `e`. The shape of that error type is yours; it must hand `e` back by value, because `e` holds the resources. `begin_with_ledger` itself may become a thin wrapper over it; its behaviour must not change.

**Invariants (spec §4.0).**
- The closure runs after every refusal point the current entry has (transport, identity, build, completion context, slot reservation), and receives the `CommitId` the record will carry.
- On a closure error the owner is exactly as before the call: no live record, the slot is idle (`slot.release(commit)`, `slot.rs:120`, or equivalent), no event emitted. A consumed `CommitId`/`EventToken` is allowed, since identities are monotonic and gaps are legal; say whether it happens.
- The new entry refuses a description with `page_flip_event` or `present_consumers`, exactly as `begin_with_ledger` does (`device.rs:1480`). The Present-carrying entry is Cii's.

**Named tests:**
- `c0_conv_ci_owner_failed_ledger_leaves_nothing` — a closure returning `Err`. Afterwards the slot is idle, `live` is `None`, and a following `begin` with a good ledger succeeds on the same owner.
- `c0_conv_ci_owner_ledger_sees_the_record_commit` — the `CommitId` the closure received equals the record's and the returned one.
- `c0_conv_ci_owner_fallible_entry_refuses_present` — both flags refused, and the closure is not called.

- [ ] Steps: tests; red; implement; gate; stop dirty and report.

---

### Task 2: The conductor registers old-state dependencies, per member

**Files:** `crates/yserver/src/kms/render/admission.rs` (`admission_dispatch_primary` and `admission_dispatch_direct`, both of which build the ledger today with `Submitted::new`), `crates/yserver/src/kms/render/resources/commit.rs` (`take_current`, the `CompletionRetired`, `ResourcesStillCurrent` and rejection arms of `consume`); tests.

**Invariants.**
- Every conductor dispatch builds its ledger through Task 1's entry, calling `register_commit_dependencies(commit, old, new, service)` (`commit.rs:572`) inside the closure, with `service` taken from `KmsBackend::resource_service`. This covers both the primary path and the direct path. There is no other ledger construction in the conductor.
- A registration error aborts the token (`admission_abort`), returns the resources to where they came from (the old state back to current, the new state back to its intent's owner), and records the refusal. No ticket is consumed, no turn advances, no loser ages, and there is no retry until the next real wake.
- **Old state per member (spec §4.6).** The old state of a commit is exactly the current resources whose members intersect the commit's members. At `CompletionRetired` only those move to releasing, and the new state *replaces* them: the current resources of every other member stay current. The same holds when a rejection or `ResourcesStillCurrent` returns the old state. Each `CommitResources` has a `crtcs: Vec<GroupMember>`. A current entry that spans both covered and uncovered members is a shape this plan does not expect; if you meet one, F8.

**Named tests** (deterministic; extend the existing `admission_backend_with_stub_executor` fixture or add a two-CRTC one; say which):
- `c0_conv_ci_dispatch_registers_displaced_obligations` — a composed admission over a current allocation of the same CRTC: after dispatch the displaced allocation carries a `KmsRelease` obligation keyed by the commit. After a `HardwareComplete` + `CompletionRetired` routed through `route_owner_event_batch` it is discharged and the allocation moves to releasing.
- `c0_conv_ci_registration_failure_aborts_the_token` — force a registration error (for example, an old allocation already frozen or detached in the service; say which you used): the owner is idle, the decider is exactly as before `lock` (same pending tickets, same turn), the resources are back, and nothing is dispatched until another wake.
- `c0_conv_ci_other_crtc_current_survives_retirement` — two CRTCs, each with a current allocation; a commit for CRTC A retires; B's allocation is still current and has no obligation, and A's is releasing.
- `c0_conv_ci_other_crtc_current_survives_rejection` — the same set-up, with the commit for A rejected by the kernel (a stub `RejectWith`, routed through `route_owner_event_batch`). A's old state is current again, and B's never left current and has no obligation.
- `c0_conv_ci_other_crtc_current_survives_direct_dispatch` — a direct dispatch through `admission_dispatch_direct` covering a subset of the device's CRTCs, over a current state that includes another member. That member stays current and obligation-free through dispatch and retirement. If the 2c-ii direct fixture can only cover every CRTC, report which shape you used; F8 if no subset is reachable.

- [ ] Steps: tests; red; implement; gate; stop dirty and report.

---

### Task 3: The production composed description builder

**Files:** new `crates/yserver/src/kms/render/composed_commit.rs` (register it in `render/mod.rs`); `crates/yserver/src/platform/drm.rs` or the platform's device entry for the cached `ACTIVE` id (your choice; say which); tests.

**Interfaces.** Produces `fn composed_description(members: &[ComposedPlane<'_>], property_ids: PropertyIds) -> CommitDescription`, or an equivalent you name. A `ComposedPlane` is one output's plane, its CRTC, and the framebuffer to show. Also produces the per-CRTC `ACTIVE` property-id discovery, with a cache.

**Invariants.**
- The minimal persistent list, as the legacy composed flip writes it (`drm/page_flip.rs:152`): for each member, the plane's `FB_ID` and `CRTC_ID`, and the CRTC with `ACTIVE = 1`. `crtc_state` is active → active for each member. No `IN_FENCE_FD` (decision 3), no `OUT_FENCE_PTR` (the owner appends it), `page_flip_event = false`, and `present_consumers` empty (decision 5).
- The result passes `AtomicCrtcClosure::compute`, and `expected_completion()` is exactly the members' CRTCs. That is what makes a bundle's `ExpectedCompletionCrtcs` name its included outputs (DMG-4).
- `PropertyIds` comes from the device: `crtc_id` from the plane (`Output::plane_crtc_id_prop`), `active` from the cached discovery, and `out_fence_ptr` from `Output::crtc_out_fence_ptr_prop` (discovered if `None`, as `page_flip.rs:187` does). A discovery failure is an error the caller treats as "not ready", never a panic.

**Named tests:** `c0_conv_ci_description_single_output`, `c0_conv_ci_description_bundle` (three members). Both build from `Output` values like the fixture's and check the closure and the flags.

- [ ] Steps: tests; red; implement; gate; stop dirty and report.

---

### Task 4: The prepare/submit fork in the scene, and `Owner` eligibility

**Files:** `crates/yserver/src/kms/render/scene.rs` (the composed present path from the flip site near line 4683 through `submit_shared_scanout_frame`, and the managed variant near line 8013), `crates/yserver/src/kms/render/admission.rs` (`admission_offer_composed`, the wake after a render completion), `crates/yserver/src/kms/render/platform.rs` (the render-completion drain), the `Owner` establishment path (the transport gate install); tests.

**Invariants.** Task 4 introduces decision 10's states Free → Rendering → Desired and Displaced, and decision 11's member identity for the prepared generation.
- **The fork.** The shared managed present path splits at the flip. Everything before it is one code path for both routes: repaint, render, `PendingAck` construction and the managed GPU batch. Then:
  - with no active conductor for the output's device, today's `submit_flip_with_fences`, staging and `pending_acks` push, unchanged;
  - with one: no flip and no staging. The buffer is registered with the render-completion drain (decision 3), and the frame becomes the output's **prepared composed generation**: buffer index, generation, captured `PendingAck`, managed batch. When its render completion drains, it is offered (`admission_offer_composed`) and the conductor is woken.
- **Displacement (decision 7).** A newer prepared generation for the same output displaces an unadmitted one. The displaced one stages nothing and acks nothing, and its snapshots stay pending in the store. Its buffer returns to the pool once its render work has completed.
- **Eligibility (decision 2).** Installing the `Owner` transport for a device is refused while any of its outputs uses the copied route or an unmanaged pool. The refusal is visible to the caller, and the device stays `Legacy`.

**The fixture (a deliverable of this task).** `KmsBackend::for_tests_with_vk_live_scene` allocates **unmanaged** pools, which decision 2 refuses for `Owner`. Build an owner-route live fixture: the live Vulkan scene with its pools adopted into the resource service as managed allocations, plus the stub executor, owner, resource service, `Owner` gate and conductor, installed the way the `c0_adm_conductor_` fixtures install them. `c0_2ci_scene_handle_page_flip_complete_registers_managed_batch_vulkan` (`scene.rs:8890`) shows pool adoption only; it builds its `PendingAck` by hand and is **not** real-tick evidence. No test of this plan may build a `PendingAck`, a `BoPhase` or a prepared generation by hand. If adoption cannot be done through production entries, F8.

**Named tests** (`_vulkan`, on that fixture):
- `c0_conv_ci_legacy_tick_flips_as_before_vulkan` — without a conductor, a damaged tick issues the flip and stages as today. Use the existing assertions on `pending_acks` and the damage state; name the observable you use for the flip.
- `c0_conv_ci_owner_tick_offers_instead_of_flipping_vulkan` — with an active conductor, the same tick issues no legacy flip and stages nothing, and after the render completion drains the conductor holds a desired composed generation for that CRTC.
- `c0_conv_ci_ready_only_after_render_completion_vulkan` — before the drain, the snapshot reports that generation `Waiting`.
- `c0_conv_ci_displaced_generation_acks_nothing_vulkan` — two ticks before any admission: the first generation is displaced. After the second is admitted and completes, the first's snapshots were acked only because the second frame's capture included them, and its buffer is free.
- `c0_conv_ci_owner_refused_for_copied_route`, `c0_conv_ci_owner_refused_for_unmanaged_pool`, `c0_conv_ci_owner_refused_for_one_bad_output_of_many` (deterministic if reachable without Vulkan; otherwise `_vulkan`) — `Owner` refused and the device still `Legacy`. The last one has several outputs on one device, with only a non-first output bad.
- The tests drive the production drain entry (`drain_scanout_render_completions` and its consumer), never `admission_offer_composed` directly. If the live fixture cannot deliver a render completion to that drain, F8.

- [ ] Steps: tests; red; implement; gate; stop dirty and report.

---

### Task 5: The real composed source

**Files:** `crates/yserver/src/kms/render/admission.rs`, `scene.rs`; tests.

**Invariants (spec §3.1).**
- For composed intents, the conductor's inputs come from backend and scene state, not from the injected `AdmissionSource`:
  - `producer_readiness`: Task 4's prepared generation, `Ready` once drained;
  - `describe`: Task 3's builder over the admitted generation's buffer framebuffer(s);
  - `composed_resources`: a `CommitResources` holding the buffer's managed allocation lease and the output's `GroupMember`, moved out of the prepared generation;
  - `homogeneous_group`: the CRTCs of the device's outputs that share a refresh rate, which is what `direct_scanout_topology_eligible` checks today.

  How you split the trait is yours (a composed half served by the backend, or a narrower trait); the direct, maintenance and recovery answers stay with the injected source until Cii and stage 4. No exit criterion of this plan may rest on the injected source's composed answers.
- The prepared generation's resources move into the ledger exactly once: into `composed_resources` at dispatch, back on refusal or rejection, never cloned.

**Named test:** `c0_conv_ci_owner_tick_offers_instead_of_flipping_vulkan`, extended through admission: the dispatched description equals Task 3's builder output for that buffer, and the ledger's new state holds that buffer's allocation.

- [ ] Steps: tests; red; implement; gate; stop dirty and report.

---

### Task 6: The damage transaction

**Files:** `scene.rs` (the per-output state, a transaction store keyed by `CommitId`, and the handlers reached from `route_owner_event_batch`), `admission.rs` (installing inside Task 2's closure), `backend.rs` (`route_owner_event`'s arms); tests.

**Invariants (spec §4.2, DMG-1..3).**
- The transaction is installed **inside** Task 2's ledger closure, keyed by the `CommitId` it receives, before any IPC. The events `begin` and the send return are routed after it exists. A closure error installs nothing.
- It carries, for each included output, a member keyed as decision 11 says, with the captured `PendingAck` contents (drawable snapshots, submitted structure and failed-repaint damage, participants, cursor transition fields).
- The milestone table:
  - `Dispatched` retains the transaction.
  - `FailedBeforeSubmit` (any cause, including `NeverDispatched`) closes it without staging.
  - `Accepted` stages each output once (`commit_submitted`, gated on the compose having been complete, as `stage_submitted_frame` does).
  - `HardwareComplete` applies (`retire_success`) and does everything `handle_page_flip_complete` does after its ack match (`scene.rs:2129`): ack the captured snapshots, subtract the captured structure and failed-repaint damage, push the damage history, set `prev_presented`, apply the cursor-transition fields.
  - `Presented` does nothing.
  - `CompletionUnknown`, and every invalidation source the spec names (incarnation poison, recovery, topology, VT release, device loss), invalidate, and the output owes a repaint (`owes_repaint`). For each source, find the event or hook that signals it on an `Owner` device today. A source with no signal yet is reported (F8, naming it); it is never skipped silently.
- **The restore row.** Find whether any `TerminalState` today is a failure after `Accepted` with the prior state proven current. If one is, restore on it. If none is, report it (F8) and do not invent one; the coordinator records the spec's F8 stop.

**Named tests** (`_vulkan`; milestones delivered through `route_owner_event_batch`): `c0_conv_ci_transaction_installed_inside_the_closure_vulkan` (a `Dispatched` returned by the send is routed and finds the transaction), `c0_conv_ci_stage_at_accepted_vulkan`, `c0_conv_ci_apply_at_hardware_complete_vulkan` (a `Presented` after it changes no damage, history or cursor state), `c0_conv_ci_dispatched_retains_the_transaction_vulkan`, `c0_conv_ci_rejection_closes_without_staging_vulkan` (a stub `RejectWith` behaviour), `c0_conv_ci_unknown_invalidates_vulkan`, `c0_conv_ci_invalidation_sources_invalidate_vulkan` (one case per reachable source, each asserting the invalidation and `owes_repaint`), `c0_conv_ci_stale_milestone_after_topology_change_vulkan` (a topology change between dispatch and `HardwareComplete`: no output is acked, and the transaction is invalidated first), `c0_conv_ci_new_paint_survives_the_ack_vulkan` (paint lands between the capture and `HardwareComplete`; it survives and drives the next tick).

- [ ] Steps: tests; red; implement; gate; stop dirty and report.

---

### Task 7: Buffer reuse behind its three gates, and the descriptor slot

**Files:** `scene.rs`, `platform.rs` (the buffer phase machine, `on_page_flip_complete`'s owner-route counterpart), `resources/`; tests.

**Invariants (spec §4.2, decisions 6 and 10).**
- Task 7 implements decision 10's KMS rows: Submitted → Accepted → Current → Releasing → Free, the rejection and unknown exits, and the Displaced → Free path. A Releasing buffer becomes Free only when all three of these hold:
  1. `CompletionRetired` has moved it to releasing;
  2. its `KmsRelease` obligation is discharged;
  3. its compose's GPU batch has retired.
  Gates 2 and 3 are the resource service reporting its allocation ready. The page event plays no part.
- A Displaced buffer becomes Free when its compose's GPU batch retires, with no KMS gate and no event fabricated for it.
- The descriptor-pool slot is released at `HardwareComplete` behind its existing compose-fence gate (`pending_pool_releases` when the fence is not yet signalled).
- `Legacy` phase handling is unchanged.

**Named tests:** `c0_conv_ci_buffer_reuse_waits_for_every_gate_vulkan` — the reachable separations, in the owner's own order: (a) `HardwareComplete` routed, `CompletionRetired` withheld: not reusable; (b) `CompletionRetired` routed (the obligation discharged there), the compose's GPU batch not yet retired: not reusable; then the batch retires: reusable. Name the resource-service census site and test that prove the outstanding-obligation guard, and re-run that test. `c0_conv_ci_pre_ipc_refusal_returns_the_buffer_to_desired_vulkan` — a post-`begin` pre-IPC refusal (a reaped executor, as 2c-ii's refusal tests use): the buffer is Desired again, holding the allocation returned by the refusal exactly once, and a later wake dispatches the same generation. `c0_conv_ci_invalidation_sources_invalidate_vulkan` (Task 6) also asserts, for each source, that no owner buffer changed state or became Free. `c0_conv_ci_displaced_generation_acks_nothing_vulkan` (Task 4) is extended: the displaced buffer becomes Free after its GPU work, while no `CompletionRetired` or `KmsRelease` exists for it. `c0_conv_ci_rejection_closes_without_staging_vulkan` (Task 6) is extended: the rejected buffer takes the Displaced path, and the output owes a repaint.

- [ ] Steps: tests; red; implement; gate; stop dirty and report.

---

### Task 8: Bundles, composited Presents, scene contracts

**Files:** `scene.rs`, `admission.rs`; the live-scene fixture if it needs more outputs; tests.

**Invariants.**
- **Bundles (DMG-4).** A tier-5 admission is one transaction over every included output: staged at the one `Accepted`, applied at the one `HardwareComplete`. No output is staged twice without an intervening apply or invalidate, and a ready output that was not included earns nothing.
- **Composited Presents (spec §3.3).** A Present copied and composited under `Owner` completes exactly once, from its GPU batch. The composed commit carries no Present, and none of its milestones emits a CompleteNotify or FIFO wake.
- **Scene contracts (spec §4.4).** Under `Owner`:
  - an invalidated or failed transaction owes a repaint that wakes the scene without new paint;
  - `NoPieces` and `HiddenDamage` stay distinct, and an output skipped this tick keeps its retained `last_pieces` in `dormancy_inputs`: an output with a pending generation is not evidence that its drawable is invisible;
  - `OffOutput`, `Hidden` and `OtherOutput` snapshots authorize no ack;
  - two outputs complete correctly in either order, bundled and separately scheduled.

**Named tests** (`_vulkan`): `c0_conv_ci_bundle_is_one_transaction_vulkan` needs **three** outputs on one device (2c-ii's tier-5 rule includes every ready CRTC); extend the fixture if needed, F8 only if impossible. Also `c0_conv_ci_composited_present_completes_once_vulkan`, `c0_conv_ci_owed_repaint_wakes_without_new_paint_vulkan`, `c0_conv_ci_off_output_damage_not_acked_vulkan`, `c0_conv_ci_skipped_output_stays_armed_vulkan` (an output whose generation is still desired while another output ticks: its drawable is not marked dormant) and `c0_conv_ci_two_outputs_permuted_completions_vulkan`.

- [ ] Steps: tests; red; implement; gate (Task 8 adds the three `cargo check` targets); stop dirty and report.

---

## Gate — the one authoritative list

Every task, in your sandbox:

```bash
cargo build -p yserver --bin yserver
cargo build --release -p yserver --bin yserver
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features tcp-transport -- -D warnings
cargo clippy --all-targets --features xdmcp -- -D warnings
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_conv_ci_; done
cargo test --release -p yserver --lib c0_conv_ci_
cargo test -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

Task 8 adds `cargo check --workspace --target <t>` for `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd`. Also grep the new code for `debug_assert!(self.` before reporting.

## What the coordinator does after each task

1. Reads the diff against the invariants, and checks each named test sets up its scenario and goes through the path it is named after.
2. Reruns the gate outside the sandbox, and — **after asking the user** for the GPU — runs `cargo test -p yserver --lib c0_conv_ci_ -- --ignored --test-threads=1` in debug and release.
3. After Task 8: applies R1–R34 by line, confirming each compiled and removed the behaviour, running the Vulkan ones with the user's go-ahead. A survivor goes back as a finding. Then the full hardware gate (spec §8.4) with the user's go-ahead, and a `docs/status.md` entry plus an acceptance finding.
4. Commits each task with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)` and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
