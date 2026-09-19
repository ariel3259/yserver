# Stage 2c-iii, plan Ci (owner entry, composed producer, damage) — accepted

**Plan:** `docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md`
revision 3, after two codex review rounds (1B 4M, then 1B 1M 1m).
**Spec:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`
revision 4 plus §4.6, after three codex review rounds.
**Implemented by:** codex, model `gpt-5.6-luna`, reasoning effort `xhigh`, one task
per run. Tasks 1–4 and the first Task 5 rounds ran in `--sandbox workspace-write`;
from the last Task 5 round on, codex ran with `--sandbox danger-full-access`
(authorized by the user), so it could run the `_vulkan` tests on the real GPU.
Its prompts forbade git writes and any hardware test outside the plan's own filter.
The coordinator verified every task outside codex and committed it.

| Task | Commit | Sent back |
| --- | --- | --- |
| 1 — fallible, CommitId-aware owner entry | `914f2818` | no |
| 2 — conductor registers old-state dependencies, per member | `b4b8667a` | no |
| 3 — production composed description builder | `3812e0cd` | no |
| 4 — prepare/submit fork, `Owner` eligibility | `ab2a348f` | twice: F-T4-1..5 (the owner route skipped `add_damage`; four tests that could not fail), F-T4-6..8 (measured on the GPU: a fixture without a real DRM node, the injected source dispatching, a queue-length assertion) |
| 5 — real composed source | `292c04a8` | three times: F-T5-1..3 (the acquisition lease blocked the compose's own write; a let-chain dropped submitted generations; injected composed answers were the default), F-T5-4 (framebuffer read from `bo.fb_handle` only), F-T5-5 (readiness ignored the compose batch's live write lease) |
| 6 — damage transaction | `338728a8` | no |
| 7 — owner buffer lifecycle after dispatch | `4b00b269` | once: F-T7-1 (the gate test asserted withheld gates without running a tick) |
| 8 — bundles, composited Presents, scene contracts | `f153c3a5` | once: F-T8-1 (a dormancy change applied to Legacy too) |

Upstream `joske/master` `00211310` (MIT-SHM PutImage GC clip, #162) was merged as
`c2ff6965` before the hardware gate; a clean merge.

## What Ci establishes, at fixture level

- `register_kms` has its first non-test caller: every conductor dispatch registers
  the commit's old-state dependencies inside the ledger closure, per member, and
  the 2c-ii conductor's whole-device `take_current` defect is fixed (spec §4.6).
- The shared managed composed route forks on the device's transport: `Legacy` is
  unchanged; `Owner` composes, waits for the render completion (no producer fence
  crosses the ioctl), is offered and admitted through the conductor with a real
  `CommitDescription`, and drives damage and buffer reuse from owner milestones.
- The owner buffer lifecycle of plan decision 10 is implemented as `BoPhase`
  states; the damage transaction of spec §4.2 is keyed by `CommitId` and by
  `OutputKey`/generation per member.
- Tier-5 bundles are one transaction; composited Presents keep their GPU-batch
  authority.

Production is unchanged (C0-R8): no conductor is installed there.

## Mutations

R1–R34 of the plan, plus those added by the send-backs. Applied by the
coordinator by line and run on the GPU unless marked otherwise.

| Mutation | Result |
| --- | --- |
| R1, R2, R4, R5, R6, R7, R9, R10, R12, R14, R15, R16, R17, R21, R24, R27, R28, R29, R30, R31 (quarantine, mechanism failure, topology), R33 | caught |
| F-T4-1 (skip `add_damage`), never free a displaced buffer, F-T5-2..5, no quarantine on unknown, both dormancy route mutations | caught |
| R22, R23, R25, R26 | caught — applied and reverted by codex on the GPU and reported; not re-applied by the coordinator |
| R3 (drop the Present refusal of the fallible entry) | **equivalent**: the completion-context validation downstream refuses the same descriptions |
| R19 (free from any phase in the release pass) | **equivalent**: `BoState::transition_to_free_after_owner_release` refuses anything but `OwnerReleasing` |
| R32 (skip the member identity check at `HardwareComplete`) | **survives, defence in depth**: no path reaches a live transaction whose member is no longer submitted — topology invalidates first (caught) and a pre-IPC refusal closes it |
| R8 (copied route allowed into `Owner`) | covered by the pure validator only: no fixture builds a copied output (F8) |
| R11, R13, R18, R20, R34, F-T5-1 | not applied mechanically (structural); R20's guard is proven at the resource service by `c0_2ci_kms_release_does_not_complete_gpu_work` and `c0_2ci_commit_kms_obligations_block_release_gate` |

## Gate

Software (coordinator): fmt; clippy default, `tcp-transport`, `xdmcp`;
`cargo check --workspace` for gnu, musl, FreeBSD; `c0_conv_ci_` five times debug
and once release; `c0_adm` 129/0; `c0_2ci` 180/0/21; full `--lib` 1921/0/121;
`cargo test --workspace` with no failures after the #162 merge.

`c0_conv_ci_ -- --include-ignored` on the GPU: **38/38** in debug and release.

**Hardware gate (2026-09-19, user on tty, GPU free, after the #162 merge):**
`render_acceptance -- --ignored` **164/164** (163 before, plus #162's test);
`c0_2ci -- --ignored --test-threads=1` **21/21**; the library's other ignored
tests (`--ignored --skip c0_2ci --test-threads=1`) **100/100** (74 before, plus
Ci's 26) — **285/285**.

## F8 stops, recorded and not invented

1. **Legacy dormancy bug (pending the user's decision).** In Legacy, a `NoBO` or
   `NoPool` skip after the walk still counts as walked: a visible drawable is
   marked `HiddenDamage` and its repaint suppressed though no frame was
   presented. Upstream logic; `c0_conv_ci_legacy_dormancy_no_bo_no_pool_f8`
   (ignored) characterises it. Ci applies the correct rule to Owner only.
2. **Restore row (spec §4.2):** no `TerminalState` is a post-`Accepted` failure
   with the prior state proven current.
3. **Device loss** has platform/renderer signals but none reaches an owner
   damage transaction.
4. **Copied route:** no fixture builds one; R8 is validator-only. The copied route
   is Ciii's.

## Carried forward

- **Cii:** the direct dispatch fills an empty new-resource member set from the
  current state's members (Task 2); the real direct producer must supply it.
- **Ci-refactor (user decision, before Cii):** (b) one per-buffer state machine
  as a type in place of the four owner queues and the owner `BoPhase` variants;
  (d) one dispatch error path and one owner begin entry. Behaviour-preserving;
  accepted when every suite above and every caught mutation are unchanged.
- **Cii and Ciii constraint (user):** new owner-route code lives in its own
  functions/modules behind one fork point per producer, never as branches inside
  legacy functions.
- **Debt left for the end of stage 4:** the injection into `scene.rs` /
  `tick_one_output`, the legacy primary branches (migration scaffolding), and test
  hooks in production types (`test_skip_render_completion_drain`,
  `DeviceCommitOwner::complete_for_tests`).
- Pre-existing, outside Ci: live-scene tests built on
  `for_tests_with_vk_live_scene()` that `eprintln` and return on a fixture error
  pass vacuously, in the hardware gate too.

## What this round taught

- **A sandboxed implementer writing GPU tests works blind.** Tasks 4 and 5 took
  five rounds between them, each diagnosed by the coordinator instrumenting the
  real GPU. Unsandboxed, Tasks 6 and 8 needed no diagnostic round and Task 5's
  last finding closed in one.
- **The coordinator's mutations still found tests that could not fail** after
  codex had GPU access (F-T7-1), and a silent Legacy change (F-T8-1). Reviewing
  the diff for changes outside the owner route, and re-running the mutations,
  are not optional.
- A plan step that needs a later task's code (Task 4's displacement test needing
  Task 6's ack path) is a plan error the implementer cannot resolve; it has to be
  moved, not worked around.
