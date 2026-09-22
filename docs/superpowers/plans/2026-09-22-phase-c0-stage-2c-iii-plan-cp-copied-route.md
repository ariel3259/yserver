# Stage 2c-iii, plan Cp — the copied scanout route

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), run with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters (`c0_conv_cp_`, `c0_conv_cfb_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_ci_`, `c0_conv_cir_`) with `--include-ignored`, never `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time. Stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code, stop and report it (F8); never silently substitute a test shape.

**Revision 3 (2026-09-22)** — incorporates codex round 2 (`../findings/2026-09-22-stage-2c-iii-plan-cp-review-round2.md`: 1 blocking, 2 major, all verified and accepted; round 1's three findings audited as applied).
- **B-1:** the blocking finding reached back into the spec, now at **revision 5**. The copied compose arm renders with no resource service and no managed batch (`scene.rs:6532`, `submit_copied_scanout_render` at `scene.rs:10041`), while the shared arm beside it passes the service and receives one (`scene.rs:6521`). Stage A's source was therefore unowned by the service for the whole of A's write, and the paired BO phase excludes only tick selection. **Task 2 now converts stage A as well**, with Q39 and Q40.
- **M-1 and M-2:** both said the same thing — the plan delegated a real design choice to the implementer and called it a report, contradicting its own F8 rule. **Decisions 10 and 11 now decide.** The sink gets its own `FencePool` over `sink_vk`, because `FencePool` is built on one `Arc<VkContext>` and the platform's is the main device's (`platform.rs:2870`), which cannot signal a sink submission. The wake reuses the existing container, because both cancellation operations already clear it and the loop already drains it at three sites; the entry gains a stage discriminator.

**Revision 2 (2026-09-22)** — incorporates codex round 1 (`../findings/2026-09-22-stage-2c-iii-plan-cp-review-round1.md`: 0 blocking, 3 major, all three verified against the tree and accepted).
- **M-1:** revision 1's Task 2 proved the promotion predicate before any production path could produce a receipt, and allowed an interim test seam to carry its evidence — the defect plan Ciii's round 1 found as its own M-1. **Tasks 2 and 3 are swapped**: the prepared copy (with its wake registration) lands first and produces the receipt; the promotion that consumes it runs its acceptance tests through that production path. The test-seam permission is removed.
- **M-2:** the spec's §8.2 row "a failed copy submission offers nothing and discharges its leases through the service" was dropped when CP-4b was rewritten around `gpu_submitted`. Restored as its own criterion with Q36 and Q37, and Task 4 now states `Displaced` and no offer for **both** `gpu_submitted` outcomes.
- **M-3:** cancellation has two distinct production operations and revision 1 named one. `cancel_scanout_render_completions_for_output` (`platform.rs:4968`) is per output; `clear_scanout_render_completions` (`platform.rs:4987`) clears the whole queue and has three callers — VT suspend (`platform.rs:6634`), `platform.rs:7664` and `scene.rs:3128`. Decision 11 now covers both, with Q38.

**Revision 1 (2026-09-22).**

**Goal:** Convert the copied composed producer (`submit_copied_scanout`, `platform.rs:6395`) to the Owner route, so that a device with a copied-route output can enter `Owner` at all. This is the item plan Ciii's acceptance carries as still owed to 2c-iii, and the first fixture in the project that builds a copied output — which is what turns Ci's validator-only R8 into an executable exclusivity case.

**Architecture:** Eight tasks. The copy becomes the **last stage of the producer**: renderer A writes the source, the sink copy writes the destination, and only when the resource service has retired the destination's write obligation does a `ComposedOffer` reach the conductor. The copy's `sync_file` wakes the core loop; it never authorizes anything. Owner-route code lives in a new `copied_owner.rs` reached from the existing fork point in the copied completion handler; nothing is interleaved into a Legacy function body (the Cii/Ciii constraint). Production is unchanged (C0-R8).

**Spec:** `docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md` at revision 5, whole. Read first: §3 (3.1 the offer boundary and CP-2a, 3.2 the ownership sequence CP-4 through CP-5, 3.3 failure), §4 (eligibility and exclusivity), §5 (retirement), §6 (the hardware run) and §8.2 (every row is an exit criterion here). Then the three review findings it incorporates (`../findings/2026-09-22-stage-2c-iii-copied-route-design-review-round{1,2,3}.md`) — each one records what the tree actually does at the seams this plan touches, and the third records the pattern that governs this plan's method. Then the parent, `2026-09-19-phase-c0-stage-2c-iii-conversion-design.md` §6.3.

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
10. **The sink's fence ticket is the sink pool's own** (spec §9.1; round-2 M-1). `submit_copy_with_fence` (`vk/scanout.rs:1817`) already takes a `vk::Fence`, used only by the probe. `FencePool` is built on one `Arc<VkContext>` and the platform's is the **main** device's (`platform.rs:2870`), which cannot signal a submission on `sink_vk` (`vk/scanout.rs:949`). So **the copied scanout pool owns a `FencePool` over `sink_vk`**, created and destroyed with the pool, and a ticket is acquired per copy submission through the existing `acquire_fence_ticket` shape (`platform.rs:6005`). The ticket moves into B's `GpuObligation` and becomes reusable at exactly the existing point: the obligation's retirement drops the batch, the last `FenceTicket` clone drops, and the pool recycles it only if it was observed signalled — an unsignalled ticket leaks and sets `renderer_failed`, as today. No new lifetime rule is introduced.
11. **The wake reuses the existing completion container** (spec §9.2; round-1 M-3, round-2 M-2). The copy's `sync_file` is registered in the queue behind `CompletionPoller` that already carries A's completions (`platform.rs:4891`), **not** a sibling. The reason is teardown and drain: both cancellation operations already clear that one container — `cancel_scanout_render_completions_for_output` (`platform.rs:4968`) per output and `clear_scanout_render_completions` (`platform.rs:4987`), which VT suspend (`platform.rs:6634`), `platform.rs:7664` and `scene.rs:3128` call — and the loop already drains it at three sites. A sibling would have to re-earn all six connections. The contract:
    - **Payload:** the entry (`PendingScanoutRenderCompletion`, `platform.rs:620`) gains a **stage discriminator** beside its `job_id`, `output_key`, `bo_idx` and `fd`. `fd: None` keeps its meaning — an already-signalled payload that bypasses polling.
    - **Correlation:** a drained completion is matched by `(output_key, bo_idx, job_id)` **and** its stage, the way `copied_render_completion_matches` (`scene.rs:133`) matches A's today. A B-stage completion never satisfies an A-stage waiter or the reverse.
    - **Drain:** unchanged. `drain_scanout_render_completions` returns both stages and the copied Owner handler dispatches on the stage.
    - **Cancellation:** both operations take both stages by construction, because there is one container. Q38 is what proves the implementation did not give itself a private one anyway.

    CP-2 holds regardless: whatever wakes, only the receipt promotes.

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
| The offer follows the retirement of this generation's destination obligation (CP-1, CP-2a) | `c0_conv_cp_offer_waits_for_the_destination_obligation_vulkan`, `c0_conv_cp_readable_fence_with_pending_obligation_offers_nothing_vulkan`, `c0_conv_cp_frozen_destination_offers_nothing_vulkan` | Q3: offer at copy submission; Q4: offer at A's completion; Q5: promote on a readable fence while the obligation is pending; **Q6: F8 — no production path reaches discharged-plus-frozen for this obligation, so the `is_frozen` half is defensive and has no reachable discriminator**; Q7: promote on a successful `service_completions` that retired another key |
| A copied frame completes end to end at fixture level (§8.1) | `c0_conv_cp_end_to_end_copied_frame_vulkan` | Q35: apply the damage transaction at `Accepted` instead of `HardwareComplete` |
| The owner buffer reaches `Desired` at the copy's retirement, not A's (CP-3) | `c0_conv_cp_desired_at_copy_retirement_vulkan` | Q8: promote when A completes |
| A generation displaced during the copy behaves as one displaced during the render (CP-3) | `c0_conv_cp_displaced_during_copy_offers_nothing_vulkan` | Q9: offer a generation displaced during the copy |
| Stage A's source write lease and obligation exist before A's handles reach the GPU (CP-4; round-2 B-1) | `c0_conv_cp_stage_a_source_is_managed_vulkan` | Q39: register A's source obligation at A's completion instead of before its submission; Q40: leave stage A unmanaged on the Owner route |
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

## Two defects the hardware run found (2026-09-22)

Task 8's cross-device run stalled on its fourth Owner frame. Six runs and four
diagnostics later the mechanism is fully proven, and it is **two independent
defects**, one ours and one upstream's. Neither was introduced by this plan;
both were found by it.

**Evidence, from the sixth hardware run:**

```text
retry_skip_counts=[("PendingAcks",0), ("RetryDeadline",0), ("EmptyDamage",0),
                   ("NoBO",1441), ("NoPool",1), ("NothingPending",0)]
