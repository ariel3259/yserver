# Stage 2c-iii addendum — direct entry waits for a composed return

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for GPU work) with `< /dev/null`. Hard rules: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only the filters `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm` with `--include-ignored`, never `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the invariant, the named tests with the scenario each must exercise, and the mutations each must catch. Stop with the tree dirty when done. **Do not ask for approval inside a run** — if something this plan states does not hold in the code, stop and report it (F8); never silently substitute a test shape or weaken an existing assertion.

**Revision 2 (2026-09-23)** — incorporates codex round 1
(`../findings/2026-09-23-ciii-addendum-plan-review-round1.md`: 1 blocking,
2 major, all verified): **B-1** `has_current_direct` (`admission.rs:2368`)
searches every current resource with no device filter — a device-blind site
of the family plan Ciii-identity closed with `CommitKey` — so it becomes
device-scoped (I-0); **M-1** a waiting direct entry requests the composed frame
it waits for (I-4), so a device with no composed work cannot block direct
forever; **M-2** A3 requires the intermediate state.

**Revision 1 (2026-09-23, coordinator).** One task. The user chose to fix this as its own addendum before the stage 3a plan.

**Goal:** On an Owner device, a direct unit can no longer become current on a device whose outputs have no established composed return, so the Ciii unflip always has something to return to.

**Why:** `docs/superpowers/findings/2026-09-23-ciii-direct-entry-without-composed-return.md` — read it first. The unflip is ready only with a composed return established (`crates/yserver/src/kms/render/admission.rs:1007`), but direct *entry* checks neither in its readiness (`admission.rs:950`–`970`) nor in its eligibility (`direct_present_eligibility_decision`, `backend.rs:833`); only the 8-Present entry probation makes the gap unlikely. It is ours (Ciii code, accepted 2026-09-22) and is fixed as a named stage 2c-iii addendum, in its own commit.

**Authority:** C.0 (`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`) §12 and the 2c-iii conversion design §6.1 (`docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`: the unflip "needs a retained composed framebuffer"); the stage 3a design §6 (`docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`), which depends on this.

## The invariants

**I-0. "Direct is current" is a per-device fact** *(rev 2, B-1)*. The check
that today answers "is any direct unit current?" across every device
(`has_current_direct`, `admission.rs:2368`, which scans all of
`commit_consumer.current_resources`) answers it for **the device being
decided**, using the device the resources carry (their `CommitKey`). Every
existing caller is converted — including the `ordinary_retirement` use at
`admission.rs:962`, which has the same device-blindness — and no device-blind
variant remains.

**I-1. Direct entry needs a way back.** When no direct unit is current **on this device** (I-0), a queued direct intent is reported `Waiting(WaitReason::ComposedReturnNotEstablished)` unless the device's composed return is established — the same `composed_return_established` predicate the unflip already uses (`admission.rs:844`, every output of the device). The reason is reported **before** producer readiness is consulted for a direct entry, so an eligible, producer-ready entry still waits.

**I-2. A successor is unaffected.** When a direct unit is already current, the direct successor's readiness is exactly what it is today; the return was established at entry and is not lost while direct is current (finding, case (b)).

**I-4. A waiting entry asks for its return** *(rev 2, M-1)*. When a direct
entry is reported `ComposedReturnNotEstablished`, the device's outputs are
marked for a full composed frame — the same scene invalidation Ciii uses when
it returns to composed — **once per waiting entry generation**, not on every
wake. The composed frame then goes through ordinary composed admission (no
direct unit is current, so it is admissible), retires, establishes the return,
and the next wake admits the entry. The wait is therefore bounded by one
composed frame, and a device that only ever receives direct work still gets
its return.

**I-3. Nothing else changes.** Composed, unflip, cursor-recovery and maintenance readiness are untouched; Legacy (no conductor) is byte-for-byte unchanged; the entry probation is untouched.

## Task 1 — the gate, its tests, and the existing fixtures

**Step 1 — inventory before editing.** List every existing test that offers a direct **entry** on an Owner conductor (the `c0_conv_cii_*`, `c0_conv_ciii_*`, `c0_conv_cfb_*`, `c0_conv_cp_*` and `c0_adm*` families) and, for each, whether its fixture establishes a composed return before the entry. Report the list in your final message.

**Step 2 — implement I-0, I-1, I-2 and I-4**: the per-device current-direct check and every caller of the device-blind one; the gate at the direct readiness report in `admission_snapshot`; the once-per-generation request for a full composed frame.

**Step 3 — fixtures.** Every inventoried test whose entry now waits because its fixture had no composed return gets a fixture that **establishes one through production entries** (a composed frame dispatched and retired through `route_owner_event_batch`, or the existing helper `c0_conv_ciii_mark_retained_composed` where it already does that) — never a weakened assertion, never a bypass of the gate. If a test cannot establish a return through production entries, that is an F8: stop and report it.

**Step 4 — new tests** (names are exit criteria):

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_conv_ciii_add_direct_entry_waits_without_a_composed_return` | Owner conductor (no Vulkan needed, like `c0_adm_conductor_unflip_waits_without_a_composed_return`), no composed return, an eligible, producer-ready direct entry queued: the snapshot reports `Waiting(ComposedReturnNotEstablished)` and a wake admits nothing | **A1** |
| `c0_conv_ciii_add_direct_successor_ignores_the_return_gate` | a direct unit current, a successor queued, the composed return absent: the successor's readiness is what it is without the gate | **A2** |
| `c0_conv_ciii_add_direct_elsewhere_does_not_exempt_an_entry` | two Owner conductors (two devices, as the two-seeded-device fixture of plan Ciii-identity): device A has a current direct unit; device B, with no composed return, has an eligible, ready direct entry: B reports `Waiting(ComposedReturnNotEstablished)` | **A4** |
| `c0_conv_ciii_add_waiting_entry_requests_its_return_vulkan` | Owner live fixture, **no composed intent pending and no damage**: a direct entry is offered; the wake reports the wait and marks the outputs for a full composed frame exactly once (a second wake does not mark again); the composed frame is dispatched and retires; the entry is admitted | **A5**, **A6** |
| `c0_conv_ciii_add_direct_entry_admitted_once_the_return_exists_vulkan` | Owner live fixture (`for_tests_with_vk_live_scene_real_drm` with a managed pool, identity from `vk.selected_drm_identity.primary`): a direct entry offered before any composed frame retired waits; a composed frame is dispatched and retires through `route_owner_event_batch`; the same entry is then admitted and dispatched | **A1**, **A3** |

