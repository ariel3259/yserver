# Stage 2b addendum — Owner CRTC clocks are probed in production

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for GPU work) with `< /dev/null`. Hard rules: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only the filters `c0_2b_add_`, `c0_3aii_`, `c0_3a_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm`, each by its own command with `--include-ignored`; never `_drm` tests (including the one this plan edits), `c0_hw_` tests, `render_acceptance`, `c0_2ci` (known intermittent hang, `docs/known-issues.md`), an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Stop with the tree dirty when done. **Do not ask for approval inside a run** — if something this plan states does not hold in the code, stop and report it (F8); never silently substitute a test shape or weaken an existing assertion.

**Revision 6 (2026-09-23, coordinator)** — Task 1 F8 before editing, three
plan gaps, all verified: (1) two Task 1 tests needed Task 2's DPMS wait —
the uncertain-probe test, the stale-timeout run and the passed-validation
re-validation run move to Task 2 with I-1a; (2) a sequence-queue lease also
blocks `acquire_probe` — added to I-3's release points with a test run;
(3) production has **no Owner activation site yet** (C0-R8: production stays
Legacy until stage 5; `install_admission_conductor` and
`try_finish_legacy_transport` have no production caller) — I-2 is anchored to
one activation step that every conductor-install entry goes through, and
stage 5 inherits it.

**Revision 5 (2026-09-23, coordinator)** — codex round 4
(`../findings/2026-09-23-2b-addendum-clock-probe-plan-review-round4.md`:
**0 blocking**, 1 major, verified and APPLIED, not re-reviewed): a *passed*
validation is not a slot release — `consume_validation` hands its lease
straight to the live commit. The validated commit proceeds only if every
clock it needs is ready; otherwise the validation is abandoned (a real
release, so a promotion point) and the work re-validates after the probe
without consuming its attempt (I-3).

**Revision 4 (2026-09-23)** — codex round 3
(`../findings/2026-09-23-2b-addendum-clock-probe-plan-review-round3.md`:
1 blocking, 1 major, both APPLIED): "stale" discards only a *result* — an
uncertain outcome of a superseded probe still stalls the executor and takes
the completion-loss route (I-3, B-1); the waiting probe is promoted at every
slot release, validation resolution included, with its own test (M-1).

**Revision 3 (2026-09-23)** — codex round 2
(`../findings/2026-09-23-2b-addendum-clock-probe-plan-review-round2.md`:
1 blocking, verified and APPLIED): I-1a now names the executor-level barrier
a timed-out probe gets — the same one every host call gets
(`terminalize_unknown` → `Stalled` + `request_termination`; `tick` reaps →
`Reaped` with a `ReapProof`) — and separates it from the lifecycle's logical
`Poisoned`; the timeout test asserts the barrier, not only the disposition.

**Revision 2 (2026-09-23)** — codex round 1
(`../findings/2026-09-23-2b-addendum-clock-probe-plan-review-round1.md`:
1 blocking, 2 major, all verified in the code and APPLIED): an uncertain
probe outcome (timeout, IPC loss, executor failure) is not an ordinary
failure — it keeps the slot and follows `COMMIT-5`/`ExecutorStalled` (new
I-1a, B-1); I-2 names the CRTC set a DPMS can touch and a missing record is
an internal error, never a partial commit (M-1); I-3 now says who holds a
probe that cannot take the slot yet and who promotes it (M-2).

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

**I-1a. An uncertain probe is a stalled executor, not a failed probe.** A
probe whose outcome is `Unknown` (timeout, IPC loss, executor failure — the
owner emits `ClockProbeResolved { outcome: Unknown(..) }` and deliberately
keeps the slot) follows C.0 `COMMIT-5`/`ExecutorStalled`: it reaches the same
production route a lifecycle completion loss takes (Task 8:
`lifecycle_report_completion_loss` → Table U → `Poisoned`), so a DPMS waiting
on that clock becomes logical-only (Task 8's poisoned rule), no commit is
begun behind the held slot, and nothing pretends the slot is free. If no such
route can carry a probe outcome, **stop with an F8**.

Two layers, each with one owner, neither new: **(a) the executor** owns the
helper process, its alias and the fd lifetime. A probe is a host call like a
commit, so its watchdog expiry already takes the generic path in
`kms/executor/mod.rs`: `terminalize_unknown` sets `ExecutorState::Stalled`
and calls `request_termination`, and `tick` reaps the helper to
`ExecutorState::Reaped`, recording the `ReapProof`; IPC loss and helper exit
reach `Stalled`/`Reaped` the same way. This addendum does not change that
path and must not add a probe-specific one. **(b) the device owner** keeps
the probe's slot lease held (it is released only by device replacement, 3d's
recovery exit, exactly as for a commit whose completion is unknown), and the
lifecycle arbiter records the logical `Poisoned` state. Logical `Poisoned`
does not claim the helper is gone; the reap proof does.

