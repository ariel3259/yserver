# Stage 2c-iii, plan Cp — the copied scanout route: accepted

**Tip:** `cb49026e`. Plan revision 3 (`ee55ff4a` and later), spec revision 5.
Eight tasks, `de920baf` through `cb49026e`, plus two production fixes the
hardware run forced out of hiding.

**Plan Cp is accepted.** With it, stage 2c-iii's writer coverage is complete:
composed (Ci), direct (Cii, Cfb), unflip (Ciii), device-qualified identity
(Ciii-identity) and now the copied route. A device with a copied-route output
can enter `Owner`.

## What the route does

The copy became the producer's last stage. Renderer A writes the source on one
device, the sink copies into the destination on the KMS device, and only when
the resource service has retired the destination's write obligation does a
`ComposedOffer` reach the conductor. The copy's `sync_file` wakes the core
loop and authorizes nothing. No owner commit on this route carries an input
fence and no host→helper request carries a descriptor.

Stage A was converted too, which was not in the plan's first revision: the
copied compose arm passed no `ResourceService` at all, so the source was
unowned by the service for the whole of A's write. Plan review round 2 found
it and it amended the spec to revision 5.

## Evidence

| Task | Commit | Mutations caught | Recorded as unprovable |
| --- | --- | --- | --- |
| 1 eligibility + the first copied fixture | `de920baf` | Q1, Q2 | — |
| 2 prepared copy, stage A, the wake | `faf48532` | Q10–Q12, Q14–Q16, Q20, Q21, Q39, Q40 | Q13 type-enforced |
| 3 promotion consumes the receipt | `d3073ce6` | Q5, Q7, Q8, Q9 | **Q6 F8** |
| 4 failure after submission, both cancellations | `0b059cfc` | Q17–Q19, Q23–Q25, Q36–Q38 | — |
| 5 the offer and the commit | `926e11f8` | Q26, Q35 | — |
| 6 retirement and release | `0cc9a217` | Q28–Q31 | **Q32 F8** |
| 7 exclusivity at the copied site | `5a998a90` | Q27 | — |
| 8 the cross-device hardware run | `964e11c1`, `cb49026e` | Q33, Q34 documented for the run | — |

Q22 (`ReadObligation` has a production caller) was re-verified by the
coordinator with an independent mutation: registering the source obligation
under the destination key fails the named test. This route is that machinery's
first production caller.

**Two criteria are recorded as unprovable rather than claimed.** Q6: the state
that would discriminate the `is_frozen` half of CP-2a — this generation's
destination obligation discharged while its key is frozen — is unreachable,
because `validate_gpu_batch` returns `Err(Frozen)` before removing any
obligation and quarantine keeps obligations pending. Q32: two live copied
generations cannot share one destination allocation, because selection takes
the destination through `Recording` to `Owner` and a later selection accepts
only `Free`. Both are written into the plan with their reasoning so nobody
spends a session trying to prove them.

## The hardware run

`c0_hw_cp_copied_route_cross_device_drm`, from tty2 with the user's approval,
renderer forced to the amdgpu iGPU's render node (`226:0`, `renderD128`, no
connector needed) and KMS on `226:1` / `renderD129`, `HDMI-2` at 1920x1080@60.
`relationship=Different`, both identities recorded. **Passes.**

Four Legacy copied frames and four Owner copied frames, each Owner frame
running the full chain in order: pending obligation observed → generation
waiting → promotion gate passed → offer enqueued → helper `Accepted` →
`HardwareComplete` → `CompletionRetired` → damage applied. The destination
obligation is registered under the destination's own key and retired before the
offer, which is what proves the ordering; fence order does not.

It took nine runs. Six of them failed, and every failure was a real defect:
the connector name (yserver names connectors like Xorg — `HDMI-2`, not sysfs's
`HDMI-A-2`), a sibling fixture repeating task 1's context-swap crash, a harness
that compared ring length on a full ring, a wait loop that called the
compositor once per frame, and then the two production defects below.

## Two production defects, both ours, both in accepted work

**The descriptor-pool slot was lost at admission** (`814ced98`).
`OwnerBuffer::into_submitted` swallowed `Desired`'s `descriptor_slot` through a
`..` pattern and built a `Submitted` variant without that field, so the
existing release site took `None`. Every composed Owner frame leaked one slot;
with a three-slot ring the fourth frame deterministically could not acquire
one. The code is Ci-refactor's (`39a68bac`, accepted 2026-09-19) and the defect
is **not specific to this route** — it affects every composed Owner frame.
Three accepted plans missed it because no fixture anywhere ran four consecutive
composed Owner frames, which is the test that now exists.

