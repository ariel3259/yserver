# Stage 2c-iii, plan Cii — the direct producer

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for hardware work) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters (`c0_conv_cii_`, `c0_conv_ci_`, `c0_conv_cir_`), never `_drm`, `render_acceptance`, unfiltered `--ignored`, or anything that modesets or takes DRM master while the user is looking at the screen; no deletes outside the worktree. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests and the mutations each must catch. Execute tasks in order, one per run. You can run the `_vulkan` tests yourself: nothing is done until its tests pass on the real GPU, in debug and release. Do not ask for approval; a real design choice the plan leaves open, or a claim here that does not hold in the code, is an F8 stop you report.

**Revision 2 (2026-09-19)** — incorporates codex round 1 (`../findings/2026-09-19-stage-2c-iii-plan-cii-review-round1.md`: 1 blocking, 4 major, all verified against the tree and accepted).
- **B-1:** a missing `Presented` had no defined carrier for the clock sample its `Skip` must use. Decision 7 names the source, the binding and the F8 when no validated sample exists; Task 6 tests it with distinct samples per CRTC.
- **M-1:** S1 was unkillable — the consumer-outside-the-event-set refusal comes from closure construction (`closure.rs:215`), not from `validate_completion_context`. Task 1 now uses a structurally valid description with an invalid `CompletionContext`, and gains a real direct-dispatch registration-failure case.
- **M-2:** Task 4's evidence would have rested on the injected source, which the spec forbids. The production request and resource interfaces move into Task 4, and Task 6's tests re-run the Task 4 scenarios through the production-backed source.
- **M-3:** S4 needs a CRTC-ineligible candidate and S6 a real layout change between `decide` and `lock`; both are named now.
- **M-4:** the newer-cursor invariant gets its own case and mutation (S23).

**Goal:** Convert the direct scanout producer to the owner, behind the same transport fork plan Ci built for the composed one. That means the Present-carrying owner entry, one production eligibility predicate, the layout hooks that invalidate a queued successor, the direct producer's own owner-route module, the present-pin leases carried by value (F13b-D1), the direct commit's Present carriage, retirement promotion through the conductor, and the composed invalidation direct entry owes.

**Architecture:** The owner entry is in `crates/yserver/src/kms/owner/device.rs`, beside Ci's fallible core. **Every new owner-route path lives in its own module** — `crates/yserver/src/kms/render/direct_owner.rs` — reached from **one fork point** in `try_present_direct` and one in the retirement path, never as branches interleaved in the legacy functions (the user's constraint, spec §8.3). The 2c-ii managed seams (`managed_prepare_direct_candidate`, `managed_tag_queued_direct_successor`, `managed_confirm_direct_dispatch`, `managed_terminalize_queued_direct_successor`, `managed_undo_direct_dispatch`, `managed_dispatch_direct_successor`, `managed_enqueue_retired_direct_completion`) are the seams this plan finally drives from production; they are not rewritten unless a task says so. Production is unchanged (C0-R8): no conductor is installed there, so every production device stays `Legacy` and takes today's `submit_direct_frame`.

