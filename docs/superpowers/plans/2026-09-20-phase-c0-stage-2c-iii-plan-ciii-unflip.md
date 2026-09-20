# Stage 2c-iii, plan Ciii — unflip, multi-device, route selection, hardware

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for hardware work) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters (`c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_ci_`, `c0_conv_cir_`), never `_drm`, `render_acceptance`, unfiltered `--ignored`, or anything that modesets or takes DRM master while the user is looking at the screen; no deletes outside the worktree. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests and the mutations each must catch. Execute tasks in order, one per run. You can run the `_vulkan` tests yourself: nothing is done until its tests pass on the real GPU, in debug and release. Do not ask for approval; a real design choice the plan leaves open, or a claim here that does not hold in the code, is an F8 stop you report.

**Revision 1 (2026-09-20)** — first draft, before codex review.

**Goal:** Finish stage 2c-iii. The owner route gains its third producer — the
unflip back to composed — and with it the return path's composed invalidation
that plan Cii's third F8 stop handed forward. Then the two properties the stage
has claimed but never demonstrated: that one device's conductor is
self-contained, and that on an `Owner` device no primary or unflip legacy write
is issued from any submit site. Last, the copied composed route, whose refusal
is today the only reason a device may not enter `Owner`, and the tty2 hardware
run of spec §6.4 with P3-2/P3-3.