**I-2. Every served Owner CRTC has a clock before its first lifecycle
commit.** The DPMS description is built from the device's served outputs
(`platform.outputs` filtered by the lifecycle projection,
`admission.rs` ~1095), so the set is: the CRTC of every served output of an
Owner device. Each has a clock record at the device's current lifecycle
epoch and topology generation without depending on a client having issued a
RANDR query. A CRTC that first becomes a served output later (client modeset,
hotplug) is 3b/3c's installation site and carries the same obligation. A
lifecycle description that names a CRTC with **no** clock record is an
internal inconsistency: logged at error level with the CRTC, never
dispatched, never sent with a partial clock map. **Anchor (rev 6):** production has
no Owner activation yet (C0-R8), so the anchor is the **activation step**:
one function that every admission-conductor installation goes through —
`install_admission_conductor` (the production entry stage 5 will call) and
both `#[cfg(test)]` variants — which installs a clock record for every served
CRTC of the device and queues its probe. A probe cannot start while the
owner holds its Legacy permit (`begin_clock_probe` refuses with
`LegacyTransportActive`), so the permit's clearing
(`try_finish_legacy_transport`) is a promotion point (I-3). The RANDR refresh
remains the installer of genuinely new epochs. **Carried to stage 5:** the
activation calls this step; stage 5 must not add a second install path.

**I-3. The probe gets the slot.** The probe uses the device's commit slot,
and `begin_clock_probe` acquires it immediately or fails
(`slot.acquire_probe`, `owner/slot.rs` ~150): when the slot is occupied there
is no pending probe. So an installed clock whose probe could not start is
held as a **waiting probe key** by the owner (or the backend beside it —
implementer's choice, named in the report), and the waiting key is
**promoted when the slot frees, before any new commit is begun on that
device**, composed or lifecycle. A stream of composed frames cannot starve it:
it is sent between two composed commits at the latest. When a new clock epoch
replaces one whose probe is waiting or in flight, the old key is dropped from
the waiting set, an in-flight result for it is discarded as stale when it
arrives (C.0), and the new epoch's key is waiting. **Stale applies to results
only:** a superseded probe's success or explicit rejection can never resolve
the new epoch's clock, but an **uncertain** outcome of that superseded host
call (timeout, IPC loss, helper exit) is still an executor fact — it takes
I-1a's two layers exactly as a current one does (the executor stalls and
reaps; the lifecycle reaches logical `Poisoned`). **Every slot release is a
promotion point:** a composed or lifecycle commit's retirement, a
validation that is rejected or abandoned, a sequence-queue lease's release,
the Legacy permit's clearing, and a probe's own resolution. A
**passed** validation is not a release: its lease passes directly to the
validated live commit (`consume_validation`). That commit may proceed only if
every clock its completion context needs is ready at that moment; if one of
them is not (a new epoch was installed on one of its CRTCs while it
validated), the validation is abandoned — which releases the slot and is a
promotion point — and the same work re-validates after the probe resolves,
without consuming its attempt and without a `Rejected` outcome (I-4). A
waiting probe for a CRTC the validated commit does not need waits for that
commit's retirement; at each, a waiting probe key is sent before the next queued
commit or validation of that device begins, whichever queue (composed
admission or lifecycle driver) holds it. Task 1 names each release site in
its inventory.

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