**Mutations** (apply by line, confirm the run compiled, restore; ledger in your report):

- **A1** — remove the I-1 check: `direct_entry_waits_without_a_composed_return` and `direct_entry_admitted_once_the_return_exists_vulkan` fail.
- **A2** — apply the check to successors too (drop the `has_current_direct` condition): the second test fails.
- **A3** — evaluate the gate against the *first* output only instead of every
  output of the device. The third test proves A3 only through the
  **intermediate state** *(rev 2, M-2)*: on a two-output device (the
  multi-output live fixture near `backend.rs:53826`), the first output's
  return established through a retired composed frame, the second's absent —
  the entry must still wait; then the second is established and the entry is
  admitted. If production composed entries cannot produce that intermediate
  state, report A3 as **unproved** with the reason — never as passed, and never
  with a fake device.
- **A4** — restore the device-blind `has_current_direct`: the
  `direct_elsewhere` test fails.
- **A5** — remove the I-4 request: the `requests_its_return` test times out
  waiting for a composed frame and fails.
- **A6** — request the frame on every wake instead of once per entry
  generation: the `requests_its_return` test's "second wake does not mark
  again" assertion fails.

**Gate:** `cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings` in default, `--features tcp-transport` and `--features xdmcp`; `cargo test -p yserver --lib c0_conv_ -- --include-ignored` and `c0_adm` (debug); `cargo test -p yserver --lib` (debug). Report exact counts.

## Limits

Fixture level; production stays `Legacy` (C0-R8). No hardware run: the Ciii/Cp card1 runs are not repeated for this addendum; stage 3a's hardware run exercises direct entry again.
