# Stage 2c-iii, plan Cp — the copied scanout route

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), run with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters (`c0_conv_cp_`, `c0_conv_cfb_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_ci_`, `c0_conv_cir_`) with `--include-ignored`, never `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time. Stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code, stop and report it (F8); never silently substitute a test shape.

**Revision 2 (2026-09-22)** — incorporates codex round 1 (`../findings/2026-09-22-stage-2c-iii-plan-cp-review-round1.md`: 0 blocking, 3 major, all three verified against the tree and accepted).
- **M-1:** revision 1's Task 2 proved the promotion predicate before any production path could produce a receipt, and allowed an interim test seam to carry its evidence — the defect plan Ciii's round 1 found as its own M-1. **Tasks 2 and 3 are swapped**: the prepared copy (with its wake registration) lands first and produces the receipt; the promotion that consumes it runs its acceptance tests through that production path. The test-seam permission is removed.
- **M-2:** the spec's §8.2 row "a failed copy submission offers nothing and discharges its leases through the service" was dropped when CP-4b was rewritten around `gpu_submitted`. Restored as its own criterion with Q36 and Q37, and Task 4 now states `Displaced` and no offer for **both** `gpu_submitted` outcomes.
- **M-3:** cancellation has two distinct production operations and revision 1 named one. `cancel_scanout_render_completions_for_output` (`platform.rs:4968`) is per output; `clear_scanout_render_completions` (`platform.rs:4987`) clears the whole queue and has three callers — VT suspend (`platform.rs:6634`), `platform.rs:7664` and `scene.rs:3128`. Decision 11 now covers both, with Q38.

**Revision 1 (2026-09-22).**

**Goal:** Convert the copied composed producer (`submit_copied_scanout`, `platform.rs:6395`) to the Owner route, so that a device with a copied-route output can enter `Owner` at all. This is the item plan Ciii's acceptance carries as still owed to 2c-iii, and the first fixture in the project that builds a copied output — which is what turns Ci's validator-only R8 into an executable exclusivity case.

**Architecture:** Eight tasks. The copy becomes the **last stage of the producer**: renderer A writes the source, the sink copy writes the destination, and only when the resource service has retired the destination's write obligation does a `ComposedOffer` reach the conductor. The copy's `sync_file` wakes the core loop; it never authorizes anything. Owner-route code lives in a new `copied_owner.rs` reached from the existing fork point in the copied completion handler; nothing is interleaved into a Legacy function body (the Cii/Ciii constraint). Production is unchanged (C0-R8).

**Spec:** `docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md` at revision 4 (`80089003`), whole. Read first: §3 (3.1 the offer boundary and CP-2a, 3.2 the ownership sequence CP-4 through CP-5, 3.3 failure), §4 (eligibility and exclusivity), §5 (retirement), §6 (the hardware run) and §8.2 (every row is an exit criterion here). Then the three review findings it incorporates (`../findings/2026-09-22-stage-2c-iii-copied-route-design-review-round{1,2,3}.md`) — each one records what the tree actually does at the seams this plan touches, and the third records the pattern that governs this plan's method. Then the parent, `2026-09-19-phase-c0-stage-2c-iii-conversion-design.md` §6.3.

## Method this plan is written under

Rounds 2 and 3 of the design review each found the author inventing a mechanism the resource service already exposed: `prepare_retirement_batch`, `abandon_unsubmitted_batch`, `has_pending_obligation`, the paired `BoPhase`. **Before writing any new service-side machinery, enumerate what `resources/mod.rs` and `resources/gpu.rs` already expose for that job and say why it does not fit.** A new helper that duplicates an existing one is a plan failure, not an implementation detail.

## Design decisions this plan fixes

Items 1–7 come from the spec; 8–11 are this plan's.

1. **The copy is the producer's last stage** (spec 3.1). The offer follows the retirement of the destination's write obligation, never the fence and never generic availability.
2. **The promotion consumes a correlated receipt** (CP-2a): the generation holds `(destination AllocationKey, ObligationId)` and promotes only when `has_pending_obligation` is false **and** `is_frozen` is false (`resources/mod.rs:290`, `:911`).
3. **The owner buffer state machine gains no state** (CP-3): `Rendering` already means the producer chain has not finished writing; the waiting stage discriminates.
4. **Preparation before GPU work** (CP-4a): A's batch serviced → destination `Write` + its GPU obligation → source `Read` + its read obligation → **then** submit → build and register the batch. A failure during preparation cancels and drops everything taken for the attempt.
5. **Failure after submission is dispositional** (CP-4b): `abandon_unsubmitted_batch` (`resources/gpu.rs:395`) by its `gpu_submitted` answer; never a ticketless batch, which would freeze no destination key.
6. **The source's exclusion is the paired BO phase** (CP-4c): selection takes the destination to `Recording` (`platform.rs:6374`) and the source is paired by `bo_idx`. No claim rests on event-loop scheduling.
7. **Eligibility requires both halves managed** (CP-7); the refusal survives as `Unmanaged`. **Exclusivity is proven at the site** (CP-8), and the transport gate's refusal is not the observation.
8. **Owner code in `copied_owner.rs`**, reached from one fork point in the copied completion handler (`scene.rs:3990`-`4002`). `submit_copied_scanout` keeps its Legacy body byte-for-byte.
9. **Test names start with `c0_conv_cp_`**; Vulkan tests end in `_vulkan` with `#[ignore = "needs live Vulkan ICD"]` on the Owner live fixture; the hardware test is `c0_hw_cp_copied_route_cross_device_drm`. Mutations are `Q1`…
10. **The sink's fence ticket** (spec §9.1): `submit_copy_with_fence` (`vk/scanout.rs:1817`) already takes a `vk::Fence`, used only by the probe. Task 2's first step is the implementer's decision on where a `FenceTicket` for the sink context comes from, written into the task report before the code.
11. **The wake source, and both cancellation operations** (spec §9.2; round-1 M-3): the copy's `sync_file` registers in the existing `CompletionPoller` or a sibling of it. **Task 2's first step states which and why**, and the choice binds two teardown paths, not one: `cancel_scanout_render_completions_for_output` (`platform.rs:4968`) is per output, while `clear_scanout_render_completions` (`platform.rs:4987`) clears the whole queue and is what VT suspend (`platform.rs:6634`), `platform.rs:7664` and `scene.rs:3128` call. A sibling container must be reached by **both**; the existing poller must be shown to be the container both already clear. CP-2 holds either way: whatever wakes, only the receipt promotes.

