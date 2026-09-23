# Stage 2b addendum — Owner CRTC clocks are probed in production

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for GPU work) with `< /dev/null`. Hard rules: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only the filters `c0_2b_add_`, `c0_3aii_`, `c0_3a_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm`, each by its own command with `--include-ignored`; never `_drm` tests (including the one this plan edits), `c0_hw_` tests, `render_acceptance`, `c0_2ci` (known intermittent hang, `docs/known-issues.md`), an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Stop with the tree dirty when done. **Do not ask for approval inside a run** — if something this plan states does not hold in the code, stop and report it (F8); never silently substitute a test shape or weaken an existing assertion.

**Revision 1 (2026-09-23, coordinator).** Two tasks. Found by the 3a-ii
hardware test; the user chose to fix it as its own addendum before 3a-ii's
acceptance.

**Goal:** an Owner device probes each installed CRTC clock through the
executor as C.0 requires, so lifecycle-class and event-bearing commits can
dispatch in production; a DPMS that arrives before its clock is ready waits
for it instead of being refused; a refusal by our own owner is never reported
as a kernel rejection.

**Why:** `docs/superpowers/findings/2026-09-23-owner-clock-never-probed.md` —
read it first. `begin_clock_probe`/`send_clock_probe_on` (stage 2b,
`5601bdf3`) have no production caller, so no clock ever reaches
`KernelSequence`; the DPMS off is refused by `validate_completion_context`
and `admission.rs:1485` reports that refusal as `IoctlRejected { EINVAL }`.

