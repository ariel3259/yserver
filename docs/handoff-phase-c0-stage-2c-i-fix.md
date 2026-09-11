# Handoff — Phase C.0 stage 2c-i, fix round after implementation review 1

**Date:** 2026-09-11
**For:** Claude Sonnet 5 as the implementing model, in Claude Code (the
Superpowers skills load for you; use `superpowers:executing-plans` and
`superpowers:test-driven-development`)
**Branch:** `feat/phase-c0-atomic-kms-migration`, worktree
`/home/ariel_santangelo/Projects/yserver-phase-b`
**Baseline:** the commit that adds this document; tree clean, `origin/master`
merged at `3033fa8a`
**Plan:** `docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md`
(checkboxes now reflect the review, not Gemini's claims)
**Findings you are closing:** `docs/superpowers/findings/2026-09-11-stage-2c-i-implementation-review-round1.md`
— every `B-n`/`M-n` below refers to it
**Previous handoff:** `docs/handoff-phase-c0-stage-2c-i.md` — its **Rulings
R1–R12 remain binding** and are not repeated here. Read them first.

## What happened

Gemini 3.8 Flash executed all ten tasks (`38ce1eb4..5ac85777`) and ticked
every checkbox. The review found the type vocabulary and four pieces of
mechanism sound, and everything that joins them absent: no production path
registers a payload alias or discharges `file_owned`; the fd-family barrier
inverts R5 and is derived from `Rc::strong_count`; `DeviceBarrier` is a free
public constructor so every barrier in every test is a literal; Task 9.5
does not exist; the transport gate is enforced at zero sinks; `Terminal`
freezes every old set on every successful flip; the 7.6 regression calls
`apply_validated_proof` under a comment saying it doesn't. Thirty-eight
steps were ticked with no or vacuous code. The suite is green because the
tests were shaped around the implementation.

You are fixing on top of that tree, not restarting. The fix is nine
sessions, one per task below, each ending in its own commit plus a fold-back
commit, each reviewed before the next starts.

## Keep — do not rewrite

These were reviewed and found correct. Touch them only where a finding
names them.

- `resources/{mod,availability,lease}.rs` — the Task-1 ledger, counters,
  `service_ready`, lease `Drop`. Open finding: M-17 only.
- `DrmCleanupRegistry::consume` (`drm_cleanup.rs:255-296`) and its retry
  tests — the R3 state machine.
- `FileOwnedBacking::new` as the sole constructor and its pairing check.
- Legacy `ScanoutBo::Drop` (untouched, per R3) and
  `DirectScanoutProbeFramebuffer::Legacy`.
- The legacy `Storage` path in `store.rs` (constructors leave `vk: None`;
  `destroy()` reproduces the old sequence).
- `poll_gpu` / `validate_gpu_batch` / `commit_gpu_batch` /
  `quarantine_gpu_batch` (`mod.rs:612-770`).
- `DirectCapacity`'s table, serials and `move_into_reserved` validation
  (`capacity.rs`), except where M-7/M-8/minor items name it.
- The `Rejected<R>` path in the generic ledger (`owner/ledger.rs`,
  `device.rs:2233-2246`).

## Rules for this round (in addition to R1–R12)

**F1 — a checkbox is ticked when its finding is closed with a test that
would fail on Gemini's tree.** For each step you tick, the fold-back names
the finding(s) it closes and the test that proves it. Where you can, run
that test against the pre-fix code path (e.g. temporarily revert the fix)
and record that it failed. A finding with no test is not closed.

**F2 — every finding in your task's row gets a verdict in the fold-back:**
`RESOLVED (test: …)`, `NOT APPLICABLE (why)` or `DEFERRED TO <task> (why)`.
Silence is not an option. If you believe a finding is wrong, say so with
file:line evidence; do not quietly skip it.

**F3 — no Spy where the real type exists.** Tasks 3–5 built real payload
types. A test for a Task-3+ mechanism uses `StorageAllocation`,
`ScanoutAllocation`, `CopiedSourceAllocation`, `DirectFramebufferAllocation`
or a real `GpuObligation`/`CoreRetirementBatch`. `Spy` is for Task-1 ledger
mechanics only. A test that fabricates the proof under test with
`apply_validated_proof` proves nothing (B-9, B-15).

**F4 — test-only constructors live under `#[cfg(test)]`.** `mock`,
`new_for_tests`, `new_for_test`, `for_tests`, `FakeFamilyInventory`,
`drop_counter`: all of them. If production code needs one to compile, the
production code is wrong (B-6). `#[doc(hidden)]` is not a substitute.

**F5 — no `Option<Arc<VkContext>>`, no `poll_signaled_result_opt`, no
status fallback.** The contracts say `Arc<VkContext>`. A `Drop` that
silently does nothing because a context is `None` is a leak with a comment
(M-23). Global Constraints: never invent a platform status fallback.

**F6 — no `let _ = <Result>` in `resources/`.** A discarded
`InvalidProof`/`Detached`/`Frozen` is a route that should have closed and
didn't (M-9). Propagate, record, or close the route.

**F7 — visibility is part of the contract.** `apply_validated_proof`
private to `resources` (`pub(in crate::kms::render::resources)`, with a
`#[cfg(test)]` shim for `store.rs` tests); `AllocationLease::new`,
`AllocationLease.entry`, `AllocationEntry.payload`, `DrmCleanupRight`
fields, `RoleReservation.role/serial` private; `DeviceBarrier` and
`TeardownRelease` sealed. `render/mod.rs` goes back to `pub(crate) mod
resources`; make `commit_owner_for_tests` `pub(crate)`.

**F8 — stop and report, do not paper over.** The previous implementer hit
a real fixture gap (`GbmDevice` over `Device::for_tests()`) and omitted the
test silently. If a step cannot be done as specified — a seam the plan
assumes does not exist, a test needs hardware you cannot reach, a contract
contradicts another — end the session with a written report in the
fold-back and stop. That is the correct outcome; a ticked box over missing
code is the wrong one.

## The gate — run all of it before every commit

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test -p yserver --lib c0_2ci
for i in $(seq 1 12); do
  cargo test -p yserver --lib c0_2ci 2>&1 | grep -E '^test result:' | grep -q ' 0 failed' \
    || echo "FLAKE on run $i"
done
```

Plus the three portable checks for any task touching `drm/`, `drm_cleanup.rs`
or `transport.rs`:

```bash
cargo check -p yserver --target x86_64-unknown-linux-gnu
cargo check -p yserver --target x86_64-unknown-linux-musl
cargo check -p yserver --target x86_64-unknown-freebsd
```

Hardware tests (`_drm`, `_vulkan`) are `#[ignore]`d and you run them
explicitly with `cargo test -p yserver --lib <name> -- --ignored`; this box
has a real DRM node and NVIDIA/RADV ICDs. Record the actual output in the
fold-back. R2 on the full suite still applies.

## Fix tasks — in this order, one per session

Dependencies are real: F-2 needs F-1's registry inventory, F-6 needs F-5's
gate, F-8 needs everything. Do not skip ahead.

| Session | Plan task | Closes | Decisive test |
| --- | --- | --- | --- |
| F-1 | Task 2 (+ M-17 from Task 1) | B-1, B-2 (registry half), M-17, M-23 (`DrmCleanupRight`, `FakeFamilyInventory`), minor `retire_closed_family` assert | 2.5 rewritten |
| F-2 | Task 4 | B-2 (adoption + discharge half), B-12, B-13, M-22, M-23 (`mock`, `Option` contexts), minor `acquire_managed_scanout_bo` validation | 4.6 rewritten; 9.5's `_drm` case (see below) |
| F-3 | Task 3 | B-14, M-18, M-19, M-20, M-21 | 3.5b rewritten |
| F-4 | Task 5 | B-15, M-23 (`GpuObligation.context`, `poll_signaled_result_opt`, `drop_counter`) | 5.1 on the real snapshot path |
| F-5 | Task 6 | B-6, B-10, B-11, M-13, M-14, M-16, minor `consume_owner_write` accounting | 6.5a/6.5b at real sinks with the counting transport |
| F-6 | Task 7 | B-8, B-9, M-2, M-3, M-4, M-5, M-6, M-15, minor `GroupMember::validate_unique` | 7.6 rewritten, owner-event-only |
| F-7 | Task 8 | M-1, M-7, M-8, minor `finish_role`/`move_into_reserved` state checks | 8.6 with real role transitions |
| F-8 | Task 9 | B-3 (deterministic half), B-4, B-5, B-7, M-9, M-10, M-11, M-12, minor `transfer` key check | 9.1, 9.3, 9.5a, 9.6 rewritten against real barriers |
| F-9 | Task 10 | B-16, M-24, matrix rows marked MISSING/Partial | 10.2 under validation layers; 10.3 audit table; `docs/status.md` corrected |

### F-1 — Task 2: the barrier and the registry inventory

The centre of the whole stage. Read plan Task 9's "Reaching the barrier"
paragraph (three numbered steps) and R5 before writing anything.

- `try_mint_file_family_closed` (B-1): drop the `Rc::strong_count` check
  and the `payload_aliases > 0` refusal. The conditions are: every
  submitter detached, helper reaped, control alias closed, non-payload
  aliases zero. When those hold, the registry **performs** step 2 — it
  walks its own inventory of outstanding file-owned contexts (which it
  must now keep: `register_right` records the right id, adoption registers
  the alias against an entry key), discharges each in the R3/R4 order via
  a callback the service supplies (the service owns the payloads; the
  registry owns the order), unregisters each alias as it closes, then
  drops its own `Rc` and mints. `payload_aliases` is the count of what the
  discharge will close, not a precondition.
- Task 2.5 rewritten: adopt a real `DirectFramebufferAllocation` (whose
  `Rc<drm::Device>` is `Device::for_tests()` — fine for this half, it is the
  ordering that is under test) into a `ResourceService` wired to the
  registry; assert the barrier is mintable **while** the payload still
  holds its alias; assert after minting that the payload's `file_owned` is
  `None`, the counting transport saw exactly one `RemoveFramebuffer` and
  the right `CloseGem` count for the `GemOwner`, `Weak::upgrade()` on the
  device is `None`, and no call follows the mint. Also rewrite
  `c0_2ci_drm_cleanup_fake_family_barrier_requires_all_closed`, which
  shares the inverted premise.
- M-17: `apply_validated_proof` visibility per F7.
- M-23: `DrmCleanupRight` fields private, `new` private to `resources`;
  `FakeFamilyInventory` and its methods under `#[cfg(test)]`.
- `retire_closed_family`: return `Err`, do not `assert_eq!`.

Commit: `fix(kms): make the fd-family barrier discharge payload aliases`.

### F-2 — Task 4: real payloads and the one closer

- B-13 first: move the physical fields out of `ScanoutBo` /
  `CopiedRenderSource` into `ScanoutAllocation` / `CopiedSourceAllocation`
  as 4.3 says, with a consuming conversion; the legacy `Drop` on the
  legacy types stays for the legacy path only. A tagged `managed_key` on a
  BO that still owns everything is the two-closers shape.
  `acquire_managed_scanout_bo` must own the `BoPhase` transition.
- B-2 (service half): `ResourceService::adopt` registers the alias with
  the registry for any payload carrying an `Rc<drm::Device>`;
  `service_ready`'s destruction of a payload with `file_owned == Some`
  first discharges through the registry (normal-path cleanup when the KMS
  proof arrived), never drops it undischarged. `apply_teardown_release`
  (M-10) refuses `file_owned == Some`.
- B-12, 4.6 rewritten: both `GemOwner` variants. For `Gbm` the sole closer
  is libgbm — so the assertion is "zero `CloseGem` on the transport, and
  the gbm_bo handle is observed dropped once" (use a real gbm_bo in the
  `_drm` case, and a `#[cfg(test)]` drop-observing wrapper in the
  deterministic case; report in the fold-back which you used where).
  Include the `FramebufferRemoved` retry path and payload destruction, not
  just a bare `FileOwnedBacking`. Replace the four vacuous tests named in
  B-12 with ones that call the function they are named after.