## Limits stated

- Fixture level plus the one hardware run of Task 8. No production caller (C0-R8).
- The managed-storage access debt (spec §7, `backend.rs:18716`) is **out of scope**. This plan must not repair it and must not depend on it.
- Stage 3/4 work — lifecycle, modeset, DPMS, VT, topology, cursor, gamma — is untouched.

## Global Constraints

- **Production is byte-for-byte unchanged**: without an active conductor nothing here runs. Each task keeps a named Legacy characterisation test green (`c0_conv_cp_legacy_copied_route_unchanged`).
- Owner milestones reach consumers only through `route_owner_event_batch`; tests deliver them that way.
- No hand-built `CoreRetirementBatch`, `GpuObligation`, `ReadObligation`, `AllocationLease`, `ComposedOffer`, `PendingAck` or `BoPhase` in any test: everything comes from the production producer. A test that cannot reach its state through production entries is an F8, not a fixture. This is sharpest for the read obligation, which **has no production caller today** (`resources/guard_tests.rs:202`, `:221`, `:244`, `resources/tests.rs:1851` are the only ones) and gets its first one here: a criterion satisfied by a hand-built batch satisfies nothing.
- Resources travel by value; nothing is bare-dropped; every lease is released exactly once; no retry on refusal.
- No side effect inside `debug_assert!`; fail closed, never panic, in non-test code; no test-only hook that bypasses the path it is named after.
- Owner-route code lives in its own functions/modules reached from one fork point; no `owner_route`-style branch inside a Legacy function body (the coordinator greps for it).
- **Honesty rule (F8).** An unreachable scenario, a seam that does not behave as stated, a fixture that cannot carry what a test needs, or a real design choice left open: stop and report.

## Checks every task must keep green