**Authority:** C.0 (`docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`)
§10 clock probing (the paragraph beginning "Before admitting an event-bearing
commit on a newly installed active hardware CRTC or clock epoch") and the
clock-source row of its §6 table; the stage 3a design
(`docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`)
§3.7 (failure edges), whose `FailedBeforeSubmit` rules assume a submission
that reached the kernel.

## The invariants

**I-1. Installed means probed.** Whenever production installs a clock record
for a CRTC of an Owner device (today `refresh_present_crtc_clock_epochs`;
inventory every production installer first), the owner serializes exactly
one `GET_SEQUENCE` probe for that `(incarnation, hardware CRTC, clock epoch)`
through the executor, and the reply is routed back through the production
owner-event path so `resolve_clock_probe` runs. A successful current reply
makes the clock `KernelSequence` with its reference; `EOPNOTSUPP`, any other
explicit errno or a malformed reply leaves it `Unresolved` and permits **no
retry within the same clock epoch** (C.0). A stale reply is discarded.

**I-2. Every served Owner CRTC has a clock before its first lifecycle
commit.** The clock record for each active CRTC of an Owner device exists at
the device's current lifecycle epoch and topology generation without
depending on a client having issued a RANDR query. If the only installer is
the RANDR enumeration path, the implementer adds the installation to the
Owner device's own setup path (where the executor/owner pair is installed)
and keeps the RANDR path as a refresh. If no such setup site can be named in
production code, **stop with an F8**.

**I-3. The probe gets the slot.** The probe uses the device's commit slot
(`slot.acquire_probe`). A pending probe is sent at the first moment the slot
is free and **before** any lifecycle-class or event-bearing commit on that
device is begun; a stream of composed frames cannot starve it indefinitely
(it is sent between two composed commits at the latest).

**I-4. DPMS waits for its clock.** A lifecycle DPMS whose expected-completion
CRTCs include one whose clock is not yet resolved (probe not started, pending
or in flight) is **not begun**: the lifecycle driver keeps it queued without
consuming the attempt, without a `Rejected` outcome and without closing
readiness, and re-drives it when the probe resolves. If the probe resolves
`Unresolved` (failure), the DPMS is not dispatchable in this clock epoch: its
representative becomes `Deferred(ReadinessClosed)` and the reason is logged
once at error level naming the CRTC and the probe outcome. No path sends a
lifecycle commit with an empty or partial clock set: `clocks` covers every
expected-completion CRTC or the commit is not begun (the silent `filter_map`
drop in `lifecycle_submit_validated_topology` goes).

**I-5. Our refusals are not kernel rejections.** In the two lifecycle entry
points (`admission_dispatch_topology`, `admission.rs:1224`, and
`lifecycle_submit_validated_topology`, `admission.rs:1485`) a `DispatchError`
from the owner is never turned into `IoctlRejected`: `TopologyLatched` is
reachable only from an errno the executor reported for a real ioctl. A
transient refusal (clock not ready, slot already in flight) follows I-4's
wait; any other `DispatchError` is an internal inconsistency: log it at error
level with the error value, and report it as a never-dispatched failure
(`FailedBeforeSubmit(NeverDispatched(..))`, representative
`Deferred(ReadinessClosed)`) — the 3a design's rule for a non-attributable
failure. Kernel `EINVAL`/`EOPNOTSUPP` classification (Task 8) is unchanged.

**I-6. Legacy is untouched.** A device without an admission conductor
(Legacy) performs no probe and no new ioctl; its behaviour and client bytes
are unchanged.

## Recorded, carried

- C.0 also requires a new probe after VT reacquire, administrative reprobe,
  identity-changing hotplug and device replacement. Those transitions are
  executed on Owner in 3b/3c, which **must** start the new epoch's probe at
  their installation sites (same obligation shape as the output
  reconciliation hook). This addendum covers initial installation and a new
  clock epoch from the existing refresh.
- The 3a design's `Deferred(ReadinessClosed)` retries only when the device
  returns to `Ready`, and no 3a production code produces that input; I-5's
  never-dispatched path therefore parks the DPMS until a newer request. That
  is the 3a design's rule, not changed here; 3d's recovery exits own the
  return to `Ready`.

## Task 1 — the probe in production (I-1, I-2, I-3, I-6)

**Step 1 — inventory before editing.** Every production site that installs,
invalidates or refreshes an Owner clock record; every production site that
begins, sends or dispatches an owner commit (so the probe's send point is
chosen beside them); and every fixture that installs a clock or its reference
by hand. Report the list.

**Step 2 — implement.** Tests (names are exit criteria), each on an Owner
stub-executor fixture driven through production entries (no
`install_clock`/`install_reference`/`clock_mut` in these tests):

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_2b_add_clock_is_probed_on_install` | Owner device set up through its production path: exactly one `ClockProbe` host call per active CRTC reaches the executor; the stub's `ProbeAccepted` reply routed through the production event path makes the clock `KernelSequence` with that reference | **P1** remove the probe start |
| `c0_2b_add_clock_exists_without_a_randr_query` | the same setup with no RANDR enumeration: the clock record exists and is probed | **P2** install only from the RANDR path |
| `c0_2b_add_failed_probe_is_not_retried_in_the_epoch` | stub replies `EOPNOTSUPP`: clock stays `Unresolved`; further ticks and a refresh at the same epoch send no second probe; a genuinely new epoch probes again | **P3** retry within the epoch |
| `c0_2b_add_probe_is_not_starved_by_composed_frames_vulkan` | composed frames submitted back to back while a probe is pending: the probe is sent no later than between two composed commits | **P4** send the probe only when no composed work is queued |
| `c0_2b_add_legacy_device_never_probes` | a Legacy device: no `ClockProbe` host call, no clock state change | **P5** probe without the Owner condition |

## Task 2 — DPMS waits, refusals are named (I-4, I-5) and the hardware test

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_2b_add_dpms_waits_for_the_clock` | Owner device, probe pending (stub holds the reply): DPMS off queues, no validation or commit host call is sent and the representative is not `Rejected`/`Deferred`; the probe reply arrives: the off validates and dispatches exactly once | **P6** begin the DPMS without waiting; **P7** never re-drive after the probe resolves |
| `c0_2b_add_dpms_after_a_failed_probe_is_readiness_closed` | probe replies `EOPNOTSUPP`, then DPMS off: no commit host call, `Deferred(ReadinessClosed)`, no `TopologyLatched` | **P8** dispatch with a partial clock set |
| `c0_2b_add_presubmit_refusal_is_not_a_kernel_rejection` | a forced non-transient `DispatchError` at the validated-submit step (through a test seam on the owner, not by editing the classification): the terminal is never-dispatched, the representative is `Deferred(ReadinessClosed)`, never `TopologyLatched` | **P9** restore the synthesized `IoctlRejected { EINVAL }` |

**Existing tests.** Fixture tests that install clocks by hand may keep doing
so where they test something else; the 3a-ii DPMS tests that exercise
dispatch (`c0_3aii_*` that send a lifecycle commit) must pass with clocks
obtained through production (Task 1's path) — change their setup, never
their assertions. If one cannot, F8.

**The hardware test.** `c0_hw_3a_dpms_owner_on_card1_drm` must obtain its
clock through the production path (Task 1), wait for the probe before the
first DPMS, and fail with the probe's outcome if the probe does not succeed.
Written and compiled, **never run**; report the unchanged tty2 command line.

## Gate

`cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings` in default,
`--features tcp-transport` and `--features xdmcp`; each permitted filter by
its own command with `--include-ignored`; new tests also in `--release`
(after `cargo build --release --bin yserver`); grep new code for
`debug_assert!(self.`; `cargo test -p yserver --test compile_fail`, then
`git status --short`; `cargo test -p yserver --lib -- --skip c0_2ci` (count
and time). Mutations by line, compiled, named failing test, restored exactly
against a saved copy. Check `coredumpctl` for anything newer than the run's
start.

## Limits

Fixture level plus the edited hardware test, which the coordinator runs from
tty2 with the user's approval. Production stays `Legacy` (C0-R8). No change to
the `ACTIVE`-only DPMS shape: the hardware run after this addendum is its first
real measurement.