- **9.5's real-GBM case lives here**, because it needs only the registry,
  `FileOwnedBacking` and a real gbm_bo — see the plan's 9.5 fixture note:
  `c0_2ci_fd_family_barrier_real_gbm_payload_drm`, render node via
  `Device::open_render_node`, `#[ignore = "requires a real DRM render node;
  run explicitly"]`, `panic!` when absent. Run it on this box and paste
  the output. The barrier must be mintable while the payload holds its
  alias; discharge must drop the gbm_bo before the device alias; the
  registry's close must be last; the counting transport must show nothing
  after it.
- M-22: explicit `drop(gbm_bo); drop(device);` in `discharge`;
  `CopiedSourceAllocation::Drop` calls `destroy_transfer_resources`.
- M-23: `SharedBacking::mock` / `CopiedSourceAllocation::mock` under
  `#[cfg(test)]`; `vk`/`render_vk`/`sink_vk` back to `Arc<VkContext>`.

Commit: `fix(kms): retain scanout physical ownership in managed payloads`.

### F-3 — Task 3: storage adoption that does not leak

- B-14: `Storage::into_managed` takes the `Arc<VkContext>` and pool it
  needs (from `&PlatformBackend` or explicit args) and refuses adoption
  without them; a test adopts a non-stub allocation and asserts
  `cleanup_handles` runs once with the real context.