```bash
cargo build -p yserver --bin yserver
cargo build --release -p yserver --bin yserver
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features tcp-transport -- -D warnings
cargo clippy --all-targets --features xdmcp -- -D warnings
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_conv_cp_; done
cargo test -p yserver --lib c0_conv_cp_ -- --include-ignored --test-threads=1
cargo test --release -p yserver --lib c0_conv_cp_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

Before acceptance, the coordinator adds: `cargo check --workspace` for Linux glibc, Linux musl and FreeBSD (spec §8.4).

## Exit criteria

| Criterion (spec) | Named tests | Mutations that must fail them |
| --- | --- | --- |
| A copied output enters `Owner` only when both pool halves are managed (CP-7) | `c0_conv_cp_eligibility_requires_both_halves`, `c0_conv_cp_unmanaged_source_keeps_the_device_legacy` | Q1: accept a copied output whose sources are unadopted; Q2: check only the destination half |
| A copied output can be built and driven at fixture level (§8.1) | `c0_conv_cp_copied_fixture_builds_a_managed_output_vulkan` | — (its absence is the F8 of Task 1) |
| The offer follows the retirement of this generation's destination obligation (CP-1, CP-2a) | `c0_conv_cp_offer_waits_for_the_destination_obligation_vulkan`, `c0_conv_cp_readable_fence_with_pending_obligation_offers_nothing_vulkan`, `c0_conv_cp_frozen_destination_offers_nothing_vulkan` | Q3: offer at copy submission; Q4: offer at A's completion; Q5: promote on a readable fence while the obligation is pending; Q6: promote while the destination key is frozen; Q7: promote on a successful `service_completions` that retired another key |
| A copied frame completes end to end at fixture level (§8.1) | `c0_conv_cp_end_to_end_copied_frame_vulkan` | Q35: apply the damage transaction at `Accepted` instead of `HardwareComplete` |
| The owner buffer reaches `Desired` at the copy's retirement, not A's (CP-3) | `c0_conv_cp_desired_at_copy_retirement_vulkan` | Q8: promote when A completes |
| A generation displaced during the copy behaves as one displaced during the render (CP-3) | `c0_conv_cp_displaced_during_copy_offers_nothing_vulkan` | Q9: offer a generation displaced during the copy |
| Every obligation is registered before the copy reaches the GPU (CP-4a) | `c0_conv_cp_obligations_precede_the_copy_vulkan` | Q10: register the destination obligation after submission; Q11: register the source obligation after submission |
| A failed preparation leaves no obligation and no lease behind (CP-4a) | `c0_conv_cp_failed_preparation_unwinds_completely` | Q12: fail the source step and keep the destination obligation; Q13: fail the source step and keep its lease |
| Batch B holds the destination write obligation and the source read obligation (CP-4) | `c0_conv_cp_batch_holds_both_obligations_vulkan` | Q14: register the batch without the read obligation; Q15: register it with no destination obligation; Q16: key the destination obligation to another allocation |
| A failed submission is disposed of by its `gpu_submitted` answer, by key (CP-4b) | `c0_conv_cp_unsent_submission_cancels`, `c0_conv_cp_uncertain_dispatch_freezes_and_closes_the_gate` | Q17: cancel the prepared obligations on an uncertain dispatch; Q18: dispose of an uncertain dispatch through a ticketless batch, so no destination key is frozen; Q19: leave the transport gate open on an uncertain dispatch |
| A failed copy submission offers nothing and discharges its leases through the service, on either `gpu_submitted` answer (spec 3.3; round-1 M-2) | `c0_conv_cp_failed_submission_offers_nothing_vulkan` (both outcomes) | Q36: offer after a failed copy; Q37: release its leases by hand instead of through the service |
| The source is excluded by the destination's paired `Recording` phase (CP-4c) | `c0_conv_cp_paired_phase_excludes_the_source_vulkan` | Q20: skip the destination's `Recording` transition at selection; Q21: release it before the copy is submitted |
| The read obligation is registered by the production path (CP-4d) | `c0_conv_cp_read_obligation_has_a_production_caller_vulkan` | Q22: drive the criterion from a hand-built batch instead of the route |
| A copy that submitted but could not register its wake offers nothing and keeps its batch (spec 3.3) | `c0_conv_cp_wake_registration_failure_displaces_vulkan` | Q23: offer on generic availability after a failed wake registration; Q24: drop the batch |
| Cancellation covers the copy wait in **both** production operations (spec 3.3; round-1 M-3) | `c0_conv_cp_output_removal_cancels_the_copy_wait_vulkan`, `c0_conv_cp_whole_queue_clear_takes_the_copy_wait_vulkan` | Q25: cancel only A's pending completions on output removal; Q38: omit the copy wait from the whole-queue clear, then prove a stale completion survives a VT suspend |
| No owner commit on this route carries an input fence, and no request carries a descriptor (CP-6) | `c0_conv_cp_commit_carries_no_input_fence_vulkan` | Q26: attach the copy fence to the commit request |
| No legacy primary write is issued at the copied submit site on an Owner device (CP-8) | `c0_conv_cp_no_legacy_write_at_the_copied_site_vulkan` | Q27: force the legacy branch at that site — and the transport gate's refusal does not count as the observation |
| The destination retires under the ledger and every §4.2 gate (CP-9) | `c0_conv_cp_destination_retires_under_the_ledger_vulkan` | Q28: release the pool slot at the ack; Q29: drop one gate |
| The source is released by B's read obligation alone (CP-10) | `c0_conv_cp_source_released_by_the_read_obligation_vulkan` | Q30: release the source at flip retirement; Q31: release it when A's batch retires |
| A retained destination allocation registers no obligation and is not released (CP-11) | `c0_conv_cp_retained_destination_registers_nothing_vulkan` | Q32: register the retained allocation as if displaced |
| The copied route works cross-device on real hardware (§6) | `c0_hw_cp_copied_route_cross_device_drm` | Q33: misroute the copy off the sink's device; Q34: scan out the source instead of the destination |

---

### Task 1: A copied output that can enter `Owner`, and the fixture that builds one

**Files:** `platform.rs` (`check_owner_eligibility` at `:3425`, `validate_owner_output_kinds` at `:90`, `OwnerEligibilityError` at `:75`), the Owner live fixture and the `c0_conv` fixture helpers; tests.

**Interfaces produced:** a fixture entry that yields an Owner-eligible copied output (both halves adopted) for every later task, and the eligibility predicate every later task's route selection rests on.

**Invariants (spec §4, CP-7).**
- A copied pool is eligible when its `destinations` **and** its `sources` have a managed key for every bo. `display_pool()` already resolves to `destinations` for a copied pool (`vk/scanout.rs:774`); the source half needs its own check.
- A half without a managed adoption keeps the device out of `Owner`, reported as `Unmanaged`. No refusal is lost: `NoOutputs` and `Missing` are untouched, and `CopiedScanoutRoute` stops being a refusal on its own.
- Legacy behaviour is unchanged for every device that does not enter `Owner`.

**First step, before any code:** build the copied fixture and report what it takes. Spec §9.3 delegates this and bounds it: the fixture may not stand in for the hardware run of Task 8. If a copied pool cannot be constructed at fixture level on this machine, that is an **F8 stop** with the evidence, not a fabricated pool.

**Second report, with the same step (spec §9.4):** whether any site other than the readback at `backend.rs:18716` resolves a copied pool's managed key as absent. A site found here is **reported, not repaired** — the managed-storage access debt is out of scope by the user's decision of 2026-09-22.

**Named tests:** `c0_conv_cp_eligibility_requires_both_halves` and `c0_conv_cp_unmanaged_source_keeps_the_device_legacy` (deterministic, through the validator and through `check_owner_eligibility`); `c0_conv_cp_copied_fixture_builds_a_managed_output_vulkan`; `c0_conv_cp_legacy_copied_route_unchanged` (the characterisation test every later task keeps green).

- [ ] Steps: the fixture report; tests; red; implement; checks; stop dirty and report.

---

### Task 2: The prepared copy, and its wake

**Files:** `copied_owner.rs` (new), `scene.rs` (the fork point in the copied completion handler, `:3990`-`4002`), `vk/scanout.rs` (`submit_copy_with_fence` at `:1817`), `platform.rs` (the completion registration at `:4891`), `resources/gpu.rs` if the preparation needs an entry beside `prepare_retirement_batch`; tests.

**Interfaces produced:** the preparation entry every later task builds on. It takes the generation's destination and source keys and produces the batch, the two obligations, the registered wake, and **the receipt** — at minimum the destination `AllocationKey` and the `ObligationId` — which Task 3 consumes and Tasks 4 to 6 dispose of.

**Three steps before any code, each reported:**
1. **Decision 10** — where a `FenceTicket` for the sink context comes from.
2. **Decision 11** — which container the copy's wake registers in, and how both cancellation operations reach it.
3. **Spec §9.5** — enumerate every production consumer other than tick selection that can reserve a copied pool's source allocation, and state for each why it cannot take the source between A's retirement and B's read reservation. An unenumerated consumer is an F8 stop, not an assumption.

**Invariants (CP-4, CP-4a, CP-4c, CP-4d).**
- The order is fixed: A's batch registered and serviced (its source write lease drops with it) → destination `Write` reserved and its GPU obligation registered → source `Read` reserved and its read obligation registered → **then** the copy submitted → the batch built owning both leases and both obligations, ticket bound, registered → the wake registered.
- A failure at either reservation step cancels every obligation already registered for the attempt and drops every lease already taken, as `cancel_pre_submit_batch` (`resources/gpu.rs:370`) does. Nothing is submitted; the generation is `Displaced`.
- No lease is shared between A's batch and B's, taken out of a registered batch, or acquired while another holder is live.
- The read obligation is registered **by this path**. It has no production caller today; this is it.
- The source's exclusion while it has no lease is the destination's paired `Recording` phase (`platform.rs:6374`), not scheduling.
- Nothing is offered in this task: a prepared generation stays `Rendering` until Task 3. That is the task's own observable, not a gap.

**Named tests:** `c0_conv_cp_obligations_precede_the_copy_vulkan`, `c0_conv_cp_failed_preparation_unwinds_completely` (deterministic where the service can be made to refuse), `c0_conv_cp_batch_holds_both_obligations_vulkan`, `c0_conv_cp_read_obligation_has_a_production_caller_vulkan`, `c0_conv_cp_paired_phase_excludes_the_source_vulkan`.

- [ ] Steps: the three reports; tests; red; implement; checks; stop dirty and report the fork point and the Owner function.

---

### Task 3: The receipt's consumer — promotion and the offer gate

**Files:** `copied_owner.rs`, `scene.rs` (the promotion site), `owner_buffer.rs` if the waiting stage needs it; tests.

**Interfaces consumed:** Task 2's receipt and its registered wake. **Every test in this task runs through Task 2's production preparation** — no injected receipt, no test seam (round-1 M-1).

**Invariants (CP-1, CP-2, CP-2a, CP-3).**
- A copied generation may become `Desired` and be offered only when `has_pending_obligation(destination_key, obligation_id)` is false **and** `is_frozen(destination_key)` is false.
- Nothing else is authority. `service_completions` returns generic keys and a service-wide error (`resources/mod.rs:359`); neither its `Ok` nor its `Err` decides this generation. The composed precedent logs a service failure and offers anyway (`scene.rs:3965`) — **that shape is not copied here**, and a test must pin the difference.
- The owner buffer gains no state: `Rendering` covers both producer stages and the waiting stage discriminates.

**Named tests:** `c0_conv_cp_offer_waits_for_the_destination_obligation_vulkan`, `c0_conv_cp_readable_fence_with_pending_obligation_offers_nothing_vulkan`, `c0_conv_cp_frozen_destination_offers_nothing_vulkan`, `c0_conv_cp_desired_at_copy_retirement_vulkan`, `c0_conv_cp_displaced_during_copy_offers_nothing_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 4: Failure after submission, and both cancellation paths