**Architecture:** The unflip is a producer, so it takes the same shape Ci and
Cii took (user's constraint, spec §8.3): its owner-route code lives in **its own
module** — `crates/yserver/src/kms/render/unflip_owner.rs` — reached from **one
fork point**, the `submit_composed_unflip` call site in `maybe_composite`
(`backend.rs:21587`). `submit_composed_unflip` (`backend.rs:3295`) and the
legacy degraded fallback stay exactly where they are. The 2c-i seams this plan
finally drives from production are `managed_handle_direct_unflip`
(`backend.rs:20890`, `#[allow(dead_code)]` today) and
`managed_can_enter_direct`; they are not rewritten unless a task says so.
Production is unchanged (C0-R8): no conductor is installed there, so every
production device stays `Legacy`.

**Spec:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`,
revision 4 with §8.3 as amended: §6.1–§6.4, §3.2, the copied-route row of the
carried-items table (§1) and §8.2's rows for those sections. Read the Cii
acceptance (`../findings/2026-09-20-stage-2c-iii-plan-cii-accepted.md`),
especially "Carried to Ciii" and the three F8 stops, and the Ci acceptance's F8
list before Task 1: F8 4 there is this plan's Task 7.

## Design decisions this plan fixes

1. **The unflip is a producer with its own module and one fork point** (user,
   spec §8.3). `maybe_composite` keeps deciding *that* an unflip is due — the
   requested flag, the "never race a composed commit against the direct
   transaction" gate (`backend.rs:21578`-`21585`) — and then asks once whether
   this output's device takes the owner route. The owner half lives in
   `unflip_owner.rs` and may reuse `composed_commit.rs`'s plane and description
   helpers; the legacy half stays where it is.
2. **Readiness is complete before admission, so the shadow moves ahead of the
   transaction.** Today `submit_composed_unflip` materializes the direct
   shadow inside the submit (`backend.rs:3300`,
   `materialize_direct_shadow_for_unflip` at `backend.rs:3209`) — a GPU render
   and two flushes in the middle of what must become one atomic dispatch. On
   the owner route the materialization happens when the unflip is **requested**,
   through the 2c-i seam `managed_handle_direct_unflip` (its step 4), and
   `Unflip` readiness stays `Waiting` until it has succeeded. The snapshot
   already computes the other two preconditions of spec §6.1 —
   `ExitRetirementOccupied` and `ComposedReturnNotEstablished`
   (`admission.rs:976`-`984`); the shadow is the third and needs its own
   `WaitReason`.
3. **One transaction replacing the complete plane set** (§6.1). The owner
   description covers **every** CRTC of the device with that output's retained
   composed framebuffer (`retained_composed_framebuffer`), never a per-CRTC
   subset: AMD rejects independent per-CRTC replacement with `ENOSPC`, which is
   why `submit_composed_unflip` exists in the shape it has. The grouping
   precondition is the one the legacy path checks first,
   `direct_scanout_topology_eligible` (`backend.rs:3457`).
4. **No degraded fallback on an `Owner` device.** When the atomic unflip fails,
   Legacy degrades to per-output composed scene flips (`backend.rs:21588`-
   `21600`). Those are `WriterClass::Primary` legacy writes, and §6.3 forbids
   them on an `Owner` device. A failed owner unflip dispatch therefore takes
   the Ci-refactor's **shared dispatch-failure path** (its decision 4) and
   fails closed. This is a stated door, not an oversight: recovering a device
   whose transport closed while it scans out a client buffer is stage 4's
   lifecycle work, and C0-R8 keeps it out of production.
5. **The return path belongs to the unflip commit's retirement** (Cii F8 3,
   DMG-5). On the legacy path the return effects hang off counting per-output
   retirements (`retire_direct_output`, `backend.rs:3355`-`3378`): stop direct,
   block re-entry until composed, `invalidate_all_scanout_damage`, mark the
   scene structure dirty. On the owner route the **unflip commit's own
   retirement** runs them, once, and the retired commit is recognised as the
   unflip from what its dispatch recorded — never by re-deriving it from scene
   state, and never by a second counter. Until that retirement no composed
   buffer of an affected output may be scanned out again without a full
   repaint; the fixture proves it **per output** (spec §6.1, round-1 M-3).
6. **Route selection is read per site, per device** (§6.3). Every submit site
   of §4.1, §5.3 and §6.1 reads the transport state of **its own** device
   before it writes. Each site carries its own mutation ("force the legacy
   branch"), and the mutation must break a named test **by an observation other
   than the transport gate's refusal** — the gate is the defence the fork is
   supposed not to need. The enumerated sites are fixed in Task 5 and the
   enumeration itself is the scope boundary.
7. **Multi-device state is already per device; what Ciii owes is the
   evidence** (§6.2). `admission_conductors` is a `BTreeMap<DrmDeviceKey,
   AdmissionConductor>` and each conductor carries its own admission, layout
   generation, composed intents, maintenance store and receipts
   (`admission.rs:266`-`286`); the transport gate and the owner are per device
   in the platform. The device-blind helpers are known and are not all
   defects: `admission_note_layout_change_all_devices` (`admission.rs:793`) is
   device-blind **by design** and `scanout_m2` is one direct group for the
   whole backend. A device-blind path that changes **another** device's
   conductor, gate, owner or damage state is a defect, fixed in the task that
   finds it; a device-blind path that is deliberate is recorded in the task's
   report, not rewritten.
8. **The copied route's fixture comes first** (Ci F8 4). No fixture in this
   checkout builds a copied output, so `OwnerEligibilityError::CopiedScanoutRoute`
   (`platform.rs:98`) has only validator-level evidence. Task 7 builds the
   fixture **before** any conversion code. If a copied scanout pool cannot be
   built on this box, that is an F8 stop reported at that point — the
   conversion is handed back to the coordinator, never substituted by a stub, a
   hand-built pool or a validator-only test.
9. **Test names start with `c0_conv_ciii_`**, so one filter selects this plan.
   `_vulkan` tests carry `#[ignore = "needs live Vulkan ICD"]` and build on
   Ci's owner-route live fixture (`for_tests_with_vk_live_scene_real_drm`,
   `backend.rs:6843`; the plain variant has no real DRM node). That fixture
   asserts **exactly one** KMS device (`backend.rs:6898`), so the multi-device
   evidence of Task 6 is not built on it; `test_kms_device` plus
   `install_transport_gate` is the shape the platform's own multi-device tests
   already use (`platform.rs:8798`, `platform.rs:9797`).

## Limits stated

- Cursor and gamma **producers**, the cursor coordinate lane, topology and
  cursor-recovery dispatch: stages 3 and 4. `Topology` and `CursorRecovery`
  stay `Unsupported` here.
