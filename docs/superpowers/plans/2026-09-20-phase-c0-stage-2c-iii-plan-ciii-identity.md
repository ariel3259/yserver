# Stage 2c-iii, plan Ciii-identity — one commit key, one device

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for hardware work) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters (`c0_conv_ciii_id_`, `c0_conv_cii_`, `c0_conv_ci_`, `c0_conv_cir_`), never `_drm`, `render_acceptance`, unfiltered `--ignored`, or anything that modesets or takes DRM master while the user is looking at the screen; no deletes outside the worktree. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests and the mutations each must catch. Execute tasks in order, one per run. You can run the `_vulkan` tests yourself: nothing is done until its tests pass on the real GPU, in debug and release. Do not ask for approval; a real design choice the plan leaves open, or a claim here that does not hold in the code, is an F8 stop you report.

**Revision 1 (2026-09-20)** — born by splitting plan Ciii at its revision 11
(user decision, 2026-09-20: "partí el plan, ya llevamos 11 revisiones"). Its
two tasks are plan Ciii revision 11's Tasks 1 and 2, unchanged in substance and
already carried through **ten codex review rounds** — rounds 3 B-2, 4 M-1,
5 B-1, 6 B-1 and 9 B-1 are theirs, and revision 7's `CommitKey` is what closed
that family. Nothing here is new design; what changes is that this repair is
accepted on its own, with its own mutations, **before** anything is built on
top of it.

**Why it is a plan of its own.** This is not unflip work. It repairs an
assumption of stages 2c-i and 2c-ii: that there is one device. Each
`DeviceCommitOwner` numbers its commits from 1 and every consumer in the
backend, the resource service and the scene correlates by that bare number, so
two owner devices silently share one another's completions, dispositions,
pins, damage transactions and buffers. Plan Ciii's unflip rests on this being
fixed; keeping them together meant re-reviewing the unflip every time the
identity contract moved.

**Goal:** make cross-device correlation impossible by construction, and prove
that one device's conductor, gate, owner and direct group are its own.