- M-19: replace the panicking `Deref`/`DerefMut` with the accessor
  conversion 3.3 asks for in `engine.rs`/`target.rs`/`backend.rs`/
  `scene.rs`/`frame_builder.rs`/`ops/render.rs`. If 202 call sites is too
  much for one session, that is a report under F8 — propose the split, do
  not ship a `Deref` that aborts the server on the first managed drawable.
- M-18: `lease.allocation.entry.payload.borrow_mut()` from `store.rs` goes
  through `with_storage_write`; `AllocationLease::new`/`entry` private.
- M-20: convert `destroy_now`, `poll_pending_retire`,
  `shutdown_destroy_all`, `retire_image_after`/`destroy_retired_image`;
  `Storage::destroy` for `Managed` is not `{}`.
- M-21: 3.5b as written — FreePixmap, deferred retirement, export,
  `DRM_FORMAT_MOD_INVALID`, client-size distinguishability, once-only FD.

Commit: `fix(kms): route managed storage access and retirement through leases`.

### F-4 — Task 5: the read/scene adapters

- B-15: 5.1 is built on the real root IncludeInferiors snapshot path
  (`read_scanout_region`, the CPU copy, the Composite ticket); the
  source-read proof is produced by the adapter from `poll_signaled_result(&vk)`,
  not by the test. 5.3/5.5 wire `drain_pending_pool_releases`, `PendingAck`,
  `engine.rs`, `frame_builder.rs`, `vk/ops/mod.rs` as the plan's file map
  says. 5.6's "scratch free after Composite error" and "descriptor reset
  exclusion" move real objects.
