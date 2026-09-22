# Stage 2c-iii, plan Cfb (direct framebuffer adoption) — ACCEPTED (2026-09-22)

**Result:** five tasks implemented, verified and committed; the plan's mutation
table applied by line by the coordinator; one post-plan regression fixed; the
software and hardware gates green at the acceptance tip. Commits `121f843a`
(task 1), `fa66fbee` (task 2), `7762a560` (task 3), `a23db854` (task 4),
`f36712ef` (task 5), `eafe06f4` (the step is Owner-only), on
`feat/phase-c0-atomic-kms-migration`. Spec revision 6, plan revision 5.

**Read with the Ciii acceptance
(`2026-09-22-stage-2c-iii-plan-ciii-accepted.md`).** The card1 run of Ciii
Task 6 found that two of this plan's claims rested on test fixtures rather
than production: the retirement release path had no production caller
(F-T6-4) and the retirement reservation leaked over a composed current
(F-T6-3). Both are fixed in `c233e5f1` and `e29f09fd`, and this plan's seven
affected tests now reach the release through the production step. This
acceptance is of the tip that includes those fixes.

## What this plan was for

Plan Ciii Task 6 stopped on an F8: the framebuffer imported for direct scanout
never entered the ledger — `take_direct_owner_resources` built
`CommitResources` with empty `allocations`, the M1 probe cache kept the only
strong owner, and `register_commit_dependencies` had nothing to compare, so
P3-3 (a retained allocation registers no `KmsRelease`) was unreachable by
construction. This plan gave the Owner route its direct framebuffer ownership:
the `DrmCleanupRegistry` installed beside the `ResourceService`; adoption at
`managed_prepare_direct_candidate` after `fb_ready` through
`direct_owner::adopt_framebuffer` under a `DirectLeasePermit`; the probe split
into the real import and the test-only validation; a checked backing serial on
the drawable and a live-checked M1 index `(DrawableId, backing serial,
topology generation) → ManagedAllocationToken` (a `Weak`); the two-phase
service step (staged zero edges → index removed → revalidate → registry
cleanup); the pending-cleanup owner with its charge by origin; the lease
carried by value through all six roles and every rollback row of spec §3.3.
Legacy is untouched: the fork is the one Task 1 added and the step is Owner-only.

## Per-task record

| Task | Commit | Sent back | Mutations caught | Equivalent / structural |
| --- | --- | --- | --- | --- |
| 1 registry beside the service, adoption at preparation | `121f843a` | once (permit test gaps: same-role stale serial, foreign device/incarnation) | F1, F9, F9a ×3, F9b, F13, F17 | F2, F9c, F35; F14 and F34 not mechanically applicable |
| 2 pending-cleanup owner | `fa66fbee` | F8: the late-failure test needs Task 4's handoff → plan rev 5 moved it | F19, F20, F21 | F22, F23 |
| 3 backing serial, live-checked index, two-phase step | `7762a560` | twice on GPU evidence (F-T3-1 ICD poisoning by repeated real-DRM contexts; F-T3-2 four real-import tests unignored on the shared context) | F4, F4b, F6, F7, F7′, F12b, F32, F33 | F5, F12a |
| 4 handoff into the ledger, every ownership row | `a23db854` | — | F10, F15, F20b, F30 | F16, F27 |
| 5 evidence closure, Ciii re-entry check | `f36712ef` | — | F20b′ | — |
| coordinator, after the plan | `eafe06f4` | — | the step's completion pass regardless of the conductor (`c0_2ci_scene_managed_shared_compose_vulkan`); dropping the restored `before_block` completion call (`c0_2ci_progress_no_composition_on_core_loop_fake_backend`) | — |
| coordinator, at acceptance | — | — | F11, F20c, F26, F28, F29 (seam) | see below |

**Mutations recorded as equivalent or unobservable at acceptance, with the reason:**

- **F3 / F18 (keep or reconstruct the cache as a strong owner).** The cache's only
  handle on the allocation is `ManagedAllocationToken` — an `AllocationKey` and a
  `Weak` — and the framebuffer is moved out of the entry at adoption (F2). A strong
  owner cannot be reconstructed without changing the token type;
  `c0_conv_cfb_eviction_leaves_a_live_allocation_vulkan` is the observation (32
  evicting inserts, the allocation still in the service).
- **F8 (count only committed leases).** There is one count, `live_use_count`,
  with no committed/uncommitted distinction in the type; a lease that is not
  counted is a lease whose drop never stages, which F7 already catches through
  `c0_conv_cfb_never_dispatched_allocation_reaches_zero_vulkan`.