**Files:** `copied_owner.rs`, `platform.rs` (`cancel_scanout_render_completions_for_output` at `:4968`, `clear_scanout_render_completions` at `:4987`), `vk/scanout.rs` (`recover_copy_failure` at `:2530`); tests.

**Interfaces produced:** the disposition entry every later failure path calls.

**Invariants (CP-4b, spec 3.3).**
- A submission failure is disposed of through `abandon_unsubmitted_batch` (`resources/gpu.rs:395`): provably not dispatched → cancel the prepared obligations, closing the transport gate only if the cancel fails; may have dispatched → close the gate and freeze the prepared entries, destination and source both. Never a ticketless batch: `quarantine_gpu_batch` takes the keys it freezes from the batch's obligation (`resources/mod.rs:1563`), so a batch without one freezes nothing.
- **On either outcome the generation is `Displaced` and nothing is offered, and every lease is discharged through the service rather than by hand** (round-1 M-2). Disposing of the resources correctly and then promoting the generation anyway is the failure this criterion exists to catch.
- A copy that submitted and then failed to register its wake keeps its batch registered, displaces the generation, removes partial waiter state and offers nothing. Generic availability is never an offer signal.
- **Both cancellation operations take the copy wait** (round-1 M-3): the per-output cancellation (`platform.rs:4968`) and the whole-queue clear (`platform.rs:4987`) that VT suspend (`platform.rs:6634`), `platform.rs:7664` and `scene.rs:3128` call. A wait that survives a VT suspend can deliver a stale completion later; that is what Q38 forces.
- A newer generation supersedes without aborting the copy in flight; its offer is discarded.