- C.0 §16.3 revision 5 is owed before stages 3/4, not here.
- The Ci F8 stops 1-3 stay open and are not re-litigated: the Legacy dormancy
  bug, the missing restore `TerminalState`, device loss without an owner
  signal.
- Recovery from a transport closed by a failed dispatch is stage 4's
  (decision 4).

## Global Constraints

- **Production is byte-for-byte unchanged**: without an active conductor the
  unflip, composed, direct and promotion paths take today's calls with the same
  arguments, the same pins and the same ordering. Each task keeps a named
  Legacy characterisation test green.
- Owner milestones reach the scene only through `route_owner_event_batch`;
  tests deliver them that way (a stub behaviour or crafted events handed to it
  — say which), never by calling a handler directly.
- No hand-built `PendingAck`, `BoPhase`, `OwnerBuffer`, `DirectPresentFrame`,
  scanout pool or prepared generation in any test.
- Resources travel by value; nothing is bare-dropped; every token is consumed
  exactly once; confirmation is at the send; no retry on refusal.
- No side effect inside `debug_assert!`; fail closed, never panic, in non-test
  code; no test-only hook that bypasses the path it is named after.
- **Honesty rule (F8).** An unreachable scenario, a seam that does not behave
  as stated, a fixture that cannot carry what a test needs, or a real design
  choice left open: stop and report.

## Checks every task must keep green

