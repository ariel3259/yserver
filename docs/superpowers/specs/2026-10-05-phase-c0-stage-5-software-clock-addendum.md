# C.0 stage 5 revision: Owner software clock, then Legacy removal

**Status:** decision record, 2026-10-05 (user decision). The stage 5 design
still has to be written and reviewed before implementation.
**Amends:** [the C.0 design](2026-08-26-phase-c0-atomic-kms-migration-design.md)
and [the stage 3 umbrella](2026-09-22-phase-c0-stage-3-lifecycle-design.md) §6.
Where they disagree, this record wins.

## Decision

Stage 5 is split in two parts:

- **5a: Owner software clock.** Devices without DRM vblank stop falling back
  to `Legacy` and run `Owner` with a software clock. Devices with DRM vblank
  keep the hardware clock (`KernelSequence`, GET/QUEUE_SEQUENCE).
- **5b: Legacy removal.** Once 5a works on the no-vblank cohort, the `Legacy`
  route is deleted. That covers its transport state, legacy writers, legacy
  cursor/gamma paths and mixed Owner/Legacy servers.

This supersedes the previous stage 5 plan ("activation by capability, `Legacy`
is the permanent fallback", C.0 §18 and the umbrella §6 rev 6). It also lifts
the C.0 scope limit that created no software clock (C.0 design :1458, :3240,
:3415). That limit was a scope boundary, not a ban; it explicitly anticipated
"a future software clock".

## Why

- **Two KMS routes cost maintenance forever.** Every display feature is
  written and tested twice, and the no-vblank cohort runs the route that is
  exercised least.
- **The no-vblank cohort is large.** NVIDIA branches 580/595 (Maxwell, Pascal,
  Volta) cannot enable DRM vblank at all, and branch 610+ needs
  `nvidia-drm vblank=1`, which is off by default.
- **There is precedent.** Xorg's Present already has a software clock
  (`present/present_fake.c`: MSC = UST / `fake_interval`, woken by
  `TimerSet`). Wayland compositors anchor their presentation clock to
  page-flip events and predict between them.

## What the driver gives without vblank (evidence)

- Page-flip completion events still arrive. Without `drm_vblank_init()`,
  `drm_crtc_send_vblank_event()` sends sequence 0 and a `ktime_get()` stamp
  taken when the event is sent, not the real flip time
  ([NVIDIA forum: vblank disabled breaks page flip timestamps/sequences](https://forums.developer.nvidia.com/t/nvidia-drm-why-is-vblank-support-disabled-by-default-it-breaks-drm-page-flip-timestamps-sequences/377853)).
- Compositors that trusted those stamps failed. Weston asserted on repeated
  or backwards timestamps; KWin reports "Pageflip timed out"
  ([Arch forum](https://bbs.archlinux.org/viewtopic.php?id=300341),
  [NVIDIA forum, flip event timeout](https://forums.developer.nvidia.com/t/bug-570-124-04-freeze-on-monitor-wakeup-flip-event-timeout/325659)).

## Requirements for 5a (to be specified in the stage 5 design)

1. **Anchor** the clock to each Owner commit's flip-complete event.
2. **Snap** the anchor to the mode's refresh-period grid, to remove the
   `ktime_get()` send-time jitter.
3. **Stay monotonic.** MSC and UST never go backwards; a repeated or
   backwards stamp is rejected or clamped.
4. **Extrapolate while idle**, as `present_fake.c` does: MSC keeps advancing
   from the last anchor by the mode period, so Present
   `target_msc`/`WaitMSC` and GLX `OML_sync_control` progress without flips.
   This is unlike Legacy's flip-counting `software_msc`.
5. **Survive missing events.** A flip whose event does not arrive within a
   bound continues on the extrapolated clock instead of stalling.
6. **Keep one clock epoch per CRTC.** It must stay consistent across DPMS,
   mode changes, hotplug and VT, re-anchoring on the next real flip.
7. **Select by capability, not driver name.** Use the hardware clock exactly
   when GET_SEQUENCE works (the existing `KernelSequence` qualification);
   otherwise use the software clock. Log which one each CRTC uses.

## Validation targets

- This machine with `nvidia-drm vblank=0`. The parameter is read-only at
  runtime, so it needs a module reload or reboot, with the user present.
- An NVIDIA branch-580 GPU (Maxwell/Pascal/Volta), for example the
  maintainer's GTX 1050 Ti, as a validation target.

## Process

Stage 5 follows the usual C.0 flow: a stage 5 design for 5a and 5b, codex
review, split plans, then implementation. 5b starts only after 5a is accepted
on the no-vblank cohort.