**Named tests:** `c0_conv_cp_unsent_submission_cancels`, `c0_conv_cp_uncertain_dispatch_freezes_and_closes_the_gate`, `c0_conv_cp_failed_submission_offers_nothing_vulkan` (both `gpu_submitted` outcomes), `c0_conv_cp_wake_registration_failure_displaces_vulkan`, `c0_conv_cp_output_removal_cancels_the_copy_wait_vulkan`, `c0_conv_cp_whole_queue_clear_takes_the_copy_wait_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 5: The offer, and the owner commit

**Files:** `copied_owner.rs`, `scene.rs` (the offer queue), `admission.rs` only if the conductor needs the copied producer named; tests.

**Interfaces consumed:** the offer Task 3 gates and pushes. This task takes it from the conductor to a completed commit; it does not re-decide when an offer may exist.

**Invariants (CP-1, CP-6, and 2c-ii's admission rules unchanged).**
- The offer carries the same shape as the composed route's; the conductor learns nothing new about the route.
- The commit built for a copied generation carries **no input fence**, and the request stays byte-only — host→helper frames carry no descriptors (`executor/mod.rs:851`), and `SCM_RIGHTS` remains helper→host for out-fences only (`transport.rs:67`).
- No admission rule is changed, added or bypassed for this route.

**Named tests:** `c0_conv_cp_commit_carries_no_input_fence_vulkan`; `c0_conv_cp_end_to_end_copied_frame_vulkan` (on the copied fixture: render → copy → obligation retired → offer → `Accepted` → `HardwareComplete` → damage applied).

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 6: Retirement and release

**Files:** `copied_owner.rs`, `resources/commit.rs` if the ledger needs the copied destination named, `platform.rs` (the pool slot); tests.

**Invariants (CP-9, CP-10, CP-11).**
- The destination retires under the owner ledger: `KmsRelease` discharged by the real completion, the pool slot released only after every gate of 2c-iii §4.2, never at the ack.
- The source is released by B's read obligation alone.
- Two generations sharing the same destination allocation for the same member register no new obligation and release nothing.

**Named tests:** `c0_conv_cp_destination_retires_under_the_ledger_vulkan`, `c0_conv_cp_source_released_by_the_read_obligation_vulkan`, `c0_conv_cp_retained_destination_registers_nothing_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 7: Exclusivity at the copied submit site