```bash
cargo build -p yserver --bin yserver
cargo build --release -p yserver --bin yserver
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features tcp-transport -- -D warnings
cargo clippy --all-targets --features xdmcp -- -D warnings
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_conv_ciii_; done
cargo test -p yserver --lib c0_conv_ciii_ -- --include-ignored --test-threads=1
cargo test --release -p yserver --lib c0_conv_ciii_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_cii_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_ci_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_cir_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

Baseline before Task 1 (commit `5a34c6ec`): `c0_conv_ci_` 38/38 and
`c0_conv_cii_` 26/26 with `--include-ignored` in debug and release;
`c0_conv_cir_` 7/7; `c0_adm` 129/0; `c0_2ci` 180/0/21; `--lib` 1933/0/142.
Every task ends at those numbers plus its own new tests. The hardware gate
(306/306 at `5a34c6ec`) and the §6.4 run are the coordinator's, after the last
task, with the user's go-ahead.

## Exit criteria

| Criterion (spec) | Tests | Mutation that must fail them |
| --- | --- | --- |
| An unflip is not admitted until the exit-retirement position is free, every affected output has its retained composed framebuffer **and** the direct shadow is materialized (§6.1) | `c0_conv_ciii_unflip_readiness_waits_on_each_precondition` (one case per precondition, each with the others satisfied) | T1: report `Ready` while the shadow is unmaterialized; T2: report `Ready` while the exit-retirement position is occupied |
| The request path materializes the shadow and moves `Current` into `ExitRetirement` through the 2c-i seam (§6.1, 2c-i §8.5) | `c0_conv_ciii_unflip_request_drives_the_managed_seam_vulkan` | T3: request without the seam, as today's `admission_request_unflip` does |
| A failed materialization leaves the unflip requested and unadmitted, and is retried on the next wake, never dispatched (§6.1) | `c0_conv_ciii_unflip_shadow_failure_defers_admission_vulkan` | T4: treat a failed materialization as ready |
| `Unflip` is dispatched; `Topology` and `CursorRecovery` stay `Unsupported` (§6.1) | `c0_conv_ciii_unflip_dispatches_and_others_stay_unsupported` | T5: keep `Unflip` in `decision_requires_unsupported`; T6: drop `Topology` from it as well |
| The unflip commit replaces the **complete** plane set of its device in one transaction, each CRTC with that output's retained composed framebuffer (§6.1) | `c0_conv_ciii_unflip_commit_covers_every_crtc_vulkan` | T7: describe only the CRTCs named in the barrier; T8: skip the topology-eligibility precondition |
| The owner route takes its own module behind one fork point; Legacy is unchanged (decision 1) | `c0_conv_ciii_legacy_unflip_unchanged_vulkan`, `c0_conv_ciii_owner_unflip_commits_instead_of_submitting_vulkan` | T9: force `submit_composed_unflip` under `Owner`; T10: take the owner route under `Legacy` |
| A failed owner unflip dispatch fails closed and never degrades to per-output legacy flips (decision 4) | `c0_conv_ciii_unflip_dispatch_failure_fails_closed_vulkan` | T11: fall through into the degraded per-output path under `Owner` |
| The unflip commit's retirement stops direct scanout, blocks re-entry until composed, and invalidates every affected output's composed buffers exactly once (§6.1, DMG-5, Cii F8 3) | `c0_conv_ciii_unflip_retirement_returns_to_composed_vulkan` | T12: drop the invalidation on the owner return path; T13: run the return effects at `HardwareComplete` instead of `CompletionRetired` |
| Every affected output is repainted in full before it is scanned out again; **per output** (§6.1, DMG-5, round-1 M-3) | `c0_conv_ciii_unflip_return_repaints_each_output_in_full_vulkan` (at least two outputs, distinct damage before the direct entry) | T14: invalidate only the reference output; T15: apply the pre-entry damage to the returning composed buffer |
| The retired unflip is recognised from what its dispatch recorded, not from scene state (decision 5) | `c0_conv_ciii_unflip_retirement_returns_to_composed_vulkan`, driven with an ordinary composed commit retiring first | T16: run the return effects for any retiring composed commit while an unflip is requested |
| The unflip does not drop or flash the cursor and preserves the current gamma (§6.1, C.0 §12) | `c0_conv_ciii_unflip_keeps_cursor_and_gamma_vulkan` | T17: omit the cursor plane state from the unflip description; T18: write a default gamma with the unflip |
| On an `Owner` device no primary or unflip legacy write is issued, at any enumerated site (§6.3) | the enumeration of Task 5, each site with its own case in `c0_conv_ciii_owner_device_issues_no_legacy_primary_write_vulkan`, **observed at the site, not at the gate** | T19-T22: force the legacy branch at each enumerated site in turn |
| On a `Legacy` device behaviour is identical to today (§6.3) | `c0_conv_ciii_legacy_device_unchanged_vulkan` plus the full software gate | T23: read another device's transport state at one enumerated site |
| An event, wake, refusal or bound violation on one device changes nothing on another (§6.2) | `c0_conv_ciii_devices_are_independent` (one case per kind: owner event batch, admission wake, refused offer, bound violation closing a gate) | T24: route the batch to every conductor; T25: close every device's gate on a bound violation |
| A device's layout generation, composed intents, maintenance and receipts belong to that device alone (§6.2) | `c0_conv_ciii_conductor_state_is_per_device` | T26: bump every conductor's layout generation on one device's change |
| A grouped direct unit never crosses devices (§6.2) | `c0_conv_ciii_direct_group_never_crosses_devices_vulkan` | T27: drop the single-device precondition from `direct_scanout_topology_eligible` |
| A device whose outputs are on the copied route enters `Owner` and commits through it (§6.3, carried-items table) | `c0_conv_ciii_copied_output_enters_owner_vulkan`, `c0_conv_ciii_copied_route_commits_through_the_owner_vulkan` | T28: refuse `Copied` in `check_owner_eligibility`; T29: flip the sink copy under `Owner` through the legacy site |
| The copied route's flip still follows its copy into the destination buffer (§6.3) | `c0_conv_ciii_copied_route_commits_through_the_owner_vulkan` | T30: commit before the sink copy's completion is observed |

---

### Task 1: Unflip readiness, completed

**Files:** `crates/yserver/src/kms/render/admission.rs` (`admission_snapshot`,
`admission_request_unflip`), `crates/yserver/src/kms/owner/admission/snapshot.rs`
(the new `WaitReason`), `backend.rs` only where the seam is called; tests.

**Interfaces:** `admission_request_unflip` (`admission.rs:713`) drives the 2c-i
seam `managed_handle_direct_unflip` (`backend.rs:20890`) instead of calling
`request_direct_unflip` alone, keeping its current effects — the barrier, the
terminalization of the exact displaced queued frame, the deferred-skip
publication — in the order they have today. `Unflip` readiness gains its third
precondition, with a `WaitReason` of its own.

**Invariants (spec §6.1):** an unflip is admitted only when the
exit-retirement position is free, every affected output has its retained
composed framebuffer, **and** the direct shadow is materialized; a failed
materialization leaves the unflip requested, unadmitted and retryable on the
next wake, and never fails the request; the seam's move of `Current` into a
freshly reserved `ExitRetirement` happens even when `OrdinaryRetirement` is
occupied (2c-i §8.5 item 3), and a failure to reserve it leaves capacity
exactly as before.

**Named tests:**
- `c0_conv_ciii_unflip_readiness_waits_on_each_precondition` — three cases,
  each with the other two satisfied, each naming its own `WaitReason`.
- `c0_conv_ciii_unflip_request_drives_the_managed_seam_vulkan` — after the
  request: the shadow is materialized, `Current` is in `ExitRetirement`, the
  queued successor is terminalized with its charge discharged, and
  `reentry_blocked_until_composed` reflects `managed_can_enter_direct`.
- `c0_conv_ciii_unflip_shadow_failure_defers_admission_vulkan` — the request
  succeeds, nothing is admitted, and a later wake with the shadow available
  admits it.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 2: Unflip dispatch, in its own module

**Files:** new `crates/yserver/src/kms/render/unflip_owner.rs`; `admission.rs`
(`decision_requires_unsupported` at `:181`, `admission_dispatch_decision` at
`:1077`); `backend.rs` at the one fork point (`:21587`); tests.

**Interfaces:** `unflip_owner` produces the device's unflip `CommitDescription`
and its `CommitResources`, and is reached from `admission_dispatch_unflip`,
which is reached from `admission_dispatch_decision`. `Unflip` leaves
`decision_requires_unsupported`; `Topology` and `CursorRecovery` stay. The
failure handling is the Ci-refactor's shared mechanism, with the unflip's own
row; nothing new is added to the policy table beyond what the route needs.

**Invariants (spec §6.1, decisions 3 and 4):** the description covers every
CRTC of the device, each with that output's retained composed framebuffer,
in one transaction; `direct_scanout_topology_eligible` is a precondition of the
dispatch, not an assumption; the resources moved into the ledger are the
exit-retirement state the seam prepared, by value; a dispatch failure fails
closed through the shared path and never reaches the degraded per-output
fallback; production's `submit_composed_unflip` and its degraded fallback are
untouched, including the `scanout_m2` bookkeeping they do
(`unflip_awaiting_outputs`, `degraded_composed_unflip`).

**Named tests:**
- `c0_conv_ciii_unflip_dispatches_and_others_stay_unsupported`.
- `c0_conv_ciii_unflip_commit_covers_every_crtc_vulkan` — two outputs, each
  named plane carrying that output's retained composed framebuffer.
- `c0_conv_ciii_legacy_unflip_unchanged_vulkan` and
  `c0_conv_ciii_owner_unflip_commits_instead_of_submitting_vulkan`.
- `c0_conv_ciii_unflip_dispatch_failure_fails_closed_vulkan` — the transport
  closes, no legacy write is issued, and the direct frame's pins are still
  held.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 3: The return path

**Files:** `unflip_owner.rs`, `admission.rs` (the retirement routing),
`backend.rs` (the return effects, beside `retire_direct_output`'s legacy
counterpart at `:3355`-`:3378`); tests.

**Interfaces:** the unflip commit's `CompletionRetired` runs the return
effects once: stop direct scanout, block re-entry until composed, invalidate
every affected output's composed buffers, mark the scene structure dirty. The
commit is recognised as the unflip from state its dispatch recorded.

**Invariants (spec §6.1, DMG-5, Cii F8 3):** the invalidation happens on the
**return**, at the unflip commit's retirement, and exactly once; every affected
output is invalidated, not only the reference one; each affected output is
repainted **in full** before it is scanned out again, and no damage recorded
before or during the direct interval survives into that repaint; an ordinary
composed commit retiring while an unflip is outstanding runs none of these
effects; direct re-entry stays blocked until a composed frame has been
presented on every affected output (`managed_can_enter_direct`).

**Named tests:**
- `c0_conv_ciii_unflip_retirement_returns_to_composed_vulkan` — also drives an
  ordinary composed retirement first (mutation T16).
- `c0_conv_ciii_unflip_return_repaints_each_output_in_full_vulkan` — at least
  two outputs, with distinct damage recorded on each before the direct entry;
  the assertion is per output.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 4: Cursor and gamma across the unflip

**Files:** `unflip_owner.rs`; tests.

**Interfaces:** none new. What the task adds is the description's carriage of
the cursor plane state the device already has, and the absence of any gamma
payload.

**Invariants (spec §6.1, C.0 §12):** the unflip transaction does not drop or
flash the cursor — the cursor plane keeps the state it had before the
transaction, with no intervening disable — and it carries no gamma payload, so
the current gamma survives it. This task adds **no** cursor or gamma producer:
those are stage 4's.

**Named tests:**
- `c0_conv_ciii_unflip_keeps_cursor_and_gamma_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 5: Route selection and exclusivity

