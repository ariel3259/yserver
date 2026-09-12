# Stage 2c-i fix round — session F-4 (Task 5) review

## Verdict

**Not yet accepted — 2 blocking, 3 major, 1 minor.** The M-23 half is
done and correct (`GpuObligation.context` by value, `poll_signaled_result_opt`
gone, `drop_counter` test-only, `ValidatedGpuBatch` private plus the
knock-on visibility fix), the new adapters in `resources/gpu.rs`
(`prepare_retirement_batch` with unwind, `cancel_pre_submit_batch`,
`freeze_uncertain_batch`, `record_read_outcome`) are the right shape, and
`submit_one_shot_op_async` is a real non-blocking submission whose ticket
reflects driver state. The session was interrupted by a rate limit and
resumed by a second Sonnet session, which reported honestly: the decisive
5.1 test has never been green, and 5.3/5.5 were stopped under F8. Two
things it did not report are the blocking items: eight previously
deterministic tests became `#[ignore]`d hardware tests (CI no longer
runs them), and the decisive test's fixture cannot work on any machine
as written.

Reviewed `4eb40641..2395881a` (`17384ae6` code, `2395881a` fold-back) by
the coordinating session (`claude-opus-5`), inline. Gate re-run here:
clippy clean, `c0_2ci` **80** deterministic (was 89), 16 ignored;
hardware run 15/16 with `c0_2ci_read_source_scratch_regression_vulkan`
failing on `prime_fd_to_buffer: Inappropriate ioctl for device`.

## Findings

### Blocking

**F4-B1 — deterministic coverage regression: eight `c0_2ci_` tests became
`_vulkan` `#[ignore]`d tests.** Deleting `GpuObligation::for_tests_stub`
(the only way to build an obligation without a live `VkContext`) forced
`c0_2ci_gpu_batch_late_invalid_proof_is_atomic`,
`c0_2ci_gpu_batch_freeze_lookup_failure_handled`,
`c0_2ci_gpu_ticket_error_quarantines_batch`,
`c0_2ci_gpu_dropped_frame_metadata_with_live_ticket`,
`c0_2ci_descriptor_reset_exclusion_until_gpu_signaled`,
`c0_2ci_progress_no_composition`,
`c0_2ci_serviced_time_pauses_during_seat_inactive_and_expires` and
`c0_2ci_adapter_vt_away_dpms_off_idle_service_progress` onto hardware.
Those test the batch machine's *logic* (atomicity, quarantine, freeze,
serviced-time accounting), which `CoreRetirementBatch.test_ticket_status`
already makes device-independent; they do not need a GPU, and CI runs
`cargo test` without `--ignored`, so the stage just lost them. The
contract says `Arc<VkContext>` and the resumed session read that
literally; the F5 amendment (F2-m2 ruling) is exactly for this case.

Fix: `GpuObligation.context: Option<Arc<VkContext>>` where the **only**
`None` constructor is `#[cfg(test)] for_tests_stub` and `GpuObligation::new`
takes `Arc<VkContext>` by value; `poll` consults `test_ticket_status`
first (it does) and otherwise requires `Some` — a `None` reaching the
real poll is a bug, `expect` it. Restore the eight tests as deterministic
`c0_2ci_` tests. Keep a `_vulkan` variant only where the real fence adds
evidence (`dropped_frame_metadata_with_live_ticket`,
`descriptor_reset_exclusion`).

**F4-B2 — the decisive 5.1 test cannot pass on any machine as written.**
`KmsBackend::for_tests_with_vk_live_scene()` builds its scanout pool over
`PlatformBackend::for_tests()`'s KMS device, whose fd is
`Device::for_tests()` — a Unix socket — so `ScanoutBoPool::allocate`'s
`PRIME_FD_TO_HANDLE` fails with `ENOTTY`. Verified here: the two
pre-existing tests on the same fixture (`root_get_image_reads_scanout_pixels_not_root_storage`,
`root_overlay_xor_pass_reaches_scanout`) hit the identical error and hide
it with `eprintln`+`return`. So `c0_2ci_read_source_scratch_regression_vulkan`
has never executed past its first line, and its assertions are unverified.
A checkbox needs a test that passes (F1); 5.1 stays unticked (it is).

