# Stage 2c-iii, plan Ciii (unflip) — ACCEPTED (2026-09-22)

**Result:** six tasks implemented, verified and committed; the hardware test of
spec §6.4 **passes on card1 with DRM master**, its two mutations T32 and T33
fail it at the P3-2 and P3-3 assertions; hardware gate **359/359** at the
acceptance tip. Commits `e3209221` (task 1), `2d1ef16f` (task 2), `bf53893e`
(task 3), `59c72ec6` (task 4), `13081c99` (task 5), `a8dbaec7` (task 6, the
card1 test), and the two production fixes the card1 run forced, `c233e5f1`
(F-T6-3) and `e29f09fd` (F-T6-4), on `feat/phase-c0-atomic-kms-migration`.
Plan revision 14, spec 2c-iii revision 4 (§6.4 as annotated 2026-09-20).

Between task 5 and task 6 sit plan Cfb's five tasks
(`2026-09-22-stage-2c-iii-plan-cfb-accepted.md`): Task 6 stopped on
2026-09-21 with an F8 (`2026-09-21-stage-2c-iii-plan-ciii-task6-f8.md` —
the direct framebuffer never entered the ledger, so P3-3 was unreachable by
construction) and resumed once Cfb was accepted at fixture level.

## What this plan was for

The unflip on the Owner route: one acyclic request entry for every production
cause; readiness on the exit-retirement slot, every output's retained composed
framebuffer and a materialized shadow; dispatch in its own module replacing the
complete plane set of the device in one transaction with the current direct
resources as the old state under `ExitRetirement`; the scoped, cumulative
return path (invalidate once, repaint every output in full, hold re-entry until
each output has proven its repaint); cursor and gamma untouched; the legacy
primary/unflip sinks never entered on an `Owner` device; and the single
hardware run of spec §6.4 that gives `KmsRelease` its real-completion evidence.

## Tasks 1–5 (before the F8)

| Task | Commit | Mutations |
| --- | --- | --- |
| 1 one request entry, complete readiness | `e3209221` | nine applied by line, nine caught |
| 2 unflip dispatch, own module | `2d1ef16f` | ten applied; eight caught, T9 equivalent (verified), one replaced |
| 3 the return path, scoped and cumulative | `bf53893e` | nine applied, nine caught (T21 needed a second round in its narrow form) |
| 4 cursor and gamma untouched | `59c72ec6` | **applied at acceptance:** T22 (extra plane property), T23 (extra CRTC property) — both caught by `c0_conv_ciii_unflip_carries_no_cursor_or_gamma_vulkan` (`backend.rs:57706`) |
| 5 route selection and exclusivity | `13081c99` | **applied at acceptance:** T26 (force the legacy branch at the `maybe_composite` unflip fork, `backend.rs:22512`) caught by `c0_conv_ciii_owner_device_issues_no_legacy_primary_write_vulkan`; T25 (force legacy at `request_direct_unflip`, `:2648`) caught by task 1's `c0_conv_ciii_every_unflip_cause_reaches_the_owner_request_vulkan` — the request site sets flags, the write site is the fork above; T34 (recorder after the permit, `page_flip.rs:141`) caught by `c0_conv_ciii_sink_recorder_counts_entry_before_the_permit` |

Tasks 4 and 5 were committed with terse messages and no mutation ledger; the
ledger above is the coordinator's, run at the acceptance tip.

**Recorded, not swept:**

- **T24 at the direct producer's retirement fork (`retire_direct_output`,
  `backend.rs:3860`: owner `retirement_wake` vs legacy
  `submit_queued_direct_successor`) survives every `c0_conv_` test.** That fork
  is plan Cii's, not this plan's enumeration, but the legacy-sink recorder
  test is the cross-plan observer and no test queues a successor at a
  retirement on an Owner device. A coverage gap for stage 3.