**Spec:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`, revision 4 with §8.3 as amended: §3.2 (registration), §3.3 (Present carriage — the direct half this time), §5.0–§5.7 (all of Cii), §8.2's rows for those sections. Read plan Ci's acceptance (`../findings/2026-09-19-stage-2c-iii-plan-ci-accepted.md`), the Ci-refactor acceptance (`../findings/2026-09-19-stage-2c-iii-plan-ci-refactor-accepted.md`) and the Ci commit messages before Task 1: they record what the GPU runs taught, including the fixture that needs a real DRM node and the readiness rule that waits for the compose batch to retire.

## Design decisions this plan fixes

1. **One fork point per producer, owner code in its own module** (user, spec §8.3). `try_present_direct` keeps choosing, pinning and importing — the prepare half — and then asks once whether this output's device takes the owner route. The owner half lives in `direct_owner.rs`; the legacy half stays exactly where it is. The same applies to the retirement path.
2. **The eligibility predicate is production's, not a copy** (§5.1). The inputs `try_present_direct` computes inline today (`backend.rs:21339`: `scanout_allowed`, `kms_outputs_active`, the hardware-cursor mode, an empty root overlay, `authoritative_root`, `has_border_clip`, the offsets, `valid_region_xid`, plus `direct_present_crtc_eligible`) move into one function that returns the decision **and** the layout/eligibility generation it was decided under. Both routes call it. `scanout_direct_eligible`'s pure core (`backend.rs:321`) stays what it is; what is extracted is the gathering of its inputs.
3. **The conductor's direct source stops guessing members** (Ci's carried item). Ci's Task 2 fills an empty `CommitResources` member set from the current state's members; the real producer supplies the `GroupMember`s of the CRTCs the direct frame covers, and the guess is removed in the same task that supplies them.
4. **The direct frame is the single Present authority** (spec §3.3, as corrected by the design's round 3). `CommitDescription::present_consumers` carries **CRTC ids** — the members of the kernel event set whose page event supplies MSC/UST — never a serial. The `CompletedPresentEvent` stays with the frame, which the conductor's confirmation already moves into the accepted slot; confirmation also binds it to the commit's `CommitId`. `Presented` delivers the sample to that frame; the frame's single publication at retirement is the only CompleteNotify that request gets.
5. **Cursor and gamma stay stage 4's** (§5.6). Cii adds no payload and no producer; what it proves is that a direct commit never carries an unchanged cursor and that a primary flip event does not retire a newer cursor generation.
6. **The `Skip` clock sample has one source and one binding (round-1 B-1).** A direct frame already carries `completion_clock: Option<PresentClockSample>`, the exact sample of its reference CRTC (`backend.rs:588`-`603`). On the owner route that field is filled **only** from an `OwnerEvent::Presented`'s sample for that frame's reference CRTC, bound by `(CommitId, reference CRTC)`; no other CRTC's sample and no other commit's may fill it. An accepted Present that reaches retirement with the field still `None` terminalizes as `Skip` with the platform's last **validated** sample for that CRTC (`present_get_completion_clock`, `platform.rs:5242`) — and if that CRTC has no validated sample at all, that is an F8 stop, not a fabricated `(0, 0)` timestamp.
7. **Test names start with `c0_conv_cii_`**, so one filter selects this plan. `_vulkan` tests carry `#[ignore = "needs live Vulkan ICD"]` and build on Ci's owner-route live fixture (`for_tests_with_vk_live_scene_real_drm`; the plain variant has no real DRM node and cannot allocate a pool).

## Limits stated

- Fixture level only; no production caller (C0-R8). The tty2 hardware run is Ciii's.
- Unflip dispatch, multi-device state, the copied route and route-selection exclusivity: Ciii.
- Cursor/gamma producers and the coordinate lane: stage 4.
- The Ci F8 stops stay open and are not re-litigated here: the Legacy dormancy bug, the missing restore `TerminalState`, device loss without an owner signal, and the copied-route fixture.

## Global Constraints

- **Production is byte-for-byte unchanged**: without an active conductor the direct path takes today's calls with the same arguments, the same pins and the same `Skip` ordering. Each task keeps a named Legacy characterisation test green.
- Owner milestones reach the scene only through `route_owner_event_batch`; tests deliver them that way (a stub behaviour or crafted events handed to it — say which), never by calling a handler directly.
- No hand-built `PendingAck`, `BoPhase`, `OwnerBuffer`, `DirectPresentFrame` or prepared generation in any test.
- Resources travel by value; nothing is bare-dropped; every token is consumed exactly once; confirmation is at the send; no retry on refusal.
- No side effect inside `debug_assert!`; fail closed, never panic, in non-test code; no test-only hook that bypasses the path it is named after.
- **Honesty rule (F8).** An unreachable scenario, a seam that does not behave as stated, a fixture that cannot carry what a test needs, or a real design choice left open: stop and report.

## Checks every task must keep green