- M-23: `GpuObligation.context: Arc<VkContext>`; delete
  `poll_signaled_result_opt`; `drop_counter` under `#[cfg(test)]`;
  `ValidatedGpuBatch` private.

Commit: `fix(kms): produce read and gpu proofs from real adapter evidence`.

### F-5 — Task 6: the gate at the sinks

- B-10: the findings doc's R11 sink inventory (12 rows) is your worklist.
  Every row that is a legacy DRM write gets `allows_legacy(class)` checked
  **beneath** the real entry point (inside `submit_flip_with_fences`,
  `submit_direct_scanout`, `submit_composed_scanout`, `commit_modeset`,
  `disable_output`, cursor set/move, `set_gamma`), with the counting
  transport under it. Classify the test-only modeset and the vblank arm
  (observational — say so in the table). `consume_owner_write` is called
  at the executor send boundary when the request is accepted (R7).
  6.5a/6.5b drive each sink four ways (Legacy/Quiescing/Owner-with-grant/
  Owner-without-grant) through the real function. Delete
  `c0_2ci_transport_gate_writer_boundary_enforcement` as it stands.
- B-11: per-batch serviced deadline, checked arithmetic, from that
  batch's registration; `set_seat_active` driven from the real VT/DPMS
  seams; expiry quarantines that batch only, never flips a global
  `exhausted`.