## Task 1 — the probe in production (I-1 without its uncertain outcome, I-2, I-3, I-6)

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
| `c0_2b_add_clock_is_probed_on_install` | an Owner device activated through the activation step (I-2): exactly one `ClockProbe` host call per served CRTC reaches the executor, only after the owner holds no Legacy permit; the stub's `ProbeAccepted` reply routed through the production event path makes the clock `KernelSequence` with that reference | **P1** remove the probe start |
| `c0_2b_add_clock_exists_without_a_randr_query` | the same setup with no RANDR enumeration: the clock record exists and is probed | **P2** install only from the RANDR path |
| `c0_2b_add_failed_probe_is_not_retried_in_the_epoch` | stub replies `EOPNOTSUPP`: clock stays `Unresolved`; further ticks and a refresh at the same epoch send no second probe; a genuinely new epoch probes again | **P3** retry within the epoch |
| `c0_2b_add_probe_is_not_starved_by_composed_frames_vulkan` | the clock is installed while a composed commit holds the slot, and composed frames keep arriving: the waiting probe is sent at that commit's retirement, before the next composed commit begins | **P4** promote the waiting probe only when no composed work is queued |
| `c0_2b_add_new_epoch_replaces_an_in_flight_probe` | a probe in flight, then a genuinely new clock epoch for the same CRTC; the old probe then succeeds: its reply is discarded and does not resolve the new clock, and the new epoch is probed | **P4b** resolve the new clock from the old reply |
| `c0_2b_add_probe_is_promoted_at_every_release` | a clock installed while the slot is held, with a successor queued, in four runs: (i) a lifecycle validation that is rejected — the waiting probe is sent before the successor's validation begins; (ii) a validation that passes, with the new epoch on a CRTC the validated commit does **not** need — the validated commit proceeds and the probe is sent at its retirement; (iii) a sequence-queue lease — the probe is sent when the queue lease is released, before the successor; (iv) the owner still holds its Legacy permit at activation — the probe is sent when the permit clears (`try_finish_legacy_transport`) | **P4e** promote only on commit retirement; **P4g** omit the queue-release promotion; **P4h** omit the permit-clearing promotion |
| `c0_2b_add_legacy_device_never_probes` | a Legacy device: no `ClockProbe` host call, no clock state change | **P5** probe without the Owner condition |

## Task 2 — the uncertain probe, DPMS waits, refusals are named (I-1a, I-4, I-5) and the hardware test

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_2b_add_dpms_waits_for_the_clock` | Owner device, probe pending (stub holds the reply): DPMS off queues, no validation or commit host call is sent and the representative is not `Rejected`/`Deferred`; the probe reply arrives: the off validates and dispatches exactly once | **P6** begin the DPMS without waiting; **P7** never re-drive after the probe resolves |
| `c0_2b_add_dpms_after_a_failed_probe_is_readiness_closed` | probe replies `EOPNOTSUPP`, then DPMS off: no commit host call, `Deferred(ReadinessClosed)`, no `TopologyLatched` | **P8** dispatch with a partial clock set |
| `c0_2b_add_missing_clock_record_is_never_a_partial_commit` | a lifecycle description naming a served CRTC whose clock record was removed: no commit host call, the refusal is logged and never-dispatched | **P8b** drop the CRTC from the clock map and send |
| `c0_2b_add_presubmit_refusal_is_not_a_kernel_rejection` | a forced non-transient `DispatchError` at the validated-submit step (through a test seam on the owner, not by editing the classification): the terminal is never-dispatched, the representative is `Deferred(ReadinessClosed)`, never `TopologyLatched` | **P9** restore the synthesized `IoctlRejected { EINVAL }` |
| `c0_2b_add_uncertain_probe_stalls_the_executor` | the stub never replies to the probe and its watchdog expires while a DPMS waits: the executor reaches `Stalled` then `Reaped` through `tick` and yields a `ReapProof`; the production completion-loss route reaches `Poisoned`; the DPMS is logical-only; no further host call is sent and the owner's slot stays held | **P4c** treat `Unknown` like an explicit errno (release the slot and mark unresolved) |
| `c0_2b_add_stale_probe_timeout_still_stalls` | a probe in flight, then a genuinely new clock epoch for the same CRTC, then the old probe times out: the executor stalls and reaps and the lifecycle reaches `Poisoned` — the timeout is not discarded as stale | **P4d** discard the old timeout as stale |
| `c0_2b_add_passed_validation_waits_for_a_needed_clock` | a DPMS validation passes while a new epoch is installed on a CRTC the validated commit needs: the validation is abandoned (a slot release, the probe is sent), then the same DPMS re-validates and dispatches once, its attempt not consumed | **P4f** let a passed validation proceed with a not-ready clock it needs |

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