```bash
cargo build -p yserver --bin yserver
cargo build --release -p yserver --bin yserver
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features tcp-transport -- -D warnings
cargo clippy --all-targets --features xdmcp -- -D warnings
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_conv_cii_; done
cargo test -p yserver --lib c0_conv_cii_ -- --include-ignored --test-threads=1
cargo test --release -p yserver --lib c0_conv_cii_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_ci_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_cir_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

Baseline before Task 1 (commit `528ac2a4`): `c0_conv_ci_` 38/38 with `--include-ignored` in debug and release; `c0_conv_cir_` 7/7; `c0_adm` 129/0; `c0_2ci` 180/0/21; `--lib` 1926/0/123. Every task ends at those numbers plus its own new tests. The hardware gate (287/287 at `528ac2a4`) is the coordinator's, after the last task, with the user's go-ahead.

## Exit criteria

| Criterion (spec) | Tests | Mutation that must fail them |
| --- | --- | --- |
| A Present-carrying description is begun through one public entry with its `CompletionContext` and a fallible, CommitId-aware ledger (§5.0) | `c0_conv_cii_present_entry_registers_and_refuses` | S1: skip the completion-context validation in the new entry |
| That entry keeps Ci's failure invariants (§4.0) | `c0_conv_cii_present_entry_failed_ledger_leaves_nothing` | S2: keep the slot reserved on a ledger error |
| One production eligibility predicate, same answer for both routes (§5.1) | `c0_conv_cii_eligibility_is_one_predicate`, `c0_conv_cii_eligibility_matches_legacy_vulkan` (both include a **CRTC-ineligible** candidate, round-1 M-3) | S3: let the owner route skip the border-clip input; S4: let it skip `direct_present_crtc_eligible` |
| A queued successor whose ancestor gains a border never reaches a commit, promoted or not (§5.2) | `c0_conv_cii_border_invalidates_queued_successor_vulkan`, `c0_conv_cii_border_invalidates_promoted_successor_vulkan`, `c0_conv_cii_layout_change_between_decide_and_lock_vulkan` (round-1 M-3) | S5: drop the layout-generation bump at one enumerated site; S6: skip the eligibility recheck at `lock` |
| Layout hooks cover the real sites (§5.2) | the enumeration of Task 3, each site with its own case in `c0_conv_cii_layout_hooks_bump_the_generation_vulkan` | S7: drop the bump at a second enumerated site |
| The owner route takes its own module behind one fork point; Legacy is unchanged (decision 1) | `c0_conv_cii_legacy_direct_unchanged_vulkan`, `c0_conv_cii_owner_direct_offers_instead_of_flipping_vulkan` | S8: force the legacy submit under `Owner`; S9: offer under `Legacy` |
| The producer supplies the commit's members; no guess remains (decision 3) | `c0_conv_cii_direct_members_come_from_the_producer` | S10: restore the current-state guess |
| Present-pin leases travel by value into `CommitResources` and are released only by the ledger (§5.4) | `c0_conv_cii_direct_carries_its_pins_vulkan`, `c0_conv_cii_direct_pins_released_only_by_the_ledger_vulkan` | S11: build an empty lease set; S12: release the source pin at dispatch |
| The direct commit carries its Present as CRTC ids, with the page-flip event and the context (§3.3, decision 4) | `c0_conv_cii_direct_description_carries_present_vulkan` | S13: drop `present_consumers`; S14: put a Present serial in `present_consumers` |
| One publication per Present request; `Presented` only supplies the sample (§3.3) | `c0_conv_cii_direct_present_completes_once_vulkan` | S15: publish from an owner-event consumer as well |
| A missing `Presented` terminalizes as `Skip` with the last validated clock of its own reference CRTC (§3.3, decision 6) | `c0_conv_cii_direct_missing_presented_skips_vulkan` | S16: publish `Flip` with a synthesized timestamp; S17: fill the sample from another CRTC's or another commit's `Presented` |
| Retirement promotion goes through the conductor, ordered predecessor → `Skip` → admission → publication (§5.3) | `c0_conv_cii_retirement_promotion_order_vulkan` | S18: commit the successor from the event handler again |
| A displaced successor idles once with its `Skip` deferred behind the predecessor (§5.3) | `c0_conv_cii_displaced_successor_defers_its_skip_vulkan` | S19: publish the `Skip` immediately; S20: idle twice |
| Direct entry invalidates every composed buffer of the affected outputs (§5.5, DMG-5) | `c0_conv_cii_direct_entry_invalidates_composed_vulkan` | S21: drop the invalidation on direct entry |
| No direct milestone applies composed damage (§5.5) | `c0_conv_cii_direct_milestones_leave_composed_damage_vulkan` | S22: apply composed damage at the direct commit's `HardwareComplete` |
| A direct commit never carries an unchanged cursor (§5.6) | `c0_conv_cii_direct_never_carries_an_unchanged_cursor` | S23: carry the unchanged cursor generation |
| A primary flip event does not retire a newer cursor generation (§5.6, C.0 §12; round-1 M-4) | `c0_conv_cii_primary_flip_does_not_retire_a_newer_cursor_vulkan` | S24: retire the live newer generation when the primary commit completes |

---

### Task 1: The Present-carrying owner entry

**Files:** `crates/yserver/src/kms/owner/device.rs`; its tests.

**Interfaces:** produces one public entry — name it — that takes a `CommitDescription` with `page_flip_event` or `present_consumers`, a `CompletionContext`, and Ci's CommitId-aware fallible ledger closure, and returns what Ci's fallible entry returns. It is expressed through the shared `begin` body the Ci-refactor built; no refusal check gains a second copy.

**Invariants (spec §5.0, §4.0):** the entry applies every check `begin_with_context` applies today, including the completion-context validation and the closure/out-fence coverage rules; a closure error leaves the owner exactly as before the call, with the slot released and no live record, and hands the error back by value; `begin_with_ledger` keeps refusing Present-carrying descriptions.

**Named tests:**
- `c0_conv_cii_present_entry_registers_and_refuses` — a Present-carrying description is accepted and its record carries the expected completion CRTCs; and a **structurally valid** description with an independently invalid `CompletionContext` is refused **before the ledger closure runs** (round-1 M-1: a consumer outside the kernel event set is refused by closure construction, `closure.rs:215`, so it cannot witness S1).
- `c0_conv_cii_present_entry_failed_ledger_leaves_nothing`.
- `c0_conv_cii_direct_registration_failure_aborts_the_token` — a **real direct dispatch** whose dependency registration fails after `lock`: the owner is idle, the decider is exactly as before `lock`, the frame and its pins are back with the producer, and the obligations that were registered carry the record's own `CommitId` (round-1 M-1).

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 2: One production eligibility predicate

**Files:** `backend.rs` (the inline inputs around `backend.rs:21339`), the new `direct_owner.rs` if the predicate belongs there, `admission.rs` (the source's `direct_eligible`); tests.

**Invariants (spec §5.1, decision 2):** one function gathers the inputs and returns both the decision and the layout/eligibility generation it was decided under. `try_present_direct` calls it where it computes them today; the conductor's source answers `direct_eligible` with the same function. No second predicate, and no copy of its inputs, remains anywhere. `scanout_direct_eligible`'s pure core keeps its signature and its tests.

**Named tests:** `c0_conv_cii_eligibility_is_one_predicate` (deterministic: for a table of input combinations the function's answer equals `scanout_direct_eligible`'s over the same inputs, and the generation it reports is the conductor's current one), `c0_conv_cii_eligibility_matches_legacy_vulkan` (the same candidate, on the same fixture, answers identically whether the device is `Legacy` or `Owner`).

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 3: The layout hooks, enumerated first

**Files:** wherever the enumeration lands (`backend.rs`, `store.rs`, `scene.rs`, the topology path), `admission.rs`; tests.

**Your first step is the enumeration**, as in plan B2: find every change that can invalidate a queued direct successor's eligibility — at least border width, geometry/configure, storage relayout, redirect changes and topology installation — and report the list before wiring it. The list in this plan is **not** closed.

**Invariants (spec §5.2):** each enumerated change advances the conductor's layout generation through `admission_note_layout_change`; a successor queued as eligible whose ancestor then gains a border never reaches a commit, including when retirement promotes it; the recheck at `lock` refuses a stale eligibility generation before the owner holds anything.

**Named tests:** `c0_conv_cii_layout_hooks_bump_the_generation_vulkan` with one case per enumerated site; `c0_conv_cii_border_invalidates_queued_successor_vulkan`; `c0_conv_cii_border_invalidates_promoted_successor_vulkan`.

- [ ] Steps: enumerate and report; tests; red; implement; checks; stop dirty and report.

---

### Task 4: The direct producer's fork and its module

**Files:** new `crates/yserver/src/kms/render/direct_owner.rs` (register it in `render/mod.rs`), `backend.rs` (`try_present_direct`'s single fork point and the retirement path's), `admission.rs`; tests.

**This task also brings the minimum production interfaces the source needs** (round-1 M-2): the direct `CommitDescription` builder and the direct `CommitResources` the conductor dispatches come from the producer here, not from the injected source, because the spec forbids an exit criterion resting on that source. Tasks 5 and 6 then complete them (members, leases, Present carriage); the tests below are **re-run unchanged** at the end of Task 6 against the finished production path, and Task 6's report says so.

**Invariants (decision 1, spec §5.3):**
- `try_present_direct` keeps the prepare half and asks **once** whether this output's device takes the owner route; the owner half is a call into `direct_owner.rs`. The legacy half — `submit_direct_frame`, `submit_queued_direct_successor` — is untouched, and no owner branch sits inside either.
- On the owner route the prepared frame becomes the latest-wins successor intent (`admission_offer_direct`) and the conductor is woken; nothing is flipped from the producer.
- A displaced successor takes the never-submitted path 2c-ii built: idle exactly once, `Skip` deferred behind the predecessor, pins released by that path.
- Retirement promotion no longer commits from the event handler: the retirement wake admits it through the conductor (C.0 §12).

**Named tests:** `c0_conv_cii_legacy_direct_unchanged_vulkan` (no conductor: the legacy submit runs, with today's pins and `Skip` ordering — name the observable), `c0_conv_cii_owner_direct_offers_instead_of_flipping_vulkan`, `c0_conv_cii_displaced_successor_defers_its_skip_vulkan`, `c0_conv_cii_retirement_promotion_order_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 5: Members and leases from the producer