**Spec:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`,
revision 4 with §8.3 as amended and §6.4 as annotated: **§6.2** and the §3.2
registration rules it protects. §6.1, §6.3 and §6.4 are plan Ciii's.

## Design decisions this plan fixes

1. **Identity is a type, not an enumeration** (plan Ciii round-6 B-1). Rounds
   3, 4, 5 and 6 each found one more consumer correlating by a bare
   `CommitId`; enumeration was not converging. A **`CommitKey`** — a
   `DrmDeviceKey` and a `CommitId` together — becomes the only thing a
   correlating consumer accepts, so a site that still correlates by number
   **fails to compile**. The checklist in Task 1 is the migration list, not
   the safety net. This is the move the Ci-refactor made with `OwnerBuffer`.
2. **The consumer is not split per device.** `DirectCapacity` is one direct
   group for the whole backend by design, and `scanout_m2` is one direct unit;
   what changes is the **key**, not the ownership. Device-blind helpers that
   are deliberate — `admission_note_layout_change_all_devices`
   (`admission.rs:793`) among them — are named in the task's report rather
   than rewritten. A device-blind path that changes **another** device's
   conductor, gate, owner, damage or buffers is a defect, fixed here.
3. **The evidence is a two-device fixture, not the live one.** Ci's owner-route
   live fixture asserts **exactly one** KMS device (`backend.rs:6898`);
   `test_kms_device` plus `install_transport_gate` is the shape the platform's
   own multi-device tests already use (`platform.rs:8798`, `platform.rs:9797`).
4. **Test names start with `c0_conv_ciii_id_`**, so one filter selects this
   plan — and plan Ciii's own `c0_conv_ciii_` filter keeps them green as a
   regression.

## Limits stated

- The unflip, route-selection exclusivity and the §6.4 hardware run stay in
  plan Ciii, which is implemented after this one is accepted.
- The copied composed route is its own later plan (plan Ciii round-1 B-2).
- Cursor and gamma producers, the cursor coordinate lane, topology and
  cursor-recovery dispatch, lifecycle and recovery, device loss: stages 3 and 4.
- Production is unchanged (C0-R8): no conductor is installed there, so no
  production device is an `Owner` device and no production path has two.

## Global Constraints

- **Production is byte-for-byte unchanged.** Each task keeps a named Legacy
  characterisation test green.
- Owner milestones reach the scene only through `route_owner_event_batch`;
  tests deliver them that way (a stub behaviour or crafted events handed to it
  — say which), never by calling a handler directly.
- No hand-built `PendingAck`, `BoPhase`, `OwnerBuffer`, `DirectPresentFrame`,
  scanout pool or prepared generation in any test.
- Resources travel by value; nothing is bare-dropped; every token is consumed
  exactly once.
- No side effect inside `debug_assert!`, and none inside a readiness
  computation; fail closed, never panic, in non-test code; no test-only hook
  that bypasses the path it is named after.
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
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_conv_ciii_id_; done
cargo test -p yserver --lib c0_conv_ciii_id_ -- --include-ignored --test-threads=1
cargo test --release -p yserver --lib c0_conv_ciii_id_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_cii_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_ci_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_cir_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

The **last task** and the coordinator at acceptance also run, for
`x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl` and
`x86_64-unknown-freebsd`:

```bash
cargo check --workspace --target <target>
```

Baseline before Task 1 (commit `5a34c6ec`): `c0_conv_ci_` 38/38 and
`c0_conv_cii_` 26/26 with `--include-ignored` in debug and release;
`c0_conv_cir_` 7/7; `c0_adm` 129/0; `c0_2ci` 180/0/21; `--lib` 1933/0/142.
Every task ends at those numbers plus its own new tests. The full hardware gate
(306/306 at `5a34c6ec`) is the coordinator's, after the last task, with the
user's go-ahead.

**Mutation ids keep plan Ciii revision 11's numbering**, so the two plans never
collide and the review rounds that produced them stay readable.

## Exit criteria

| Criterion (spec) | Tests | Mutation that must fail them |
| --- | --- | --- |
| Two owners' equal numeric `CommitId`s never correlate across devices (§6.2, §3.2) | `c0_conv_ciii_id_equal_commit_ids_do_not_cross_devices` | T36: key the completion cache by `CommitId` alone; T37: match a releasing resource without comparing its device |
| A terminal or `CompletionUnknown` on one device leaves another device's Present disposition and pins alone (§6.2) | `c0_conv_ciii_id_foreign_terminal_leaves_a_pending_present_alone` | T40: drop the device comparison from the `present_dispositions` scan; T41: match the pending direct frame by `CommitId` alone |
| A `Presented` or `CompletionRetired` on one device never samples, publishes, promotes or releases another device's pending direct frame (§6.2, §3.3) | `c0_conv_ciii_id_foreign_milestones_leave_a_pending_direct_frame_alone` | T44: publish the pending frame at retirement without matching its `CommitKey`, as today's no-argument helper does |
| One device's milestones never accept, apply, remove or block another device's damage transaction or owner buffer (§6.2) | `c0_conv_ciii_id_foreign_milestones_leave_a_damage_transaction_alone` | T42: key `owner_damage_transactions` by `CommitId` alone; T43: scan owner buffers without comparing the device |
| An event, wake, refusal or bound violation on one device changes nothing on another (§6.2) | `c0_conv_ciii_id_devices_are_independent` (one case per kind: owner event batch, admission wake, refused offer, bound violation) | T28: route the batch to every conductor; T29: close every device's gate on a bound violation |
| A device's layout generation, composed intents, maintenance and receipts belong to that device alone (§6.2) | `c0_conv_ciii_id_conductor_state_is_per_device` | T30: bump every conductor's layout generation on one device's change |
| A grouped direct unit never crosses devices (§6.2) | `c0_conv_ciii_id_direct_group_never_crosses_devices_vulkan` | T31: drop the single-device precondition from `direct_scanout_topology_eligible` |

---

### Task 1: Commit identity is device-qualified

**Files:** `crates/yserver/src/kms/render/resources/commit.rs`,
`resources/present.rs`, `backend.rs` (the event routing and the pending direct
frame), `scene.rs` (the damage transactions and the owner-buffer scans); tests.

**Why this is a task and not an assumption.** Each `DeviceCommitOwner` mints
`CommitId`s from **its own** allocator starting at 1 (`owner/device.rs:217`,
`identity.rs:136`-`150`), so two owner devices issue the same numbers. The
consumers that correlate by those numbers are shared.

**The mechanism (round-6 B-1).** Correlation stops being possible by number:
this task introduces a **`CommitKey`** — a `DrmDeviceKey` and a `CommitId`
together — and **every consumer that correlates takes a `CommitKey`**, never a
bare `CommitId`. A site that still correlates by number then fails to compile,
which is what ends the per-round discovery of one more consumer. The owner's
own internal records, which never leave their device, may keep the plain id;
any other correlation deliberately left device-blind is **named in the task's
report** with why.

**The migration checklist (round-3 B-2, round-4 M-1, round-5 B-1, round-6
B-1).** Every site below is converted; it is a checklist, not the safety net —
the type is:
- the single `CommitResourceConsumer` (`backend.rs:1501`) and its
  `hardware_completed_commits`, `commit_members` and `reserved_retirements`,
  keyed by a bare `CommitId` (`resources/commit.rs:130`-`145`);
- its releasing/rejected-resource match, `res.commit_id == Some(commit)`, with
  no device (`resources/commit.rs:286`);
- the `Terminal` handler's scan of `present_dispositions`, comparing only
  `key.commit` although `PresentKey` already carries a device
  (`resources/present.rs:4`-`8`, `resources/commit.rs:394`-`400`), and the
  `Presented` consumer beside it;
- the owner-event routing, which holds `device_key` and does not pass it to the
  consumer (`backend.rs:20361`);
- the pending direct frame's recorded identity and
  `managed_enqueue_unknown_direct_completion`, which matches it by `CommitId`
  alone (`backend.rs:588`, `:2921`);
- the scene's `owner_damage_transactions`, keyed by a bare `CommitId`
  (`scene.rs:1080`-`1081`), whose installation refuses a number already present
  (`:1907`) and whose `Accepted`/`HardwareComplete`/retirement/terminal paths
  are called with the number alone (`:1956`-`:2004`);
- the owner-buffer transitions, which scan every output for
  `buffer.commit_id() == Some(commit)` without a device (`scene.rs:2085`,
  `:2175`, `:2217`);
- the pending direct frame's **ordinary** milestones (round-6 B-1):
  `managed_record_direct_presented`, which matches the frame by `CommitId`
  alone (`backend.rs:2844`-`2856`), and
  `managed_enqueue_retired_direct_completion`, which takes **no argument** and
  unconditionally takes the pending frame (`backend.rs:2878`-`2899`), together
  with the routing that calls them (`backend.rs:20328`-`20358`,
  `:20401`-`:20465`). Both take the frame's `CommitKey` and do nothing unless
  it matches.

Splitting the consumer per device is **not** the shape: `DirectCapacity` is one
direct group for the whole backend by design (decision 2). What changes is the
**key**, to `(DrmDeviceKey, CommitId)`; `route_owner_event_batch` already
carries the device at every consumption site.

**Invariants (spec §6.2, §3.2):** no device can discharge, cache, consume,
terminalize, accept, apply, refuse or restore another's work, even when both
owners issue the same numeric `CommitId`. A cached `HardwareComplete` belongs to one device; a
`CompletionRetired` of the same number on another device neither consumes it
nor discharges any `KmsRelease` obligation. A `Terminal` or `CompletionUnknown`
on one device neither changes another device's Present disposition nor
releases another device's pins, and a `Presented` or `CompletionRetired` on one
device neither samples, publishes, promotes nor releases another device's
pending direct frame — **release-before-proof is impossible across devices**. One device's damage transaction is neither
accepted, applied nor removed by another device's milestone of the same
number, and installing a transaction on one device is never refused because
another device already holds that number. Every commit identity recorded after this plan — plan
Ciii's unflip retirement included — uses this qualified key (round-5 B-1).

**Named tests:**
- `c0_conv_ciii_id_equal_commit_ids_do_not_cross_devices` — two owners whose
  commits carry the **same numeric** `CommitId`, interleaved: A's
  `HardwareComplete` is cached, B's `CompletionRetired` of the same number
  arrives first, and B discharges nothing its own completion has not proven,
  while A's cache survives for A.
- `c0_conv_ciii_id_foreign_terminal_leaves_a_pending_present_alone` — A's direct
  Present is pending; B's commit of the same number reaches
  `FailedBeforeSubmit` and then `CompletionUnknown`; A's disposition and A's
  pins are untouched.
- `c0_conv_ciii_id_foreign_milestones_leave_a_pending_direct_frame_alone` — A's
  direct frame is pending **with its `Presented` sample already recorded**; B's
  commit of the same number delivers `Presented` and then `CompletionRetired`;
  A's pending frame, A's current frame, the publication queue and A's pins are
  all unchanged (round-6 B-1).
- `c0_conv_ciii_id_foreign_milestones_leave_a_damage_transaction_alone` — A and B
  each hold a live damage transaction and an owner buffer under the **same**
  numeric `CommitId`; B's `Accepted`, `HardwareComplete`, `Terminal` and
  `CompletionRetired` are interleaved, and A's transaction, A's buffer and A's
  damage are untouched; B's own installation is not refused (round-5 B-1).

- [ ] Steps: enumerate and report the sites; tests; red; implement; checks;
  stop dirty and report.
---

### Task 2: Multi-device

**Files:** `admission.rs`, `backend.rs` and `platform.rs` where a device-blind
path is found; tests.

**Interfaces:** none new unless a defect is found. Commit identity is **Task 1's**; this task proves the conductor, gate, owner and direct-group halves of
§6.2.

The fixture is **not** the live single-device one (decision 3): two seeded KMS
devices with their own gates, the shape `platform.rs:8798` and
`platform.rs:9797` already use.

**Invariants (spec §6.2):** one conductor per `DrmDeviceKey`, each with its own
admission, layout generation and transport state; an event, wake, refusal or
bound violation on one device changes nothing on another — not its layout
generation, not its composed intents, not its maintenance store, not its
receipts, not its gate, not its owner; a grouped direct unit never crosses
devices. Device-blind paths that are
deliberate (decision 2) are named in the report rather than rewritten; a
device-blind path that changes another device's state is a defect, fixed
here.

**Named tests:**
- `c0_conv_ciii_id_devices_are_independent` — four cases: an owner event batch, an
  admission wake, a refused offer, and a bound violation that closes a gate.
- `c0_conv_ciii_id_conductor_state_is_per_device`.
- `c0_conv_ciii_id_direct_group_never_crosses_devices_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.
---

## What the coordinator does

After each task: read the diff, re-run the checks above outside codex, apply
this plan's mutations **by line** against the recorded mutation text, confirm
each is caught by its named test, then commit. `_vulkan` tests and their
mutations run on the GPU with the user's go-ahead.

After the last task, with the user's go-ahead and the GPU free: the full
hardware gate (`render_acceptance -- --ignored`, `c0_2ci -- --ignored`, the
library's other ignored tests — 306/306 at `5a34c6ec`, plus this plan's new
`_vulkan` test), the three `cargo check --workspace --target` gates, then the
acceptance finding and `docs/status.md`. Plan Ciii follows.