Fix: the live-scene fixture opens a **real primary node without master**
when one exists — `TestDevice::open_real_drm_or_ignore()` (scans
`/dev/dri/cardN`) wrapped with `Device::from_file_for_tests(file)` — and
substitutes it for the socket device on the fixture's KMS owner.
`PRIME_FD_TO_HANDLE`, `ADDFB2` and `RMFB` do not require master, only a
primary node, so pool allocation should succeed while the display server
holds master; the fixture never commits. Without a node: `panic!` (R12).
If ADDFB2 turns out to need master on this kernel, that is an F8 stop
with the errno in the report. As a side effect the two pre-existing tests
will start actually running; leave their skip idiom alone (out of scope)
but report what they do.

### Major

**F4-M1 — a racy assertion in the decisive test.** `backend.rs` test:
`assert!(!scratch_ticket.poll_signaled_result(&vk_ctx).unwrap())`
immediately after `submit_one_shot_op_async` of an empty command buffer.
A no-op submission can be signalled before the poll; this will flake
(R2). Remove it — the test's evidence is the `wait` → `poll_gpu` →
`service_ready` → `drops == 1` sequence, which is deterministic.

**F4-M2 — source and scratch are `Spy` payloads while the real types
exist (F3), and the adapter's `source_key` is a free parameter.**
`read_scanout_region_for_managed_source` reads the scanout and then
records the outcome against whatever key the caller passes; nothing ties
the key to the buffer that was read. With F-2's
`register_managed_scanout_bo` and F-3's `Storage::into_managed`, the test
can be real: register the live-scene pool's bo as the managed source
(that is what `read_scanout_region` reads), adopt a real
`StorageAllocation` as the Composite scratch, and have the adapter take
the source key from the pool slot (`managed_key()` of the bo it reads)
rather than a parameter. Then "source retention is not extended by
scratch use" is asserted on the real scanout allocation.

**F4-M3 — 5.3/5.5 wiring stopped under F8; correct call, now needs its
own session.** `engine.rs`, `frame_builder.rs`, `scene.rs`,
`drain_pending_pool_releases` and `PendingAck` still have no diff; the
adapters exist but nothing at the seams calls them. That is the point of
Task 5 and it is inert under R8 (the managed-route branch is never taken
in production), so it is safe to write: `prepare_retirement_batch` at the
managed-route branch of scene submission, `PendingAck` carrying the
`CoreRetirementBatch`, `cancel_pre_submit_batch`/`freeze_uncertain_batch`
on the two failure exits, `drain_pending_pool_releases` consulting the
service before returning a managed bo. Split out as **F-4c**, after
F-4b, so it is reviewed on its own.

### Minor

**F4-m1** — `read_scanout_region_for_managed_source` carries
`#[allow(dead_code)]`; F-4c's wiring removes the need. The
`ScanoutReadSelection` visibility widening is fine.

## What was verified and holds

- M-23 (all four): `grep poll_signaled_result_opt` empty; `drop_counter`
  and its `Drop` read `cfg(test)`; `ValidatedGpuBatch` `pub(super)` with
  `validate_gpu_batch`/`commit_gpu_batch` narrowed to match (R1 fix,
  recorded).
- `prepare_retirement_batch` unwinds every reserved lease/obligation on a
  mid-loop failure; `cancel_prepared_entries` logs instead of `let _`.
- `submit_one_shot_op_async`: pre-submit failures free the CB, a queued
  CB is never freed (pool owner's lifetime), fence is the ticket's.
- Fold-back: honest about the unconfirmed test and the F8 stop; the
  coverage regression (F4-B1) is not mentioned.

## What F-4b must do

1. F4-B1 — `Option<Arc<VkContext>>` with `cfg(test)`-only `None`; the
   eight tests deterministic again.
2. F4-B2 — live-scene fixture over a real primary node without master;
   `panic!` without one.
3. F4-M1 — remove the racy assertion.
4. F4-M2 — real managed source (pool bo) and real scratch storage; the
   adapter derives the source key from the bo it reads.
5. Fold back; run the decisive test on this box and paste it green.

Then **F-4c**: F4-M3 (5.3/5.5 wiring at the seams, inert under Legacy)
and F4-m1.
