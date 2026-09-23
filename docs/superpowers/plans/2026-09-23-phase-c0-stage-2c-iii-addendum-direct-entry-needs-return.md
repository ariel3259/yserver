# Stage 2c-iii addendum — direct entry waits for a composed return

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for GPU work) with `< /dev/null`. Hard rules: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only the filters `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm` with `--include-ignored`, never `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the invariant, the named tests with the scenario each must exercise, and the mutations each must catch. Stop with the tree dirty when done. **Do not ask for approval inside a run** — if something this plan states does not hold in the code, stop and report it (F8); never silently substitute a test shape or weaken an existing assertion.

**Revision 3 (2026-09-23) — rewritten, not patched.** Codex round 2
(`../findings/2026-09-23-ciii-addendum-plan-review-round2.md`: 1 blocking,
3 major, all verified) showed every revision-2 change traded: a gate in
**readiness** acts on a direct frame that is already prepared, and a prepared
direct frame sets `hold_direct` (`backend.rs:21487`), which stops the tick
before the scene composes (`backend.rs:22529`) — so the composed frame the
entry waits for is never produced (B-1); a per-generation bootstrap request
restarts on every replaced frame (M-1); and scoping `has_current_direct` per
device removed the guard on a single, global retirement-capacity role (M-2).
Revision 3 moves the gate to **eligibility**, decided per Present before any
direct frame is prepared: without a composed return the Present is simply not
direct-eligible, it takes the composed path, and that composition establishes
the return. No hold, no bootstrap request, no generation counting, and
`has_current_direct` is left as it is (see "Recorded, not changed").

**Revision 2 (2026-09-23)** — round 1's B-1/M-1/M-2 answered with I-0 (per-
device current-direct), I-4 (bootstrap request) and an intermediate state for
A3; superseded by revision 3.

**Revision 1 (2026-09-23, coordinator).** One task. The user chose to fix this
as its own addendum before the stage 3a plan.

**Goal:** On an Owner device, a direct unit can no longer become current on a
device whose outputs have no established composed return, so the Ciii unflip
always has something to return to.

**Why:** `docs/superpowers/findings/2026-09-23-ciii-direct-entry-without-composed-return.md`
— read it first. The unflip is ready only with a composed return established
(`crates/yserver/src/kms/render/admission.rs:1007`), but nothing required one
at direct entry; only the 8-Present entry probation made the gap unlikely. It
is ours (Ciii code, accepted 2026-09-22) and is fixed as a named stage 2c-iii
addendum, in its own commit.

**Authority:** C.0 (`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`)
§12 and the 2c-iii conversion design §6.1
(`docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`:
the unflip "needs a retained composed framebuffer"); the stage 3a design §6
(`docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`),
which depends on this.

## The invariants

**I-1. No composed return, no direct entry.** On a device with an active
admission conductor (Owner) and **no direct unit current**, a Present is
direct-eligible only if the device's composed return is established — the
predicate the unflip already uses (`admission.rs:844`, every output of the
device), extracted so that both callers use the one function. The check is
part of the shared eligibility predicate `direct_present_eligibility`
(`backend.rs:3918`), so the candidate is refused before any direct frame is
prepared, `hold_direct` is never set for it, and the Present takes the
composed path like any other ineligible Present.

**I-2. The composed path establishes the return by itself.** A refused Present
is composed and flipped through ordinary composed admission; when that
composed frame retires, the return is established and a later eligible Present
may enter direct (after the unchanged entry probation). No explicit request,
counter or bootstrap state exists.

**I-3. Only entries, only Owner.** A direct **successor** (a direct unit
already current) is unaffected: its return was established at entry and is
not lost while direct is current (finding, case (b)). Legacy — no conductor —
is byte-for-byte unchanged; its unflip (`submit_composed_unflip`) does not use
this predicate. Composed, unflip, cursor-recovery and maintenance readiness,
the entry probation and `has_current_direct` are untouched.

## Recorded, not changed

`has_current_direct` (`admission.rs:2368`) scans every device's current
resources (round-1 B-1). It is **latent, not reachable today**: direct is
eligible only when every output is on the primary device
(`direct_scanout_topology_eligible`, `backend.rs:3877`), so at most one device
can ever hold or enter direct. It also guards `ordinary_retirement`, a single
capacity role shared by every device (`resources/capacity.rs:188`), which a
naive per-device scoping would break (round-2 M-2). It must be revisited by
whichever stage first makes direct scanout multi-device; this addendum records
it in the finding and does not touch it.

## Task 1 — the gate and its tests

**Step 1 — inventory before editing.** List every existing test that drives a
direct **entry** on an Owner conductor (the `c0_conv_cii_*`, `c0_conv_ciii_*`,
`c0_conv_cfb_*`, `c0_conv_cp_*` and `c0_adm*` families) and, for each,
whether its fixture establishes a composed return before the entry. Report the
list in your final message.

**Step 2 — implement I-1** (extract the predicate; add the check to the shared
eligibility decision, applied only with an active conductor and no current
direct unit).

**Step 3 — fixtures.** Every inventoried test whose entry is now refused
because its fixture had no composed return gets a fixture that **establishes
one through production entries** (a composed frame dispatched and retired
through `route_owner_event_batch`, or `c0_conv_ciii_mark_retained_composed`
where it already does exactly that) — never a weakened assertion, never a
bypass of the check. A test that cannot establish a return through production
entries is an F8: stop and report it.

**Step 4 — new tests** (names are exit criteria):

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_conv_ciii_add_entry_refused_without_a_composed_return` | Owner conductor, no composed return, no direct current: a candidate that passes every other eligibility input is refused by `direct_present_eligibility`, and no direct frame is prepared | **A1** |
| `c0_conv_ciii_add_successor_eligibility_ignores_the_return` | a direct unit current, a successor candidate, the return check forced to report "absent" through the extracted predicate's test seam: the successor's eligibility is unchanged | **A2** |
| `c0_conv_ciii_add_legacy_eligibility_unchanged` | a device with no conductor: eligibility for the same candidate is identical with and without a composed return | **A4** |
| `c0_conv_ciii_add_refused_present_composes_then_enters_vulkan` | Owner live fixture (`for_tests_with_vk_live_scene_real_drm` with a managed pool, identity from `vk.selected_drm_identity.primary`), no composed frame retired yet: eligible Presents are refused and composed; the composed frame retires through `route_owner_event_batch`; after the probation, the next Present enters direct and is admitted | **A1** |
| `c0_conv_ciii_add_every_output_needs_its_return_vulkan` | a device with **two outputs**, the first output's composed return established through a retired composed frame and the second's absent: entry is refused; once the second retires, entry is allowed | **A3** |

**Mutations** (apply by line, confirm the run compiled, restore; ledger in your
report):

- **A1** — remove the I-1 check: `entry_refused_without_a_composed_return`
  and `refused_present_composes_then_enters_vulkan` fail.
- **A2** — apply the check to successors too: `successor_eligibility_ignores_the_return` fails.
- **A3** — evaluate the predicate on the first output only:
  `every_output_needs_its_return_vulkan` fails. If no fixture can put two
  outputs on one device, or production entries cannot produce the
  first-established/second-absent state, that test is reported **not
  written, with the reason**, and **I-1's every-output clause is recorded as
  unproved** in the report — never counted as passing evidence, never proved
  with a fake device.
- **A4** — apply the check without the active-conductor condition:
  `legacy_eligibility_unchanged` fails.

**Gate:** `cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings` in
default, `--features tcp-transport` and `--features xdmcp`;
`cargo test -p yserver --lib c0_conv_ -- --include-ignored` and `c0_adm`
(debug); `cargo test -p yserver --lib` (debug). Report exact counts.

## Limits

Fixture level; production stays `Legacy` (C0-R8). No hardware run: stage 3a's
hardware run exercises direct entry again.