**Files:** `platform.rs` (`submit_copied_scanout` at `:6395`), `copied_owner.rs`; tests.

**Invariants (CP-8, parent §6.3).**
- On an `Owner` device no legacy primary write is issued at this site. The Legacy body stays byte-for-byte what it is today.
- The mutation is applied **at the site** — force the legacy branch — and must break a named test whose observation is the write that reaches the device, **not** the transport gate's refusal. A test that observes the gate does not satisfy this criterion.
- This is what closes R8 for the copied route: plan Ci could cover it only in the pure validator because no fixture built a copied output.

**Named tests:** `c0_conv_cp_no_legacy_write_at_the_copied_site_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 8: The cross-device hardware test

**Files:** `backend.rs`, beside Ciii's `c0_hw_ciii_owner_route_on_card1_drm` (`backend.rs:65619`); no production change expected.

**Invariants (spec §6).**
- The run forces the renderer to the amdgpu iGPU (`card0`) while KMS and the connected output stay on `card1`/`HDMI-A-2`, and **records the two distinct device identities it used**. A run that cannot show two identities has not tested this criterion.
- It drives render → copy → offer → `Accepted` → `HardwareComplete` → damage applied, and proves the destination was not offered before its obligation retired — by recording that obligation's registration **under the destination's own key** and then its retirement. Fence order does not establish it.
- Its own mutations, on the same hardware under the same filter: misroute the copy off the sink's device (Q33), and scan out the source instead of the destination (Q34). CP-8's mutation tests a different criterion and does not substitute.
- If the forced-renderer configuration is not reachable on this machine, that is an F8 stop with the evidence.

**The implementer writes the test; the coordinator runs it from tty2 with the user's approval**, as in Ciii Task 6. Nothing in this task may run a modeset, take DRM master or touch `_drm` filters inside the codex run.

**Named test:** `c0_hw_cp_copied_route_cross_device_drm`.

- [ ] Steps: the test; the invocation and filter, written down; stop dirty and report. The coordinator runs it.
