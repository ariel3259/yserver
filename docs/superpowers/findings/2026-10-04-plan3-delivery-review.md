## Verdict

0 blocking, 0 major, 0 minor

Coverage: INCOMPLETE

No defect meeting the restricted reporting rules was demonstrated. This is not a clean review: the excerpt limit left Task 4’s pre-yield completion handoff unresolved.

## Incorporation audit

| Prior findings | Disposition |
|---|---|
| None; first review | Check 1 skipped as instructed. |

## Findings

### Blocking

None demonstrated.

### Major

None demonstrated.

### Minor

None demonstrated.

## Coverage and implementation checks

**1. Incorporation audit:** Skipped because there is no prior review.

**2. Architecture and cross-task contracts:** Checked the specified delivery owners and relevant baseline paths.

- Task 1’s per-window hierarchy/presence rule agrees with addendum E1 ([lines 162–170](docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md#L162)). Tagged Xorg implements root and descendant traversal in `Xi/exevents.c:3279–3312`. This checkout currently collects presence and hierarchy targets per client, while `DeviceChanged` already collects selected windows ([hotplug.rs:33](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/xinput/hotplug.rs:33), [hotplug.rs:210](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/xinput/hotplug.rs:210), [hotplug.rs:370](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/xinput/hotplug.rs:370)).
- Task 2 agrees with F1 ([lines 181–186](docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md#L181)) and tagged Xorg’s natural-delivery attempt followed by fallback in `dix/events.c:4431–4462`. The described scroll coupling exists: Motion fallback depends on `xi2_grab_window_target` computed for the paired event ([pointer_fanout.rs:2064](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/core_loop/pointer_fanout.rs:2064)).
- Task 4 states G’s required ordering correctly. Its integration with the synchronous release path remains unresolved below.

**3. Safety, ownership and failure semantics:** Checked Xorg’s saved lock-action semantics, configuration-result identity matching and the VT release boundary.

Task 3 agrees with F2 ([lines 188–191](docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md#L188)). Tagged Xorg saves the press action and suppresses clearing saved locked bits when `LockNoUnlock` applies (`xkb/xkbActions.c:373–384`). The checkout’s detach path transfers saved modifier bits ([backend.rs:12528](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver/src/kms/render/backend.rs:12528)); the subsequent floating-release path was not inspected within the budget.

The configuration completion consumer checks both token and source and retains generation/client filtering ([run.rs:1324](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/core_loop/run.rs:1324)). No contradiction with the main design’s process-lifetime completion ownership was demonstrated.

The unresolved VT sequence is concrete: an applied write produces a completion awaiting core consumption; the core enters VT release; the backend hook performs pause/suspend and `VT_RELDISP` before returning ([run.rs:1244](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/core_loop/run.rs:1244), [backend.rs:23104](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver/src/kms/render/backend.rs:23104)). Task 4 requires consumption before that yield, matching G ([lines 197–206](docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md#L197)). The implementation of `pause_input_thread` and the surrounding runner handoff were not read, so this was not classified as a demonstrated plan defect.

**4. Spec compliance and verification strategy:** Compared Tasks 1–5 with E1, F1, F2, G, H and relevant main-design ownership, lifecycle and documentation requirements. No textual contradiction was found. The addendum explicitly withdraws the main design’s resolve-after-rebind clause, so Task 4’s removal of that behavior is consistent.

The inherited rules assign real request/input paths, post-scenario state checks, mutation checks, nightly formatting, tests and `cargo clippy --all-targets -- -D warnings` to implementation. Task 6 assigns the requested hardware pass. No builds, tests, mutation experiments or portability checks were run; their results remain unestablished.

**Reading budget:** 24/24 bounded excerpts beyond the target plan, counting disjoint ranges separately. Investigation stopped at the cap.

**Follow-ups (not defects):**

- Resolve only Task 4’s handoff: inspect `pause_input_thread` and the runner’s VT-release dispatch to determine how the core can consume configuration results before final session yield. This distinguishes an executable existing handoff from a necessary release-phase change; it does not justify another full review.