- **F24 (also destroy in the entry's `Drop`).** `DirectFramebufferAllocation`
  has no `Drop`; the right is consumed only through
  `discharge_file_owned(registry)`. A `Drop` that reached the device directly
  would bypass the fixture's mock I/O, so `c0_conv_cfb_cleanup_exactly_once_vulkan`
  cannot observe it; the invariant is held by the type.
- **F25 (discharge at `HardwareComplete` when no retirement recorded it).**
  Applied as a discharge over `current_resources` at the `!found` branch: it is
  a no-op, because before `CompletionRetired` the consumer holds neither the old
  state nor its obligations — they are in the ledger's `Submitted`. Both
  completion orders are covered by `c0_conv_cfb_completion_orders_keep_one_owner_vulkan`.
- **F31 (run the step before `route_owner_event_batch` at one site).** Survives:
  `c0_conv_cfb_service_step_removes_the_index_before_cleanup_vulkan` calls the
  step directly, not through the four entry points, so the plan's "four cases,
  one per site" is not what was implemented. The ordering affects only *when*
  a staged edge is consumed (the next pass — `before_block` runs every loop
  iteration), not whether. Recorded as a coverage gap, not a defect; the card1
  test drives the real loop order.
- **F29 at the admission dispatch site** (`admission.rs:1184`) survives the Cfb
  test, which drives the `managed_handle_direct_unflip` seam; it is caught by
  `c0_hw_ciii_owner_route_on_card1_drm` at the unflip registration assertion
  (Ciii acceptance).

## The post-plan regression (`eafe06f4`)

The hardware gate after task 5 found `c0_2ci_scene_managed_shared_compose_vulkan`
regressed from 21/21 (`aaac1a8f`): task 3's step ran its completion pass on
every device with a service, so the 2c-i Legacy fixture's GPU batch was retired
inside the tick at a site that never serviced completions before the plan. The
step now returns before anything unless the device has an active conductor; and
because task 3 had folded the two pre-existing Legacy completion calls
(`before_block`, `on_owner_completion_ready`) into the step, those are restored
verbatim. Codex's first cut had instead turned the 2c-i progress test's device
into Owner; the characterisation was kept Legacy.

## Fixture lessons (recorded in `docs/status.md`)

- Repeated create/destroy of real-DRM Vulkan contexts poisons the NVIDIA ICD
  after ~20 cycles (it stops enumerating the dGPU); real-DRM fixtures share one
  context under a `cfg(test)` serial, the test device is `/dev/null`, helpers
  are reaped.
- GPU tests are `#[ignore = "needs live Vulkan ICD"]` with a `_vulkan` suffix;
  four real-import tests that ran unignored in the parallel suite lost the
  device (`ERROR_DEVICE_LOST`).
- `device_lock` tests flake (~1 in 10) on "the last close releases it" through
  the fork window of the module's own helper-spawning tests
  (`docs/known-issues.md`); unrelated to this plan.
- A hardware-free "release" claim must name its production caller: this
  plan's did not (F-T6-4).

## Open

- The managed-storage access adaptation of spec §2.5 (storage is never adopted
  here; the present pin retains the source) — carried.
- F31's per-site coverage: a test that drives each of the four entry points.

## Gate at the acceptance tip

Tip `e29f09fd` (this plan plus the Ciii Task 6 fixes), coordinator, outside
codex, GPU free, from tty:

Software: fmt; clippy default, `tcp-transport`, `xdmcp`; `cargo check
--workspace` for Linux glibc, Linux musl and FreeBSD; `c0_conv_` **140/140**
with `--include-ignored` in debug and release; `c0_adm` 129; `c0_2ci`
180/0/21; `--lib` **1954/0/195 ×3**.

Hardware: `render_acceptance -- --ignored` **164/164**; `c0_2ci -- --ignored`
**21/21**; the library's other ignored tests (`--skip c0_2ci --skip
render_acceptance --skip c0_hw_`) **173/173**; `c0_hw_ciii_owner_route_on_card1_drm`
**1/1** on card1 with DRM master. **359/359 in total**, against the 313/313 of
the Ciii-identity acceptance plus this plan's and Ciii's new ignored tests.

**Plan Cfb is accepted.** The Owner route's direct framebuffer is a counted,
registry-cleaned allocation carried by value through the ledger; the same
source reuses its key; every rollback row has one owner; and the card1 run
of Ciii Task 6 — the re-entry this plan was written for — passes.