- B-6: `RecipientReservation::new_for_tests` back under `#[cfg(test)]`;
  whatever production code needed it is wrong (see F-8).
- M-13: `begin_quiescing`'s Busy reads the real ownership-unit state and
  unflip request/retire, not setters. M-14: `close` refuses with
  outstanding grants; `issue_handover_permit` takes `LegacyDrained` and
  the dispositions; `try_finish_legacy_transport` connected. M-16: 6.1 on
  the core-loop fake backend; `next_deadline` keeps polling while
  inactive (the budget pauses, service does not).
- Fold-back **must** contain the caller inventory table (R11).

Commit: `fix(kms): enforce the transport gate beneath every drm sink`.

### F-6 — Task 7: `Terminal` by cause, one keyed discharge path

- B-8: match on `terminal`; only `CompletionUnknown` freezes, and only
  the keys correlated to that commit. A test feeds the real owner event
  order (`HardwareComplete`, `CompletionRetired`, `Terminal{Completed}`)
  and asserts the old set becomes releasable; another feeds
  `Terminal{FailedBeforeSubmit}` then `ResourcesStillCurrent` and asserts
  the current set is neither frozen nor cancelled-to-discharged.
- M-3/M-2/M-4: one bookkeeping path — the triples on
  `CommitResources::kms_obligations`, as the plan chose; delete
  `in_flight`/`correlate_commit` and the un-keyed `crtcs` loop; validate
  all proofs, then apply (no `?` mid-loop).
- M-5: the displaced-pair producer adapter (test-only producer is fine
  under R8, but the adapter code is production): compute
  `(allocation, member)` with `new[member] != old[member]`, `register_kms`
  before `Submitted::new`, pre-IPC failure returns registration ownership;
  add `take_current()`.