**The managed acquisition was not rolled back on a pre-submit skip**
(`73cd20ae`). `acquire_managed_scanout_bo` transitions the destination
`Free → Recording` (B-13, ours, from 2c-i) and four reachable fallible exits
returned without undoing it. A bo left in `Recording` is never selected again.
On hardware one `NoPool` skip stranded a destination and the following **1441**
ticks all skipped with `NoBO`. Of the twelve exits enumerated between
acquisition and render submission, four were reachable and got a test and a
mutation each; eight were shown unreachable with a reason rather than given a
rollback that could never run.

This one was **first recorded as upstream's and corrected**: the compose tick
is shared, but Legacy's `acquire_scanout_bo` only selects a `Free` bo and never
transitions it, so Legacy strands nothing. The implementer refused to code
against that claim and checked it. Record and rule:
`2026-09-22-tick-strands-bo-on-post-acquisition-skip.md`.

Both defects would have surfaced at activation in stage 3 or 4, as a desktop
that freezes after a few frames with no indication of where to look.

## The submission-latency measurement

Asked for by the user when the design chose to wait for the copy fence in
userspace rather than hand it to the kernel as `IN_FENCE_FD`. Criterion fixed
**before** the numbers were seen: defensible if the submission delay is well
under a vblank period and the missed-vblank fraction is indistinguishable;
otherwise a measured limitation and an evidence-backed requirement for C.1.

**Submission delay, Owner: 368, 197, 171, 376 µs** — about 0.28 ms, under two
percent of a 16.67 ms vblank period at 60 Hz. The criterion's first half is met
with room to spare.

**Missed-vblank fraction, Owner: not measurable on this route**, reported as
`NA` with `vblank_comparable=0/4`. A composed Owner commit has no `Presented`
event and therefore no commit-correlated MSC; the clock that exists is marked
`correlated_to_commit=false`. This is a property of composed commits, not a gap
in the instrument.

**Legacy's submission delays are negative** (−4, −897, −915, −867 µs) and
correctly so: Legacy hands the fence to the kernel and queues the flip before it
signals. The two transports' submission delays are therefore **not comparable as
one figure**, and any future report must say so.

**The first numbers were an artifact and nearly became a finding.** Owner read
10312, 10229, 10250 and 10163 µs — four values within 0.15 ms of the wait
loop's own 10 ms poll quantum. Had that constancy gone unnoticed, we would have
recorded a 61%-of-a-frame penalty and built a C.1 requirement on a measurement
of our own harness. The number fell thirty-fold once the fence instant came
from the drain that observes it.

**What C.1 should be asked for**, narrowed by the evidence: not lower latency —
0.28 ms justifies nothing — but a **commit-correlated completion sample for
composed commits**, without which the vblank question cannot be answered on
this route at all. C.0 §14 already projects producer-fence transfer for C.1 but
does not say the copied route is among its consumers.

## Gate at the acceptance tip (`cb49026e`)

Software: `cargo +nightly fmt`; clippy `--all-targets -- -D warnings` in
default, `tcp-transport` and `xdmcp`; debug and release builds; `c0_conv_cp_`
**34/34** with `--include-ignored` in debug and release; `c0_conv_` 174;
`c0_adm` 129; `c0_2ci` 180/0/21; `--lib` **1960/0/222**.

Hardware: `c0_hw_cp_copied_route_cross_device_drm` passes, both latency
summaries `status=complete`. Coredumps: none after the run that produced them
during task 1's investigation.

Still owed before acceptance of the stage as a whole: `cargo check --workspace`
for Linux musl and FreeBSD at the final tip.

## Carried

- The managed-storage access debt (Cfb §2.5, site `backend.rs:18716`), still
  out of scope by the user's decision and still owed before activation.
- The Ci F8 stops: the Legacy dormancy bug (upstream's, meets none of the
  ownership tests), the missing restore `TerminalState`, device loss without an
  owner signal.
- Cfb's F31 per-site coverage; the T24 and T27 coverage gaps.
- `docs/phase-c0-upstream-fixes-revalidation.md` after stage 4.
- Q33 and Q34 are documented with exact line anchors for the hardware run but
  were not applied: the run that would carry them is the final-tip rerun.
