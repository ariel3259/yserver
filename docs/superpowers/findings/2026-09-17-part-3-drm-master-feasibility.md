# Part 3 — DRM master on tty2 is reachable, and the flip path works on this box

**Date:** 2026-09-17, from an active VT (tty2) with the desktop logged out.
**Spec:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` section 9.
**Status:** feasibility measurement only. **This is not part 3**, and nothing
here satisfies P3-1..P3-4: the probe drives dumb buffers through libdrm and
never touches the `ResourceService`, the ledger or a managed scanout buffer.
Part 3's tests do not exist yet; per section 9.3 the implementer writes them
and the user runs them.

## Why this was measured first

Section 9.1 says the flip-accepted path "has never run on hardware here".
Before spending a plan, a codex review and an implementation cycle on tests
that only an active VT can run, the question is whether this box can run them
at all: two GPUs, a proprietary NVIDIA driver, and master granted by logind
only to the seat's active session.

## The environment, as measured

| Fact | Value |
| --- | --- |
| Active session while the desktop was up | session 1 (wayland, tty1) — `SET_MASTER` from tty2 refused with **EACCES** on both cards |
| Active session after logging the desktop out | session 3 (tty, **VTNr 2**) — `SET_MASTER` **succeeds** on both cards |
| `card0` | amdgpu (integrated), every connector disconnected |
| `card1` | **nvidia** (RTX 5060 Ti), **HDMI-A-2 connected** |
| Vulkan's preferred device | the NVIDIA one, primary node **226:1 = `card1`** |

So the node `VK_EXT_physical_device_drm` reports for the device Vulkan selects
is the same card that drives the display. The multi-GPU hazard the fixture's
own comment describes (`backend.rs` ~6003: "blind `card0` is the integrated
one") does not bite here, because the fixture prefers the reported node.

## The probe

`tools/part3-flip-probe.c`, run as `./flip_probe /dev/dri/card1`:

```
master: acquired
atomic client cap: yes
connector 1216: 1920x1080 1920x1080@60
crtc: 205
dumb buffers + ADDFB2: ok (fb 1239, fb 1240)
modeset: ACCEPTED -- fb 1239 is on screen
P3-1: page flip ACCEPTED (fb 1240), waiting for the kernel event...
page-flip COMPLETION from the kernel: seq=0 at 340.596886
OUT_FENCE_PTR property: present
P3-4: atomic commit with OUT_FENCE_PTR -> 0 (ok), out_fence fd=-4294967292
page-flip COMPLETION from the kernel: seq=0 at 340.613749
probe: done
restore: drmModeSetCrtc -> 0 (ok)
restore: master dropped
```

Every ingredient part 3 needs works on this hardware: master, modeset, an
**accepted** page flip, a **real kernel completion event**, and an atomic
commit carrying `OUT_FENCE_PTR`. The console was restored and master dropped.

**One gotcha for whoever writes the tests.** `OUT_FENCE_PTR` writes an **s32**
into the caller's storage. The probe passed an `int64_t` pre-set to `-1`, so
the kernel filled only the low half and the value read back as
`-4294967292` = `0xFFFFFFFF00000004` — the real fd is `4`. The production code
is already correct (`let mut out_fence: i32 = -1;`, `scene.rs:7893` and
`:8011`); a test that widens it would read a bogus fd and could "prove" a
fence that is not there.

## The existing hardware tests, with master available

Section 9.2's last paragraph asks which existing hardware tests encode the
no-master outcome, measured rather than assumed.

`cargo test -p yserver --lib c0_2ci -- --ignored` with master available:
**18 passed, 0 failed** — identical to the run without it. The reason is not
that they are indifferent: the fixture opens the real primary node
**deliberately without master** (`backend.rs` ~6003: the three ioctls it uses
carry no `DRM_MASTER` requirement and "the fixture never commits"), so master
at the session level never reaches them.

The one that **encodes the no-master outcome** is
`c0_2ci_sink_gamma_gate_four_states_drm` (`resources/tests.rs` ~3306): in its
`Legacy` arm it asserts the gamma ioctl is reached **and fails**
(`expect_err("apply_gamma_to_live_output without master must fail")`). That
assertion holds only while the fixture holds no master. If part 3 makes the
fixture acquire master, this test must be revisited — it would then be
asserting the opposite of what the kernel does.

`arm_idle_vblanks_with_scanout_disallowed_clears_and_returns_zero`
(`backend.rs` ~43900) reads like a second one, but it is a deterministic unit
test driving `VtState::Suspended`; it encodes the policy, not a kernel
outcome, and real master does not touch it.

## What this changes for part 3's plan

- The tests can run on `card1`, and the plan's commands should select it by
  the node Vulkan reports rather than by index.
- The fixture will have to take master deliberately, which is the change that
  puts `c0_2ci_sink_gamma_gate_four_states_drm` in scope.
- P3-4's out-fence must be read into an `i32`.
- Still open, and not addressed here: whether a **PRIME-imported Vulkan
  image** — not a dumb buffer — can be flipped on this NVIDIA driver. That is
  P3-1's real content, and the probe does not answer it.