**Files:** `direct_owner.rs`, `admission.rs` (the direct dispatch's ledger closure and Ci's member guess), `backend.rs`; tests.

**Invariants (decision 3, spec §5.4):** the request builder receives the frame's `GroupMember`s from the producer and Ci's fallback that reads them from the current state is **removed**; the frame's present-pin leases — source pin and fallback-target pin — move **by value** into the commit's `CommitResources`; while the dispatched commit lives, those pins are released by no path other than the ledger (the `PriorBufferReleased` equivalent in the resource service, or the rejection/never-dispatched path).

**Named tests:** `c0_conv_cii_direct_members_come_from_the_producer` (deterministic where possible), `c0_conv_cii_direct_carries_its_pins_vulkan`, `c0_conv_cii_direct_pins_released_only_by_the_ledger_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 6: The direct commit's Present carriage

**Files:** `direct_owner.rs` (the description builder), `admission.rs` (confirmation binds the frame to the `CommitId`; the `Presented` sample), `backend.rs` (`managed_enqueue_retired_direct_completion` as the single publication); tests.

**Invariants (spec §3.3, decision 4):**
- the description sets `page_flip_event` and names in `present_consumers` the **CRTC ids** of the kernel event set, with the `CompletionContext`, through Task 1's entry; a Present serial never appears there;
- confirmation binds the accepted frame to the commit's `CommitId`; the owner's `Presented { samples }` for that id delivers MSC/UST to that frame and completes nothing itself;
- the frame's publication at retirement is the only CompleteNotify/FIFO wake for that request; no other consumer publishes or wakes for it;
- an accepted Present without validated presentation terminalizes as `Skip` with the last validated clock sample, never a fabricated `Flip` timestamp; no idle or release before the ledger proves it.

**Named tests:** `c0_conv_cii_direct_description_carries_present_vulkan`, `c0_conv_cii_direct_present_completes_once_vulkan` (both event orders), `c0_conv_cii_direct_missing_presented_skips_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 7: Composed invalidation and the cursor invariants

**Files:** `direct_owner.rs`, `scene.rs`; tests.

**Invariants (spec §5.5, §5.6):** entering direct invalidates every composed buffer of the affected outputs, exactly as the legacy route does; no milestone of a direct transaction applies composed damage; a direct commit never carries an unchanged cursor generation, and a primary flip event does not retire a newer cursor generation.

**Named tests:** `c0_conv_cii_direct_entry_invalidates_composed_vulkan`, `c0_conv_cii_direct_milestones_leave_composed_damage_vulkan`, `c0_conv_cii_direct_never_carries_an_unchanged_cursor`.

- [ ] Steps: tests; red; implement; checks (Task 7 also runs `cargo check --workspace --target` for `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-unknown-freebsd`); stop dirty and report.

---

## What the coordinator does

After each task: reads the diff against the invariants and the constraints (one fork point, no owner branch inside a legacy function, no assertion weakened), re-runs every check outside codex including the GPU filters, and commits with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)` and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.

After Task 7: applies S1–S24 by line against the implemented code, confirming each compiled and each fails its named test; runs the full hardware gate with the user's go-ahead; writes the acceptance finding and the `docs/status.md` entry. A surviving mutation goes back as a finding unless it is shown equivalent, and the showing is recorded.
