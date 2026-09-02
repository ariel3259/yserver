# Phase C.0 kernel-path adversarial review

**Date:** 2026-08-31
**Reviewer:** Opus
**Reviewed spec:** the post-Phase-A+B rewrite of
`2026-08-26-phase-c0-atomic-kms-migration-design.md`
**Disposition:** Incorporated into the Draft after maintainer/user approval

**Subsequent revision:** coordinate concurrency, multi-CRTC admission, executor
bounds and the software MSC were refined by
`2026-09-01-phase-c0-coordinate-concurrency-adversarial-review.md`; that later
finding is authoritative where the two dispositions differ.

## Verification basis

The review was checked against:

- merged yserver baseline `master@c09358a1` / Phase A+B `fc76b743`;
- the separate Phase C.2 draft preserved in the C.1/C.2 documentation stash;
- local Linux source at `77cb8f24c2381a8abb7272d7bbdec548d6426a8a`;
- `drivers/gpu/drm/i915/display/intel_cursor.c`;
- `drivers/gpu/drm/drm_atomic_helper.c`;
- `drivers/gpu/drm/drm_atomic_uapi.c`; and
- `drivers/gpu/drm/drm_plane.c`.

The kernel audit confirms that i915 rejects non-zero cursor source panning and
that the legacy plane helper is what sets `legacy_cursor_update`, allowing the
driver's `atomic_async_check`/`atomic_async_update` cursor path. The atomic
userspace ioctl has no equivalent cursor-coordinate switch. Phase C.2 already
identifies this UAPI gap and owns its durable replacement.

## Accepted architectural corrections

1. Cursor edge placement defaults to the complete source rectangle with signed
   destination coordinates. Source cropping is an optional per-plane policy
   selected only by serialized `TEST_ONLY` qualification.
2. C.0 scopes its legacy prohibition. Persistent cursor state remains atomic,
   while a coordinate-only `MOVECURSOR` may be ordered by the sole owner on a
   qualified already-installed plane until Phase C.2 replaces or removes it.
3. Cursor/primary completion capability and atomic gamma capability are
   independent. Missing `GAMMA_LUT` reports RANDR gamma unavailable and does not
   disable C.1.
4. `KmsIoExecutor` remains provisional. Direct RTX 5060 Ti and RX 6800 XT
   measurements of all seat-active host-call classes must select one global
   executor versus in-process design before implementation planning or code.
5. `TEST_ONLY` is a non-mutating `ValidationOnly` class with an exclusive owner
   lease and explicit watchdog; it is not a live blocking commit or live slot.
6. Off transitions require real out-fence evidence, and RX 6800 XT plus RTX 5060 Ti
   run an eight-hour zero-poison soak.
7. Two-CRTC validation separates physical commits from logical per-CRTC
   retirements and gates each CRTC plus their aggregate against refresh targets.
8. NVIDIA evidence is cohort-specific. One GPU generation cannot enable all
   NVIDIA hardware; unvalidated cohorts keep the shipping software cursor.
9. Atomic `EBUSY` without an owner live record is an invariant/driver failure,
   not scheduling. Coordinate-transport `EBUSY` closes only that fast transport.
10. `FlipDrivenSoftware` gains a separate mode-period software protocol clock
    for idle `PresentNotifyMSC`; it cannot prove KMS completion or release a
    buffer.
11. The owner explicitly tracks `AtomicSnapshotId`; EventToken exhaustion asks
    for controlled restart rather than automatic incarnation replacement.
12. The owner/merged-primary PR may merge independently with C.0 capability
    false, preserving bisectability before final cursor/gamma conversion, but
    has its own RX 6800 XT/RTX 5060 Ti lifecycle hardware gate.
13. The qualified coordinate ioctl is measured with the other seat-active host
    calls, follows the selected global execution architecture, and cannot run
    during an exclusive validation lease. `CoordinateSubmitting` retains the
    sole device slot through typed return or actual helper reap, preventing an
    uncertain late ioctl from racing its atomic fallback.
14. N=2 counts each distinct workload-issued per-CRTC generation once; carried,
    duplicate, skipped, rejected, and superseded state cannot inflate it.

## Deliberate pushback

The proposed restoration of a production environment/config override was not
accepted. The current merged tree has no `YSERVER_HW_CURSOR_NVIDIA` runtime
lever, the prior maintainer decision rejects production rollout switches, and a
manual override cannot substitute for hardware evidence. The valid concern—one
GTX 1050 result cannot determine every NVIDIA generation—is handled by the
cohort-specific support policy instead.

The review's suggestion that one fast cursor-only atomic measurement could
delete the complete executor was also narrowed. Cursor-only timing cannot prove
that modeset, `TEST_ONLY`, gamma, unflip, GET_SEQUENCE or other seat-active host
calls never stall. The evidence gate measures every relevant class and requires
a spec rewrite after the result; it does not preserve both architectures into
implementation.

## Merge-required hardware

- AMD Navi 21/Radeon RX 6800 XT: full C.0-ready matrix must pass. The former
  Polaris/RX 580 is unavailable and its historical evidence cannot substitute
  for current C.0 validation.
- NVIDIA RTX 5060 Ti: executor evidence probe, policy arms and soak are
  mandatory. Atomic-HW cursor policy/performance failure leaves the Blackwell
  cohort software-cursor/C.0-incapable without blocking AMD-backed C.0 merge.
  Host-call, owner-completion, poison, watchdog, or off-fence failure in its
  mandatory software-cursor soak remains merge-blocking.
- Intel, Asahi and other AMD/NVIDIA cohorts are best-effort unless hardware is
  available. No unvalidated cohort is enabled from source audit alone.