- **T27 (read another device's transport state at a legacy write site) is
  unobservable on the one-device fixtures:** an unknown device has no gate and
  answers `true`, exactly as the Legacy device does, and an Owner device never
  reaches the site. Needs a two-device fixture with one Owner and one Legacy.

## Task 6 — the card1 run (`a8dbaec7`, `c233e5f1`, `e29f09fd`)

`c0_hw_ciii_owner_route_on_card1_drm` establishes `Owner` on card1 (writer
coverage over every scanout BO, real fd-inheriting `KmsIoExecutor`, real
`DeviceCommitOwner`, `ResourceService` and `DrmCleanupRegistry`), then drives
with bounded waits: composed → direct A (Vulkan-rendered PRIME import) →
direct A again while A is current → unflip. Codex wrote it and did not run it
(decision 10). The coordinator's runs found four things:

| | What | Where fixed |
| --- | --- | --- |
| F-T6-1 | the test required `Presented` for the two non-Present commits; the owner never emits it for an empty `present_event` | test (codex) |
| F-T6-2 | the card1 preflight mapped the live-scene fixture's placeholder key `0:0`; the target window's null test storage segfaulted the ICD in the unflip's shadow copy (`VK_NULL_HANDLE` image, confirmed with the validation layer); the unflip wait had to include the release, which lands one loop pass after the routed retirement | test (coordinator) |
| **F-T6-3** | **production:** the dispatch reserved `OrdinaryRetirement` whenever `current_resources` was non-empty, so a direct entry over the composed current leaked the reservation (nothing consumes it at `CompletionRetired` without an old `Current` role) and every later direct offer waited for ever on `OrdinaryRetirementOccupied` — the same-source offer of step 3 reported `NothingAdmissible` | `c233e5f1`: reserve iff `Current` is occupied; cancel an unconsumed reservation at retirement; two tests, two mutations caught |
| **F-T6-4** | **production:** `CommitResourceConsumer::on_available` had no production caller — the owner sites and the step discarded `service_completions`' keys — so on the Owner route a retired commit's `CommitResources` (framebuffer lease, present pins, source/fallback leases, its retirement role) were never released; the unflipped allocation stayed in the service. Every "released" claim of plans Cfb and Ciii had gone through a test-only `on_available` call | `e29f09fd`: the step's Phase A feeds the keys to `on_available` and calls it with none after a routed retirement; a freed role wakes admission once per pass; the test-only helpers are gone and seven tests reach the release through production; three tests, three mutations caught |

F-T6-3 and F-T6-4 are the class the memory "invariants need a production
caller" names, met a second time: the hardware-free evidence for a release
path must name the production call that performs it, and T5 of plan Cfb was
written after the sequence it needed (offer, then retirement) rather than the
one production runs (retirement, then offer).

**Card1 results at the tip** (`cargo test -p yserver --lib
c0_hw_ciii_owner_route_on_card1_drm -- --ignored --test-threads=1`, tty, DRM
master, HDMI-2 1920×1080@60): passes twice.

| Mutation | Result |
| --- | --- |
| T32 drop the displaced buffer's registration (`commit.rs:726`, `if false`) | fails at "displaced composed allocation must be registered before first direct completion" (`backend.rs:65997`) — P3-2 |
| T33 register the retained allocation (`commit.rs:726`, `if true`) | fails at "retained same-source allocation must register no KmsRelease" (`backend.rs:66081`) — P3-3 |
| Cfb F29 at the admission dispatch site (`admission.rs:1184`, allocations cleared) | fails at "unflip must register the displaced direct framebuffer for KmsRelease" (`backend.rs:66138`) |

## Gate at the acceptance tip (`e29f09fd`)

Software: fmt; clippy default, `tcp-transport`, `xdmcp`; `cargo check
--workspace` for Linux glibc, Linux musl and FreeBSD; `c0_conv_` **140/140**
with `--include-ignored` in debug and release; `c0_adm` 129; `c0_2ci`
180/0/21; `--lib` **1954/0/195 ×3**.

Hardware (GPU free, tty): `render_acceptance -- --ignored` **164/164**;
`c0_2ci -- --ignored` **21/21**; the library's other ignored tests
(`--skip c0_2ci --skip render_acceptance --skip c0_hw_`) **173/173**;
`c0_hw_ciii_owner_route_on_card1_drm` **1/1**. **359/359 in total.**

**Plan Ciii is accepted.** Stage 2c-iii's conversion of the three producers on
the Owner route — composed (Ci), direct (Cii, Cfb) and unflip (Ciii), with
device-qualified identity (Ciii-identity) — has its hardware evidence.

## Carried

- The copied-route plan (the composed route copied for the Owner device — spec
  2c-iii §4.6) is still owed to stage 2c-iii.
- The Ci F8 stops stay open: the Legacy dormancy bug, the missing restore
  `TerminalState`, device loss without an owner signal.
- Cfb: the managed-storage access adaptation (§2.5); F31's per-site coverage.
- The two coverage gaps above (T24 at the direct retirement fork, T27 on a
  two-device fixture).
- `docs/phase-c0-upstream-fixes-revalidation.md` after stage 4.
