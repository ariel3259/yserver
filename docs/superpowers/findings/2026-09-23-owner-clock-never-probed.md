# Finding — no Owner CRTC clock is ever probed in production

**Date:** 2026-09-23. **Found by:** the 3a-ii hardware test
`c0_hw_3a_dpms_owner_on_card1_drm` on card1 (NVIDIA 615.71.09, kernel
7.2.6), run from tty2. **Owner:** stage 2b (commit `5601bdf3`, "probe CRTC
clocks through the asynchronous owner"). **Fixed by:** the stage 2b addendum
`../plans/2026-09-23-phase-c0-stage-2b-addendum-clock-probe.md`.

## Symptom

Cycle 1's DPMS off ended `Deferred(TopologyLatched(1))`, state `Ready`: it looked
like the kernel had rejected the `ACTIVE`-only off with `EINVAL`/`EOPNOTSUPP`.

## Evidence

1. **The kernel never saw the real commit.** With `drm.debug=0x16` the off's
   `TEST_ONLY` shows `checking` and the helper's modeset additions, then
   `Clearing` — no `atomic driver check ... failed`, no `commit failed: ...`,
   and no further `Allocated atomic state` for a real commit. The kernel
   accepted the validation and received nothing after it.
2. **The EINVAL is ours.** `lifecycle_submit_validated_topology`
   (`render/admission.rs:1485`) discards the `DispatchError` of
   `begin_validated_with_context` and reports
   `FailedBeforeSubmit(IoctlRejected { errno: EINVAL })`, which Task 8 then
   classifies as an attributable kernel rejection (`TopologyLatched`).
   Temporary instrumentation (removed) printed the discarded error:
   `InvalidCompletionContext`.
3. **The cause is an empty clock set.** `validate_completion_context`
   (`owner/device.rs`) requires, for the lifecycle class, a ready
   `KernelSequence` clock for every expected-completion CRTC. Instrumented:
   `closure_expected=[205] clocks={}` — `clock_key_for_hardware_crtc(205)`
   returned nothing, and the caller's `filter_map` dropped the CRTC silently.
4. **Nothing in production probes a clock.** `begin_clock_probe` and
   `send_clock_probe_on` have no caller outside tests; a clock becomes
   `KernelSequence` only through `resolve_clock_probe`. Clock *records* are
   installed only by `refresh_present_crtc_clock_epochs`, reached from
   `randr_outputs_and_modes`. Every fixture that exercises a lifecycle or
   event-bearing commit installs the clock and its reference by hand
   (`install_clock` + `install_reference`), which is why no fixture test saw it.

## Rule broken

C.0 §10 (clock probing, spec line ~1769): "Before admitting an event-bearing
commit on a newly installed active hardware CRTC or clock epoch, the owner
serializes one `DRM_IOCTL_CRTC_GET_SEQUENCE` clock probe … No event-bearing
commit is admitted until … a genuinely new CRTC clock epoch obtains a current
result." The mechanism exists; its production caller does not.

## Consequences

- No lifecycle-class commit (DPMS today; VT, hotplug and recovery in 3b–3d)
  can ever dispatch on an Owner device in production.
- A pre-submit refusal is reported as a kernel rejection, so Task 8's
  `TopologyLatched` classification can be reached without the kernel.
- The `ACTIVE`-only DPMS shape on NVIDIA is still **unmeasured**: the commit
  never reached the driver.

## Secondary observations

- `admission_dispatch_topology` (`admission.rs:1224`) turns any
  `begin_validation_with_options` error into `Rejected { None }`, i.e.
  `Deferred(ReadinessClosed)` with the device `Quiescing`; that deferral
  retries only when the device returns to `Ready`, and no production code
  sends `ArbiterInput::DeviceStateChanged` in 3a (recovery exits are 3d):
  that `Quiescing` is permanent.
- The same shape as `on_available` in 2c-ii (F-T6-4): a mechanism tested
  only through hand-driven fixtures.