descriptor_pool_slots_in_use=3/3
bo_idx=2 phase=Recording acquired_then_skipped_same_tick=true
  acquisition_skip_events=[{ tick: 1, bo_idx: 2, generation: 5, reason: "NoPool" }]
owner_progress=[(0, 7, Current, "OwnerSubmitted")]
render_completion_waiters=[]
```

**Defect 1 — ours: the descriptor-pool slot is lost at admission.**
`OwnerBuffer::into_submitted` (`owner_buffer.rs:317`) destructures `Desired`
with `..`, which swallows its `descriptor_slot`, and builds a `Submitted`
variant that **has no such field** (`owner_buffer.rs:66`). The intended release
site, `take_owner_composed_resources` (`admission.rs:1561` into `scene.rs:1985`),
then calls `take_descriptor_slot()` on a `Submitted` buffer and gets `None`, so
the ring is never given the slot back. `CompletionRetired` releases no slot
either (`scene.rs:2216`). **Every composed Owner frame therefore leaks one
descriptor-pool slot.** With a three-slot ring, the fourth frame can never
acquire one: exactly three Owner frames succeeded on hardware and the fourth
did not, deterministically, not as a race.

This is **our** code, from Ci-refactor (`39a68bac`, accepted 2026-09-19), and it
is **not specific to the copied route** — it affects every composed Owner frame.
Ci, Cii and Ciii did not catch it because no fixture runs four consecutive Owner
composed frames. R8 keeps it out of production today; it would have surfaced at
activation in stage 3 or 4.

**Defect 2 — upstream's: a skip after acquisition strands the buffer.**
`acquire_managed_scanout_bo` moves the destination `Free → Recording`
(`platform.rs:6374`), and the `NoPool` branch (`scene.rs:6694`, now `:6853`)
returns `Skipped(NoPool)` **without restoring that phase**, while the
fence-ticket failure three lines below does release its own pool slot. Other
fallible paths after acquisition lack rollback too. The blame is upstream's:
Jos, commit `02bafec3`, 2026-09-03. Its effect here is to turn a transient pool
shortage into a permanent stall — one `NoPool` stranded `bo_idx=2`, and the
1441 following ticks all skipped with `NoBO`.

**Relationship.** Defect 1 causes the exhaustion; defect 2 makes it
irreversible. Fixing either alone improves matters; defect 1 must be fixed
regardless, because it blocks the Owner route in production.

**Disposition (user, 2026-09-22):** defect 1 is fixed in this branch and
carries its own evidence. Defect 2 is reported upstream with the evidence
above, as the Legacy dormancy bug was; whether we also patch it locally is a
separate decision.

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

**One step before any code, reported (spec §9.5):** enumerate every production consumer other than tick selection that can reserve a copied pool's source allocation, and state for each why it cannot take the source between A's retirement and B's read reservation. An unenumerated consumer is an F8 stop, not an assumption. Decisions 10 and 11 are **decided in this plan** and are not yours to reopen; implement them as written and report only if the tree contradicts one.

**Invariants (CP-4, CP-4a, CP-4c, CP-4d).**
- **Stage A is converted too** (round-2 B-1). On an Owner copied output the compose that writes the source is prepared the way the shared arm already is: the source is reserved `Write` and its GPU obligation registered **before** A's raw handles reach the GPU. The copied arm passes no service today (`scene.rs:6532`); this task is what gives it one. Legacy copied outputs keep calling `submit_copied_scanout_render` exactly as they do now.
- The order is fixed: A's batch registered and serviced (its source write lease drops with it) → destination `Write` reserved and its GPU obligation registered → source `Read` reserved and its read obligation registered → **then** the copy submitted → the batch built owning both leases and both obligations, ticket bound, registered → the wake registered.
- A failure at either reservation step cancels every obligation already registered for the attempt and drops every lease already taken, as `cancel_pre_submit_batch` (`resources/gpu.rs:370`) does. Nothing is submitted; the generation is `Displaced`.
- No lease is shared between A's batch and B's, taken out of a registered batch, or acquired while another holder is live.
- The read obligation is registered **by this path**. It has no production caller today; this is it.
- The source's exclusion while it has no lease is the destination's paired `Recording` phase (`platform.rs:6374`), not scheduling.
- Nothing is offered in this task: a prepared generation stays `Rendering` until Task 3. That is the task's own observable, not a gap.

**Named tests:** `c0_conv_cp_stage_a_source_is_managed_vulkan`, `c0_conv_cp_obligations_precede_the_copy_vulkan`, `c0_conv_cp_failed_preparation_unwinds_completely` (deterministic where the service can be made to refuse), `c0_conv_cp_batch_holds_both_obligations_vulkan`, `c0_conv_cp_read_obligation_has_a_production_caller_vulkan`, `c0_conv_cp_paired_phase_excludes_the_source_vulkan`.

- [ ] Steps: the §9.5 report; tests; red; implement; checks; stop dirty and report the fork point, the Owner function, and where stage A's preparation was inserted.

---

### Task 3: The receipt's consumer — promotion and the offer gate

**Files:** `copied_owner.rs`, `scene.rs` (the promotion site), `owner_buffer.rs` if the waiting stage needs it; tests.

**Interfaces consumed:** Task 2's receipt and its registered wake. **Every test in this task runs through Task 2's production preparation** — no injected receipt, no test seam (round-1 M-1).

**Invariants (CP-1, CP-2, CP-2a, CP-3).**
- A copied generation may become `Desired` and be offered only when `has_pending_obligation(destination_key, obligation_id)` is false **and** `is_frozen(destination_key)` is false.
- Nothing else is authority. `service_completions` returns generic keys and a service-wide error (`resources/mod.rs:359`); neither its `Ok` nor its `Err` decides this generation. The composed precedent logs a service failure and offers anyway (`scene.rs:3965`) — **that shape is not copied here**, and a test must pin the difference.
- The owner buffer gains no state: `Rendering` covers both producer stages and the waiting stage discriminates.

**Task 3 reachability report / F8 (2026-09-22).** The state needed to
discriminate Q6 — this generation's destination obligation discharged while
that destination key is frozen — is not reachable through the copied-route
production entries. `service_completions` sends a signalled batch through
`validate_gpu_batch`, which rejects a frozen entry before removing its
obligation; the rejected batch goes to `quarantine_gpu_batch`, which retains
the batch and its obligations while freezing its keys. The other production
freeze paths (`abandon_unsubmitted_batch`/`freeze_uncertain_batch`, failed
read recording, and commit quarantine) also freeze without discharging the
copied destination obligation. After the successful batch commit, the copied
wake checks the receipt immediately; the destination is not in owner-commit
resources before promotion. While B is pending, its destination `Write` lease
also excludes another production read/write/KMS reservation for the same key,
so no second production batch or commit can freeze that key in the gap.
Therefore the `is_frozen` half of CP-2a is a defensive check with no reachable
discriminator: Q6 cannot be proven as written and this plan records that fact
instead of claiming the existing pending-obligation test proves it. The
existing `c0_conv_cp_frozen_destination_offers_nothing_vulkan` test remains a
valid frozen-plus-pending safety test and is not weakened.

The Q3 and Q4 mutations have distinct code mechanisms. Q3 cannot create a
valid offer at the sink copy submission because
`prepare_owner_copy_after_render_completion` submits the copy and then only
binds/registers B's batch, registers its wake, and returns the receipt; the
generation is still `Rendering` with no `Desired` buffer to offer. Q4 cannot
produce a valid offer at A's completion because that branch only prepares B,
records the receipt, and changes the acknowledgement to
`OwnerCopyWaiting`; it does not call `into_desired` or enqueue an offer. Those
operations are reachable only from the later `CopiedOwnerCopy` wake after the
receipt's gate is checked.

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

**Task 6 reachability report / F8 (2026-09-22).** The Q32 state needed to
discriminate an erroneous retained-destination registration — two live copied
Owner generations for one member naming the same destination `AllocationKey` —
is not reachable through the copied production entries. Selection transitions
the destination to `Recording`, then the Owner route to `Owner`; a later
selection accepts only `Free`, and the old generation reaches `Free` only after
its owner-ledger retirement and resource-service release. Thus a second
generation cannot share the allocation while the first generation is still in
the ledger, which is exactly the interval in which
`register_commit_dependencies` could distinguish retained from displaced.
The named Vulkan test records this production exclusion (`bo_idx` differs)
and does not claim to prove P3-3's retained branch. Q32 therefore has no
reachable discriminator in this copied fixture; the existing commit-ledger
tests remain the proof of the shared-key predicate.

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
- The run forces the renderer to the amdgpu iGPU (`card0`) while KMS and the connected output stay on `card1`/`HDMI-2`, and **records the two distinct device identities it used**. A run that cannot show two identities has not tested this criterion.
- It drives render → copy → offer → `Accepted` → `HardwareComplete` → damage applied, and proves the destination was not offered before its obligation retired — by recording that obligation's registration **under the destination's own key** and then its retirement. Fence order does not establish it.
- Its own mutations, on the same hardware under the same filter: misroute the copy off the sink's device (Q33), and scan out the source instead of the destination (Q34). CP-8's mutation tests a different criterion and does not substitute.
- If the forced-renderer configuration is not reachable on this machine, that is an F8 stop with the evidence.

**The submission-latency measurement (user, 2026-09-22).** The conversion
moves the copy's wait from the kernel to userspace: the Legacy copied route
hands the copy fence to the kernel as `IN_FENCE_FD` and queues the flip
immediately, while the Owner route waits for that fence, services, offers and
only then submits the commit. The flip therefore leaves at least one event-loop
hop later. Nothing in this plan measured that, so this run does, on the one
machine where both routes exist:

- **submission delay** — from the copy fence observed signalled to the commit
  submitted to the kernel, in microseconds, per frame;
- **missed-vblank fraction** — over N frames, how many land on a later vblank
  than the one they would have made, read from the completion MSCs.

Both are measured for the copied route under `Legacy` and under `Owner`, same
workload, same session. The criterion is fixed **before** the numbers are seen:
if the submission delay is well under one vblank period and the missed-vblank
fraction is indistinguishable between the routes, the trade is defended in the
spec and the PR text. If either says otherwise, it is recorded as a measured
limitation of C.0 and becomes an evidence-backed requirement for C.1 — namely
that the producer-fence transfer C.0 §14 already projects must count the copied
route among its consumers, which that section does not promise today.

**The implementer writes the test; the coordinator runs it from tty2 with the user's approval**, as in Ciii Task 6. Nothing in this task may run a modeset, take DRM master or touch `_drm` filters inside the codex run.

**Named test:** `c0_hw_cp_copied_route_cross_device_drm`.

**Task 8 implementation report (2026-09-22).** The test is at
`crates/yserver/src/kms/render/backend.rs:68278`, with the test-only latency
collector in `crates/yserver/src/kms/render/platform.rs:679` and the two
copy-fence hooks at `platform.rs:6770` and
`crates/yserver/src/kms/render/copied_owner.rs:420`; the Legacy submission
hook is at `platform.rs:6792`. For Owner, the test-only recorder timestamps
the helper's atomic-ioctl submission boundary by subtracting the helper's
measured `helper_duration_ns` from the received `Accepted` event timestamp;
the `Accepted` event is still printed as the protocol milestone. The hardware
run uses one
live fixture and one timing session: four Legacy copied frames, then four Owner
copied frames on the same card1 master/HDMI-2 modeset. The output records
`renderer_primary`/`renderer_node` and `sink_primary`/`sink_render`. While the
generation is still `Rendering`, the test observes the pending destination
obligation under its own key and labels the generation as waiting, not offered.
The copied completion handler then checks the same receipt and frozen state,
emits the promotion-gate line after both checks pass, enqueues the real
`ComposedOffer`, and emits that fact after insertion; the helper `Accepted`
observation follows. Owner completion event lines are emitted from the event
batch before that batch is synchronously passed to `route_owner_event_batch`,
and all damage checks happen after routing.

**Task 8 hardware follow-up (2026-09-22).** The first real-device run completed
four Legacy frames and reached `Accepted`, `HardwareComplete`, and
`CompletionRetired` for the first Owner commit, then failed the damage assertion.
That assertion compared the length of `BufferAgeRing`, which is bounded to
`scanout_bo_count + 1` (four entries for this three-buffer pool). The four
Legacy frames had already filled it, so a successful Owner push evicted the
oldest generation without increasing the length. The failure therefore did
not establish whether the Owner damage transaction applied. The harness now
compares the newest retired damage generation, which advances when the actual
damage retirement path pushes even after the ring is full; the 15-second
deadline and damage conditions are unchanged. The pre-retirement log names
were corrected, and a separate test-only line records the real offer enqueue
after the exact destination obligation check. This remains pending a coordinator
hardware rerun; the first run is not a production F8 finding.

**Task 8 third hardware follow-up (2026-09-22).** The coordinator's third real
device run recorded renderer `226:0` / `renderD128`, sink `226:1` / `renderD129`,
`Different`, and HDMI-2 at 1920x1080@60. Four Legacy frames applied damage.
Owner frames 0–2 each passed the destination obligation, promotion gate, offer,
`Accepted`, `HardwareComplete`, `CompletionRetired`, and damage checks. Owner
frame 3 did not produce a newer damage generation within the existing 15-second
deadline. This run exposed a harness gap: its wait loop serviced the resource
service, Owner/DRM events, and render completions, but only called the compositor
once at frame start. `retire_owner_current` applies the release gates from
`tick_one_output`; if an allocation becomes releasable after that initial tick,
the wait loop never asks the production tick to retry acquisition. The hardware
test now retries that same tick while the current frame has not created a newer
Owner generation. The frame count and deadline are unchanged. If frame 3 still
times out, the test prints every destination BO's index and `BoPhase`, whether
its managed allocation is releasable, and its Owner ledger state plus a
`Current` flag. The third run suggested a harness release-gate gap, but did not
capture the requested pool snapshot. The fourth-run evidence below shows that
this pump did not explain the later post-acquisition stall.

**Task 8 fourth hardware follow-up (2026-09-22).** The coordinator's fourth
device run confirmed that Owner frame 3 selected `bo_idx=2` (`Recording`), so
the stall is after acquisition. `bo_idx=1` remained `OnScreen` from Legacy and
outside the Owner ledger. `acquire_managed_scanout_bo` selects only `Free`
destinations, and the Owner path does not return that Legacy slot to `Free`, so
the shared-session harness leaves two destinations for Owner transport. This
reduces buffering but does not account for the selected frame failing to create
an Owner generation.
The timeout snapshot did not name the internal generation stage or show
pending completion jobs/readiness, so it cannot distinguish an undrained stage A
completion from a missing sink-copy submission, an undrained copy completion,
or a later pool-state gate. This run therefore does not establish either a
harness scheduling gap or a production F8 stall. The next timeout report adds
the Owner generation and `InFlightStage`, registered render/copy completion
jobs with readiness, and source plus destination releasability. No retry
deadline or frame count changed.

**Task 8 fifth hardware follow-up (2026-09-22).** The fifth snapshot closes the
stage-location question: Owner frame 3 acquired `bo_idx=2` (`Recording`), but
there was no Owner buffer entry and no render-completion waiter. The scene has
no generation gate for a current ack at `OwnerSubmitted`; the early tick gate
checks `pending_acks`, which do not contain Owner buffers. The explicit
post-acquire no-render skip is the descriptor ring's `NoPool` branch: when all
three pool slots are occupied, `tick_one_output` returns `Skipped(NoPool)`
without cancelling the just-acquired destination. Other post-acquire fallible
audit/fence-ticket paths likewise lack that rollback. The fifth diagnostic did
not print the last skip reason or descriptor-ring occupancy, so `NoPool` is the
identified code site and condition, not a proven observation of the triggering
branch. The abandoned `Recording` destination is nevertheless a production
resource leak. `bo_idx=1` is stale `OnScreen` state from the harness's shared
Legacy-then-Owner session, reducing the Owner phase to two available pool
members; once `bo_idx=2` is stranded, the current `bo_idx=0` cannot be released
until a newer Owner frame is current and no free destination remains. This is
an F8 production stop, not a harness-only starvation result. Per the F8 rule,
no production change was made; the Task 8 hardware claim remains stopped.

The same run's latency snapshot remained at zero samples. Completion-MSC
capture was conditional on a nonzero raw Legacy sequence or an exact CRTC key
in the Owner `Presented.samples` payload, even though the production router can
provide the sequence fallback/routed clock after consuming those events. The
test now records MSC after routing, using that clock fallback where the payload
omits it, and its partial output names the four pending boundaries individually.
A pipe-backed non-GPU test verifies that both transport samples close when
their fence, submission and completion boundaries arrive. The hardware run was
not repeated under this task's constraint, so its new boundary flags are the
remaining evidence needed to verify the real path.

The latency recorder now snapshots completed samples and pending frames before
the per-frame damage assertion panics. It prints every available `CP-LATENCY`
line and a `CP-LATENCY-SUMMARY status=partial` per transport, including
collected/expected frame counts, before the failure message. A successful run
prints the same evidence with `status=complete`; the final integrity checks
still require four samples per transport.

The coordinator runs it from tty2, with the user's approval, using exactly:

```bash
cargo test -p yserver --lib kms::render::backend::tests::c0_hw_cp_copied_route_cross_device_drm -- --exact --ignored --nocapture --test-threads=1
```

The latency evidence is printed as one `CP-LATENCY` line per collected frame
with `transport`, `status`, `submission_delay_us`, `expected_msc`,
`completion_msc`, and `missed_vblank`, followed by one
`CP-LATENCY-SUMMARY` line per transport with `status`, collected/expected
`frames`, `missed_vblank=N/collected`, and pending/unstarted counts. A failure
before all four frames complete prints these rows with `status=partial`. The measurement is test-only: it
duplicates the copy fence, polls it without changing production scheduling,
timestamps the fence-signalled and commit-submitted boundaries, and takes the
completion MSC from the Legacy flip event or Owner `Presented` sample.

The required mutations are replacements, not adjacent branches, and leave
CP-8's forced-legacy mutation untouched:

- **Q33 — misroute the copy off the sink device.** At
  `crates/yserver/src/kms/vk/scanout.rs:2351-2353`, replace the production
  queue-submit block

  ```rust
  self.sink_vk
      .device
      .queue_submit2(self.sink_vk.graphics_queue, &submits, fence)
  ```

  with

  ```rust
  source
      .render_vk
      .as_ref()
      .expect("Q33: copied source has no renderer Vulkan context")
      .device
      .queue_submit2(
          source
              .render_vk
              .as_ref()
              .expect("Q33: copied source has no renderer Vulkan context")
              .graphics_queue,
          &submits,
          fence,
      )
  ```

  This replaces the sink-device queue decision in
  `CopiedScanoutPool::submit_managed_copy_with_fence`; it must not be bolted
  beside the real submission.

- **Q34 — scan out the source instead of the destination.** At
  `crates/yserver/src/kms/render/scene.rs:4166-4169`, replace

  ```rust
  let managed = match service.reserve(
      prepared.identity().managed_key,
      crate::kms::render::resources::UseKind::Retain,
  ) {
  ```

  with

  ```rust
  let managed = match service.reserve(
      source_receipt.0,
      crate::kms::render::resources::UseKind::Retain,
  ) {
  ```

  This replaces the Owner promotion's destination framebuffer identity with
  the copied source receipt's allocation; it must not be added as a parallel
  path.

The implementation did not run this ignored hardware test, any `_drm` test,
render acceptance, or an unfiltered ignored test. Hardware reachability/F8 is
therefore intentionally left for the coordinator's approved tty2 run.

- [ ] Steps: the test; the invocation and filter, written down; stop dirty and report. The coordinator runs it.
