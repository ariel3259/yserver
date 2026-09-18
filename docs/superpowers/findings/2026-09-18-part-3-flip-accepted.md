# Part 3 — the flip-accepted path on real kernel evidence: P3-1 and P3-4 proven

**Date:** 2026-09-18, from tty2 as the seat's **active** session, Hyprland logged out.
**Spec:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md` section 9.
**Plan:** `docs/superpowers/plans/2026-09-17-phase-c0-stage-2c-i-debt-part-3.md`, revision 4.
**Tree:** `451e2ae6`. Implemented by codex (`gpt-5.6-luna`, `xhigh`); every task reviewed and committed by the coordinator.

## The boundary, first

**Part 3 is not the bounded delivery check of the C.0 design's section 16.3.
It satisfies none of that check's requirements and is never reported as doing
so** (spec 9.4). It is an early, partial falsification of the stage 2c-i ledger
on real kernel evidence, and it proves two of section 9.2's four invariants.

## Result

| Invariant | Test | Result |
| --- | --- | --- |
| **P3-1** — a managed scanout buffer, flipped through the real path, is accepted by the kernel and becomes the CRTC's current buffer | `c0_2ci_managed_scanout_flip_accepted_drm` | **proven** |
| **P3-2** — `KmsRelease` discharged by the real completion, not before | — | **F8 stop, not proven** (spec 9.5) |
| **P3-3** — a retained buffer registers no obligation | — | **F8 stop, not proven** (spec 9.5) |
| **P3-4** — the flip's out-fence resolves through its canonical status query | `c0_2ci_managed_scanout_out_fence_resolves_drm` | **proven** |
| 9.2's last paragraph — which existing test encodes the no-master outcome, **run with master** | `c0_2ci_sink_gamma_gate_four_states_master_drm` | **answered by execution** |

### P3-1

The fixture performed production's initial modeset (first bo with a
framebuffer, `commit_modeset`, `mark_on_screen_after_modeset`), then a free bo
was converted to managed through `register_managed_scanout_bo`, the scene
rendered into it through the real submission path, and
`submit_flip_with_fences` returned `Ok`. The test waited, under a 5 s deadline,
for **that** bo's page-flip completion, and only then read the CRTC back from
the device: it scans out the managed framebuffer, and not the one it scanned
out before the tick. The GPU retirement batch for the frame then retired.

This also answers the question the plan expected to be its likeliest F8: **the
NVIDIA 615.71.09 open driver does flip a PRIME-imported, Vulkan-rendered
framebuffer**, not only a dumb buffer.

### P3-4

After the accepted flip and its matching completion, the bo — now `OnScreen` —
still owned the out-fence in `release_fence_fd`. Borrowed without being taken,
it reached `FenceStatus::Success` through `sync_file::query_status` within the
deadline, and the bo still owned the same descriptor afterwards. No timeout, no
fence `Error(_)`.

### The gamma audit

Run with the live fixture's **master-holding** fd, the existing gamma test's
own body — the same four gate states — gave:

- Quiescing, Owner, Closed: refused by the transport gate before the ioctl, as
  without master;
- **Legacy: `Ok`** — the gamma write succeeded.

So `c0_2ci_sink_gamma_gate_four_states_drm` does encode the no-master outcome:
its Legacy arm's `expect_err("... without master must fail")` holds only
without master, and its doc comment's stated reason is the true one. The
existing test is unchanged; if a future fixture gives its fd master, that arm
must be revisited. The master-held run snapshotted and restored the CRTC's
gamma, since the live fixture's CRTC snapshot does not cover gamma.

## Not proven here, and why

**P3-2 and P3-3** — spec 9.5. No production path registers a `KmsRelease`
obligation in stage 2c-i: `register_kms` is called only from
`register_commit_dependencies`, which is called only from tests, and the
discharge runs on the owner's events. The flip proven above registers a GPU
retirement batch and no `KmsRelease`. Their first real evidence belongs to the
owner commit path in stages 3/4, whose delivery check should carry them.

**R12 for the gamma master test.** The no-master check (the test must *fail*,
not pass, without master) was run for P3-1 and P3-4 and both failed with the
fixture's message before touching the CRTC. It was **not** run for the gamma
master test: it creates a Vulkan device, and on 2026-09-17 the GPU was in use
for VR — a similar run broke a WiVRn stream. That test enters through the same
`for_tests_with_live_kms` constructor, whose master acquisition is the
function that produced the verified failure; it is recorded as unverified by
execution rather than claimed.

## How it was run

Task 5's script was not written before the user had the VT ready, so the
coordinator ran the command it specifies, after checking what it would check:

- `loginctl show-seat seat0 -p ActiveSession` named this shell's session (3,
  VT 2); `nvidia-smi` showed the GPU idle (0 %, 10 MiB, no compute clients); no
  Hyprland, WiVRn or Steam process; `SET_MASTER` on `card1` succeeded and was
  dropped at once;
- `cargo test -p yserver --lib part3_tests -- --ignored --nocapture --test-threads=1`.

```
test kms::render::part3_tests::c0_2ci_managed_scanout_flip_accepted_drm ... live-KMS fixture: card=/dev/dri/card1 connector=HDMI-2 mode=1920x1080 (1920x1080@60)
ok
test kms::render::part3_tests::c0_2ci_managed_scanout_out_fence_resolves_drm ... live-KMS fixture: card=/dev/dri/card1 connector=HDMI-2 mode=1920x1080 (1920x1080@60)
ok
test kms::render::part3_tests::c0_2ci_sink_gamma_gate_four_states_master_drm ... live-KMS fixture: card=/dev/dri/card1 connector=HDMI-2 mode=1920x1080 (1920x1080@60)
master-held gamma Legacy outcome: Ok
ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 1840 filtered out; finished in 1.01s
```

No restoration failure was reported, and afterwards the GPU was back to idle.