- B-9: 7.6 rewritten — real `CommitResources` old/new through
  `CompletionRetired`, `new` obligations registered via `register_kms`,
  the only proof path is the owner event, assert `has_pending_obligation`
  on the `new` set after `HardwareComplete`, assert the `KmsDisposition`
  after cancel is not `Discharged`, end with `cancel`. Not one
  `apply_validated_proof` call in the body. Also the two-output
  reference-CRTC case and the Skip/never-submitted-successor case (M-11 of
  the reviewer's list; `Presented` consumes `samples`).
- M-6: 7.5/7.5a as written — `release_present_source`,
  `retained_present_wakes`, `PresentRelease` consumption, the COW test
  with its assertions, `deferred_cow_release` back to private.
- M-15: `Quarantined` freezes only that commit's entries and closes the
  gate (give the consumer the gate handle it needs).

Commit: `fix(kms): consume commit outcomes by cause with one keyed release path`.

### F-7 — Task 8: transitions the consumer actually performs

- M-1: `on_available` collects, assigns back, then returns.
- M-7: `CompletionRetired` performs `move_into_reserved`/`move_role`,
  returns the Submitted token to Current; `finish_role` rejects a merely
  `Reserved` token; 8.3 (managed preparation seam, `implicit_layout`
  rejection, bounded probe cache) and 8.5 (composed-return retention per
  output, unflip wait) get code in the backend preparation seam the task
  names.
- M-8: remove the `has_pending_obligation` skip; fix the double
  registration in the test.
- 8.6 rewritten so the destination token is *not* pre-attached and
  occupancy is actually reached; `RoleReservation::new_for_test` under
  `#[cfg(test)]`.

Commit: `fix(kms): perform direct role transitions in the commit consumer`.

### F-8 — Task 9: real barriers, real revocation

- B-4: `DeviceBarrier` variants carry the proof (`FileFamilyClosed` by
  value, or a `_private: ()` field); the only constructors are
  `from_file_family_closed(FileFamilyClosed)` and a device-loss
  constructor that takes proven evidence; `record_device_barrier` takes
  the barrier by value. Every `DeviceBarrier::FileFamilyClosed(dev)`
  literal in tests goes through a real `try_mint_file_family_closed`.
- B-5: `TeardownRelease` constructible only by consuming the actual
  `FileFamilyClosed` + the recorded dispositions; `RetainingSupervisor`
  under `#[cfg(test)]`, and `reserve_slot` with it (this is what B-6
  needed).
- B-7: `IncarnationBundle` carries the `TransportGate`; `transfer` calls
  `revoke_owner_writes` before `close`, records each revoked grant's owner
  record as `Quarantined`, freezes uncertain entries, stops acquisition;
  9.6's `ExecutorStalled`-with-grant case tests it.
- B-3 (deterministic half): the fake-inventory ordering test as 9.5
  describes (control alone: no; reap with one alias: no; last alias: mint,
  discharge, late lease drop issues no ioctl); the real-GBM `_drm` case
  was written in F-2 — cite it.
- M-9: no `let _`. M-10: `file_owned` disposition validated. M-11:
  returned descriptors registered under the incident and blocking the
  mint until closed; movable poll registration. M-12: 9.4 code
  (engine/store detach preserving cleanup ownership, managed
  `shutdown_destroy_drawables`); 9.1 with real dispatch uncertainty, a
  destroyed backend fixture, counted fd cleanup and wakes.

Commit: `fix(kms): seal teardown barriers and revoke owner writes at handoff`.

### F-9 — Task 10: evidence that is true

- B-16: 10.2 frees a drawable, repeats for promoted backing and snapshot
  scratch, reads the invalidation counters, runs under validation layers
  (`VkContext::new` enables `VK_LAYER_KHRONOS_validation` in debug builds
  and with `YSERVER_VK_VALIDATION` set — `vk/device.rs:338-350`; the layer
  is installed on this box; assert zero validation messages), and includes a
  gbm_bo in the `_vulkan` case where the box has a render node. Paste the
  run.
- M-24: 10.3's caller-audit table in the plan's execution notes; the 6.5a
  inventory (from F-5) referenced; every environmental skip listed;
  `docs/status.md` corrected — no lavapipe claim, no "verified
  destruction" claim without the validation-layer run, and the readiness
  boundary names what 2c-ii can rely on **and what it cannot**.
- Matrix rows marked MISSING/Partial in the findings get real fixtures
  (F3): two services sharing a real BO; grouped A/B with a shared source
  and reference CRTC; unflip with a composed-return resource; the
  unknown→detach→late-reply→reap row through a real mint.

Commit: `test(kms): verify resource terminalization adapters against real payloads`.

## Fold your work back into the plan

After each session, in a **separate commit** from the code:

- tick the steps you closed and, under the task's `Status:` line, replace
  the `REJECTED` note with `**Fix round 1: <sha>.**` followed by the F2
  verdict table for every finding in your row;
- correct the shown code to what actually compiled;
- record what the task required beyond its written text, any R1 fix, and
  any F8 stop.

Do not tick a step whose finding is `DEFERRED`.

## When you finish a session

Report: findings closed with their tests, findings deferred and why, the
hardware runs you executed with their output, and anything you had to fix
under R1 or stop on under F8. Do **not** push, squash-merge or activate
anything. The next session does not start until the coordinating review
of this one has passed.
