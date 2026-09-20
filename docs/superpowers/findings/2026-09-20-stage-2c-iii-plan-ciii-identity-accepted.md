# Stage 2c-iii, plan Ciii-identity — ACCEPTED (2026-09-20)

**Result:** both tasks implemented, verified and committed; thirteen mutations
applied by line by the coordinator, all thirteen caught; hardware gate
**313/313**. Commits `b955a5f5` (task 1) and `2ce060f8` (task 2), on
`feat/phase-c0-atomic-kms-migration`.

## What this plan was for

It was not unflip work. It repaired an assumption of stages 2c-i and 2c-ii:
that there is one device. Each `DeviceCommitOwner` mints its `CommitId`s from
its own allocator starting at 1, so two owner devices issue the same numbers,
while the backend, the resource service and the scene all correlated by that
bare number. Device A's cached `HardwareComplete` was consumed by device B's
commit of the same number; B's terminal events reached A's Present
dispositions; B's retirement published A's pending direct frame and released
its pins; B's milestones accepted, applied and removed A's damage transaction
and owner buffers. Release-before-proof, across devices.

The plan was born by splitting plan Ciii at its revision 11 (user decision
after ten review rounds): four consecutive rounds had each found one more
consumer of this same family, and keeping the two subjects together meant
re-reviewing the unflip every time the identity contract moved.

## Task 1 — commit identity is device-qualified (`b955a5f5`)

A `CommitKey { device, commit }` is now the only thing a correlating consumer
takes, so a site that still correlates by number **does not compile**. That
type, not the site list, is what ended the per-round discovery. Converted:
`CommitResourceConsumer` and its completion cache, members and reserved
retirements; the releasing and rejected resource matches; both present
disposition scans; the owner-event routing; the pending direct frame's identity
and every one of its milestone consumers; the scene's damage transactions and
owner-buffer scans; admission and handoff. Owner-local records that never leave
their device keep the plain id.

Five tests, each driving two owners whose commits collide numerically.

| Mutation | Test that failed |
| --- | --- |
| T36 completion cache keyed by `CommitId` alone | `c0_conv_ciii_id_equal_commit_ids_do_not_cross_devices` |
| T37 releasing-resource match without the device | `c0_conv_ciii_id_equal_commit_ids_do_not_cross_devices` |
| T40 `FailedBeforeSubmit` disposition scan without the device | `c0_conv_ciii_id_foreign_terminal_leaves_a_pending_present_alone` |
| T40c `Presented` disposition scan without the device | `c0_conv_ciii_id_foreign_presented_leaves_a_pending_present_alone` |
| T41 pending frame matched by `CommitId` alone | `c0_conv_ciii_id_foreign_milestones_leave_a_pending_direct_frame_alone` |
| T41b unknown-completion frame match without the device | `c0_conv_ciii_id_foreign_terminal_leaves_a_pending_present_alone` |
| T44 retirement publishes without matching the key | `c0_conv_ciii_id_foreign_milestones_leave_a_pending_direct_frame_alone` |
| T42 damage map keyed by `CommitId` alone | `c0_conv_ciii_id_foreign_milestones_leave_a_damage_transaction_alone` (GPU) |
| T43 `accept_owner_buffer` without the device check | the same (GPU) |

T40c and T41b are sites the coordinator found that the plan had not enumerated:
the disposition scan exists **twice**, and the frame match likewise. When a site
is fixed, look for its siblings.

**Sent back twice, both times for the same defect class: the tests probed a
collision that was not one.** The first tranche exercised only the branch where
nothing matched, so T37 survived; the second did not make the `CommitId`s
numerically equal, so T40 survived. The implementer self-reported the third gap
(T43 had no owner-buffer assertion), which was right.

## Task 2 — one device's conductor is its own (`2ce060f8`)

Evidence only: **no production change was needed**. With identity qualified in
task 1, the conductor, gate, owner and direct-group halves of §6.2 already
held. Three tests on a two-device seeded fixture, not the live Vulkan one,
which asserts exactly one device.

| Mutation | Test that failed |
| --- | --- |
| T28 owner event batch routed to every conductor | `c0_conv_ciii_id_foreign_milestones_leave_a_damage_transaction_alone` |
| T29 bound violation closes every device's gate | `c0_conv_ciii_id_devices_are_independent` |
| T30 layout generation bumped on every conductor | `c0_conv_ciii_id_conductor_state_is_per_device` |
| T31 single-device precondition dropped | `c0_conv_ciii_id_direct_group_never_crosses_devices_vulkan` |

**T28 is caught by task 1's damage test, not by the batch case of
`devices_are_independent` that the plan's table pairs it with.** The invariant
is proven; the pairing is not. A future change to that damage test could remove
the only evidence without any named test going red.

Deliberate device-blind paths, reported and left alone: the layout-change
fanout across devices, and the backend-wide direct capacity and group state
(one direct unit by design).

## Gate

Software (coordinator, outside codex): fmt; clippy default, `tcp-transport`,
`xdmcp`; `c0_conv_ciii_id_` 8/8 with `--include-ignored` in debug and release;
`c0_conv_ci_` 38, `c0_conv_cii_` 26, `c0_conv_cir_` 7; `c0_adm` 129; `c0_2ci`
180/0/21; `--lib` 1939/0/144 at `2ce060f8`; `cargo check --workspace` for Linux
glibc, Linux musl and FreeBSD.

**Hardware gate (2026-09-20, user on tty, GPU free), run at `e3209221`, which
also carries plan Ciii's task 1:** `render_acceptance -- --ignored` **164/164**;
`c0_2ci -- --ignored` **21/21**; the library's other ignored tests **128/128**.
**313/313 in total**, against the 306/306 of the Cii acceptance plus the seven
new ignored tests (two here, five in plan Ciii's task 1).

## Two things on the record, not swept

1. **An unexplained `--lib` failure.** The first run after the implementer's own
   run reported 1938 passed and 1 failed, and the failing name was not
   captured. Fifteen further runs — ten clean, five under a parallel release
   build — did not reproduce it, and the plan's own filter passed ten for ten.
2. **The ignored library tests must be run in the documented buckets.** Running
   `--lib -- --ignored` as a single invocation fails
   `c0_2ci_managed_scanout_flip_accepted_drm`,
   `c0_2ci_managed_scanout_out_fence_resolves_drm` and
   `c0_2ci_sink_gamma_gate_four_states_master_drm`; each passes on its own and
   all three pass in the `c0_2ci`-filtered bucket. DRM master is held per
   process and an earlier test in the same binary does not release it. The
   three-bucket split the gate has always used is load-bearing, not cosmetic —
   this is the first time it was written down.

**Plan Ciii-identity is accepted.** Stage 2c-iii continues with plan Ciii (the
unflip, its route and the hardware run; task 1 landed at `e3209221`), and still
owes the copied-route plan before the stage is complete.
