# Stage 2c-i fix round — sessions F-7, F-8, F-9, F-4d, F-10 independent review

Companion to `2026-09-13-stage-2c-i-fix-F11-F12-opus-review.md`. These five
sessions (`f39a01c6..b748a202`, 2026-09-12 20:39 → 2026-09-13 08:06) were
implemented and self-reviewed by Gemini; the `…-F7/F8/F9/F4d/F10-review.md`
files are the implementer's claims. This is the independent pass: diff read,
gate re-run (fmt/clippy clean, `c0_2ci` 121/13, `--ignored` 13/13 on this
box), and a mutation on each session's decisive mechanism.

## Verdicts

| Session | Task | Verdict | Carried |
| --- | --- | --- | --- |
| F-4d | 5 write half | **ACCEPTED** (mechanism real, R8 kept) | F4d-M1 |
| F-8 | 9 | **ACCEPTED** | F8-M1, F8-M2, F8-m1 |
| F-7 | 8 | **REJECTED** | F7-B1, F7-B2 |
| F-9 | 10 | **ACCEPTED** | F9-m1 |
| F-10 | 3 (read-mostly) | **ACCEPTED** | — |

## F-4d (`d96e1c4c`) — Task 5 write half

Holds:
- The managed branch of `submit_shared_scanout_frame` (`scene.rs`) renders
  through the payload. Mutation: `ManagedSharedComposeTarget::{image,
  image_view, command_buffer}` returning the husk's fields → the process
  dies with SIGSEGV in the driver (same evidence F-4c's read half gave).
- R8: the legacy branch is the old body verbatim (diffed old function vs new
  `else` block: only `signal_fence`→`compose_ticket.fence()`, `Ok(submitted)`
  →`Ok((submitted, None))`).
- Threading of `Option<&mut ResourceService>` through `tick`/
  `maybe_composite`/`tick_one_output`/`drain_all`/`retire_failed_submit_bos`/
  `drain_pending_pool_releases` matches the F4c-review's description;
  `PendingAck.managed_batch` is registered at flip completion
  (`scene.rs:2165`), on flip reject (`:8031`), on `export_signaled_fd`
  failure, and in `drain_all` (register after a good fence wait, quarantine
  `Frozen` after a failed one).
- `with_scanout_read/write` skip the extra reservation when the caller's
  lease already is of that kind — required for the batch's Write lease.
- F4-m1: the allow on `read_managed_scanout_region_bytes` is gone
  (`read_scanout_region` now dispatches to it for a managed bo); only
  `read_scanout_region_for_managed_source` keeps one (no caller).

**F4d-M1 (major) — plan step 5.5 is ticked with no test that would fail.**
The four `is_releasable` gates (`drain_pending_pool_releases`,
`retire_failed_submit_bos`, `handle_page_flip_complete`'s retirement arm,
`drain_all`'s `PoolSlot` release) can all be disabled and the whole `c0_2ci`
suite including hardware passes (134/134). The decisive test
`c0_2ci_scene_managed_shared_compose_vulkan` exercises only the flip-reject
exit (fixture has no master — the test itself asserts the bo ends in
`Recording`), so neither the pool-recycle gating nor the flip-accepted path
(`PendingAck` → registration at completion, `scene.rs:2165`) ever runs.
Fix: a deterministic scene-level test on the existing
`drain_deferred_scene_resources` / `PendingAck` harness with a managed key
whose service says not-releasable, asserting the slot stays queued, then
releasable → released; and one that drives a `PendingAck` carrying a batch
through `handle_page_flip_complete` and asserts `pending_batches().len()==1`.
Then the mutation above must fail.

## F-8 (`ac3c94f7`) — Task 9

Holds (mutation: `register_returned_descriptor`/`register_pool_husk` not
counting + `quarantine_live` skipped on revocation → three tests fail:
`c0_2ci_handoff_complete_fd_family_barrier_deterministic`,
`c0_2ci_handoff_under_executor_stalled_revokes_grant_and_quarantines`,
`c0_2ci_adapter_unknown_detach_late_reply_reap`). `DeviceBarrier` sealed
(`_private: ()`, constructors take the proof by value), `TeardownRelease`
mint under `#[cfg(test)]`, `HandoffRouter::transfer` revokes before close and
quarantines on `revoked > 0`, `service` propagates errors.

**F8-M1 (major) — the pool-husk alias is a hand-bumped counter (M-13
pattern).** F2-m1 asked Task 9 to account for the husk's `Rc<Device>` clone
left by `take_physical_backing`. F-8 answered with
`DrmCleanupRegistry::register_pool_husk()/unregister_pool_husk()`
(`drm_cleanup.rs:367-373`), which only `tests.rs:4957/4980` call — neither
`register_managed_scanout_bo` nor `take_physical_backing` nor
`detach_managed_entries` registers or unregisters anything, so in any real
sequence the inventory does not follow the husk. Wire the counter at the
site that creates/destroys the alias (`register_managed_scanout_bo` has the
registry in hand; `detach_managed_entries` needs it), and make the F-4d
scene test's cleanup go through it.

**F8-M2 (major) — nothing in the router closes returned descriptors.**
`deliver_descriptor` registers the fd under the incident (M-11, correct),
but `close_returned_descriptors` has only test callers
(`tests.rs:4984`, `adapter_tests.rs:789`): a `HandoffRouter` that receives a
late descriptor can never mint the barrier without a test-only hand call.
The router owns that step — close them once the helper is reaped /
ingress drained, on its own teardown path, and let the deterministic test
observe it instead of calling it.

**F8-m1 (minor)** — `quarantine_live` terminalizes with
`UnknownCause::ContradictoryEvidence` ("an outcome whose shape contradicts
the request class"); a grant revoked mid-flight is not that. Use a cause
that says revoked/stalled (new variant or `HostCall`).

## F-7 (`f39a01c6`) — Task 8

Holds: `finish_role` rejects `Reserved` tokens; `on_available` restores
every unconsumed resource on error (read: the failing `res` is pushed back
before the remainder, admission closed); `CompletionRetired` moves old
Current into a pre-reserved retirement slot and Submitted into Current
(`commit.rs:255-290`); the capacity-level tests in `tests.rs` (6-role
contract, 8.6) drive real `DirectCapacity`/`CommitResourceConsumer`.

**F7-B1 (blocking) — the managed prepare seam charges no role past its own
return.** `KmsBackend::managed_prepare_direct_candidate`
(`backend.rs:18232-18330`, `#[allow(dead_code)]`, test-only caller) reserves
`Preparing`, moves it to `Successor` on success (`:18298`), queues the frame
in `scanout_m2.queued_successor`… and then `cancel_reservation(prep_slot)`
(`:18327`) — the Successor role goes back to `Vacant` in the same call. The
queued successor (and the victim it replaced) is unaccounted, so 8.3's
"bounded live imports" and 8.6's "maximum of six charged positions" are void
for the seam; a second candidate reserves and moves freely. `move_role` also
never turns a `Reserved` into `Occupied`, so nothing the seam does could
ever be `finish_role`d. `prereserve_retirement` (8.4) has no caller but
tests, so `CompletionRetired`'s pre-reserved branch is unreachable from the
seam. Only the `implicit_layout` rejection path is tested
(`c0_2ci_backend_managed_prepare_direct_candidate_implicit_layout_rejection`
asserts `occupied()==0` — true on the success path too).

**F7-B2 (blocking) — the managed unflip seam does not implement 8.5.**
`managed_handle_direct_unflip` (`backend.rs:18340-18360`) documents "uses
ExitRetirement for Current even if OrdinaryRetirement is occupied; waits for
submitted work"; the body requests the legacy unflip, drops the queued
successor, materializes the shadow, and returns `managed_can_enter_direct()`.
No `reserve(ExitRetirement)`, no `move_role(Current → ExitRetirement)`, no
wait. `c0_2ci_backend_managed_unflip_and_reentry_contracts` asserts the
unflip flag and reason only.

Required: the seam keeps the Successor charge on the queued frame (the
`DirectPresentFrame` carries its `RoleReservation`), occupies it at dispatch
and pre-reserves retirement (`prereserve_retirement`) before replacement;
unflip reserves/moves `ExitRetirement`; tests on the success paths asserting
`occupied()` counts across prepare → replace → dispatch → retire, and a
mutation that deletes the charge must fail. R8 still holds (no production
caller), but the seam must do what its contract says.

## F-9 (`a7139742`) — Task 10

Holds: matrix rows 3/7/10/11 are adapter tests over real
`ResourceService`/`CommitResourceConsumer` (row 7 read in full: reversed
`HardwareComplete` evidence, reference-CRTC sample, source retained until the
last member); F5b-m2's `allows_legacy(Modeset)` on `Drop`/rollback;
`c0_2ci_sink_gamma_gate_four_states_drm` reaches the ioctl (EACCES without
master) in Legacy and is refused before it in the other three states;
validation counters are thread-local, incremented in the real debug
messenger, layer enabled in debug builds and installed on this box.

**F9-m1 (minor)** — `c0_2ci_live_lifetime_adapters_vulkan` asserts zero
validation messages but never asserts the layer is active
(`debug_messenger.is_some()` / layer present); on a box without
`VK_LAYER_KHRONOS_validation` it passes vacuously.

## F-10 (`ebae39e0`) — Task 3 read-mostly

Holds. Mutation: `Storage::destroy`'s Managed arm keeping the lease →
`c0_2ci_storage_managed_destroy_transitions_to_detached` and
`…_detaches_before_drop` fail. The `PixelIdentity` extension with
`format`/views is fine (immutable handles). Note that F-11 later extended
the same struct with a mutable `current_layout` Cell — see F11-B1.

## Where this leaves the round

Blocking: F11-B1 (Task 3), F7-B1/F7-B2 (Task 8). Major: F11-M1, F4d-M1,
F8-M1, F8-M2. Minor: F12-m1..m3, F8-m1, F9-m1. Split into three fix
sessions (plan-size rule): **F-13a** Task 3 (F11-B1, F11-M1, F12-m1..m3),
**F-13b** Task 8 seam (F7-B1, F7-B2), **F-13c** tests and wiring (F4d-M1,
F8-M1, F8-M2, F8-m1, F9-m1). Each gets this reviewer's mutation pass before
the next; the final stage review is rewritten after F-13c.