**Files:** the enumerated submit sites; tests.

**Interfaces:** none new. The task's first product is **the enumeration
itself**, written into the plan's report: every site that issues a primary or
unflip legacy write, with its file, line and `WriterClass`. The enumeration is
this criterion's scope boundary — a site missing from it is a hole, so the
enumeration is derived from the `allows_legacy` call sites of
`WriterClass::Primary` and `WriterClass::Unflip` and from the submit sites of
§4.1, §5.3 and §6.1, not from memory. As measured at `5a34c6ec` those are the
composed scene flip (`scene.rs:6330`), the direct submit and the retirement
promotion that shares it (`backend.rs:2695`, reached from
`submit_queued_direct_successor` at `:2709`), the composed unflip
(`backend.rs:3327`) and the copied scanout flip (`platform.rs:6400`, Task 7's).

**Invariants (spec §6.3, decision 6):** each site reads the transport state of
**its own** device before writing; on an `Owner` device no primary or unflip
legacy write is issued from any of them; on a `Legacy` device the behaviour is
identical to today. Each site's mutation forces its legacy branch, and the test
that fails must observe **the write that happened** — the commit that did not
reach the owner, the plane that changed outside a transaction — and **not** the
transport gate's refusal.

**Named tests:**
- `c0_conv_ciii_owner_device_issues_no_legacy_primary_write_vulkan` — one case
  per enumerated site.
- `c0_conv_ciii_legacy_device_unchanged_vulkan`.

- [ ] Steps: enumerate and report the sites; tests; red; implement; checks;
  stop dirty and report.

---

### Task 6: Multi-device

**Files:** `admission.rs`, `backend.rs` and `platform.rs` where a device-blind
path is found; tests.

**Interfaces:** none new unless a defect is found. The fixture is **not** the
live single-device one (decision 9): two seeded KMS devices with their own
gates, the shape `platform.rs:8798` and `platform.rs:9797` already use.

**Invariants (spec §6.2):** one conductor per `DrmDeviceKey`, each with its own
admission, layout generation and transport state; an event, wake, refusal or
bound violation on one device changes nothing on another — not its layout
generation, not its composed intents, not its maintenance store, not its
receipts, not its gate, not its owner; a grouped direct unit never crosses
devices. Device-blind paths that are deliberate (decision 7) are named in the
report rather than rewritten; a device-blind path that changes another device's
state is a defect, fixed here.

**Named tests:**
- `c0_conv_ciii_devices_are_independent` — four cases: an owner event batch, an
  admission wake, a refused offer, and a bound violation that closes a gate.
- `c0_conv_ciii_conductor_state_is_per_device`.
- `c0_conv_ciii_direct_group_never_crosses_devices_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 7: The copied composed route

**Files:** `platform.rs` (`check_owner_eligibility` at `:3411`,
`validate_owner_output_kinds` at `:90`, `output_uses_owner_route` at `:3443`,
`submit_copied_scanout` at `:6381`, whose legacy write gate is at `:6400`), a
module for the owner half; tests.

**Step 0, before any conversion code: the fixture.** Ci's F8 4 records that no
fixture in this checkout builds a copied output, so the refusal has only
validator-level evidence. Build one that allocates a real copied scanout pool
on this box — `allocate_copied_scanout_pool` is reachable directly with the
live `VkContext` as both source and sink renderer, which is the shape the
production fallback uses when the copy-free candidates fail
(`platform.rs:2932`-`2976`) — and prove it is a copied pool by driving one
legacy copied frame through `submit_copied_scanout` before any owner code
exists. **If that pool cannot be allocated on this box, stop: F8.** Do not
hand-build a pool, do not stub the copy, and do not fall back to a
validator-only test.

**Interfaces:** the copied route's owner half lives in its own module behind
one fork point in `submit_copied_scanout`, exactly as Ci and Cii did. The
sink copy stays where it is; what forks is the flip.

**Invariants (spec §6.3, carried-items table):** a device whose outputs are on
the copied route may enter `Owner`, and its composed commits go through the
owner; the flip still follows the sink copy's completion — the owner commit is
dispatched only once the copy the frame depends on has completed, never
before; the destination buffer is the commit's new state and the displaced one
its old state, with the same obligations any other composed commit registers;
an unmanaged scanout pool is still refused.

**Named tests:**
- `c0_conv_ciii_copied_output_enters_owner_vulkan`.
- `c0_conv_ciii_copied_route_commits_through_the_owner_vulkan`.
- the Legacy characterisation test of step 0, kept green.

- [ ] Steps: fixture (or F8); tests; red; implement; checks; stop dirty and
  report.

---

## What the coordinator does

After each task: read the diff, re-run the checks above outside codex, apply
this plan's mutations **by line** against the recorded mutation text, confirm
each is caught by its named test, then commit. `_vulkan` tests and their
mutations run on the GPU with the user's go-ahead, from tty when the user asks
for it.

After Task 7, with the user's go-ahead and the GPU free: the full hardware gate
(`render_acceptance -- --ignored`, `c0_2ci -- --ignored`, the library's other
ignored tests — 306/306 at `5a34c6ec`, plus this plan's new `_vulkan` tests),
then the §6.4 run below, then the acceptance finding and `docs/status.md`.

### The tty2 hardware run (spec §6.4)

From tty2 on card1, after asking the user (the GPU is in personal use). A
fixture establishes `Owner` with the writer-coverage evidence of the debt spec
§4.4, then drives, with the real conductor, helper and producers:

1. a composed frame: `Accepted` → `HardwareComplete` → damage applied;
2. a direct frame from a Vulkan-rendered PRIME-imported buffer: out-fence
   `Success`, leases released at `PriorBufferReleased`;
3. the unflip back to composed;
4. a commit whose new state **retains** an allocation of the old state for the
   same member — a direct Present of the same source buffer again. The retained
   allocation gets no `KmsRelease` obligation and is not released, while any
   displaced allocation in the same commit is. If no such commit is reachable
   on card1, that is an F8 stop, not a substitution by a fixture.

**P3-2** (a displaced buffer's `KmsRelease` is discharged by the real
completion — steps 1-3) and **P3-3** (a retained buffer registers none —
step 4) ride on this run, each with its own mutation run on the hardware under
the same filter: dropping the displaced buffer's registration must fail P3-2,
and registering the retained allocation must fail P3-3.

**Reachability recheck, run by the plan's author on the implemented Ci/Cii code
(2026-09-20, required by spec §6.4 before these are anchored):** `register_kms`
(`resources/mod.rs:657`) has one caller, `register_commit_dependencies`
(`resources/commit.rs:627`, registering exactly the old-state allocations not
retained by the new state for the same member, `:668`), and that function has
**three non-test callers**, all on the paths this run drives:
`admission.rs:1280` and `:1338` (primary dispatch) and `admission.rs:1533`
(direct dispatch). The condition of spec §3.2 holds; P3-2 and P3-3 are
anchored here and close the debt spec's §9.5 F8.
