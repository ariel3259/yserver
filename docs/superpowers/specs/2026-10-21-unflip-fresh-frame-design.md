# Fresh composed frame on direct-scanout unflip

Issue #281 · Design spec · 2026-10-21

Citation roots: `B` = `crates/yserver/src/kms/render/backend`; `P` = `crates/yserver/src/kms/render/platform`; `S` = `crates/yserver-render/src/kms/render/scene`; `D` = `crates/yserver/src/drm`.

## Problem and current behavior

When direct scanout ends, yserver can show an old composed frame for one refresh. The unflip path selects each output's retained `OnScreen` BO, which direct scanout left untouched; its grouped atomic commit has no render dependency. Only after that replacement retires does yserver invalidate scene damage and compose again. The first visible composed frame is therefore stale. (`B/kms/scanout.rs:1045–1083, 1202–1210`; `P/scanout_bos.rs:196–212`; grouped commit: `D/modeset.rs:1785–1860`.)

Unflip is requested by several scene and Present transitions; for example, an ineligible authoritative-root Present calls `request_direct_unflip`, which clears the direct hold (`B/kms/present.rs:825–833`; `B/kms/scanout.rs:318–328`). Before unflip, yserver materializes the active direct source into its pinned fallback target (`B/kms/scanout.rs:935–1042`). Direct output retirement deliberately keeps the prior composed BO reserved (`B/kms/scanout.rs:1241–1245`). The direct Present has already received its Flip completion with `emit_idle = false`, but its source and fallback pins stay held; `release_direct_frame` later queues its one Idle and releases both pins (`B/kms/scanout.rs:769–773, 1253–1287`).

## Design

Use one asynchronous prepare → readiness → grouped-submit flow for every unflip, including shared and copied outputs. Preparation renders the current scene into each output's free scanout BO while its old `OnScreen` BO remains reserved and visible. Readiness means that every output has a final KMS input dependency available; a fence may still be unsignalled. The core never blocks for rendering. One all-output atomic commit then replaces the primary planes.

The batch is a render handoff until KMS accepts the grouped commit. Render jobs and their readiness cookies are not page-flip obligations. On acceptance, the single P1 commit ID is installed in every output's scene acknowledgement and event association. Each output then retires through `SceneCompositor::handle_page_flip_complete` exactly once; direct ownership is released only when all those replacements have retired.

The production scene path already flushes preceding paint before `SceneCompositor::tick` (`B/portable/compose.rs:33–87`), builds and records the output scene (`S/tick.rs:699–710, 1249–1265`), and creates normal acknowledgements (`S/tick.rs:1334–1349`). It must be adapted to defer individual KMS submissions during an unflip batch. Do not create a test-only production route. Copied outputs keep the existing source-render readiness and source-fence import, but defer the sink's per-output KMS submit until the grouped commit; the sink-copy completion is that output's final fence (`P/scene_sink.rs:178–196, 689–709`; `P/scanout_bos.rs:56–66, 107–145`).

## Testable rules

**R1 — Fresh full frame.** Before submitting any replacement, materialize the current direct source into its captured fallback target, include all preceding paint, invalidate/mark the scene so every output gets a full repaint, and render the current scene into a `Free` BO for that output. Keep the old `OnScreen` BO reserved through that output's grouped replacement retirement; never render into it during preparation. Scene invalidation and dirty marking are available in `SceneCompositor` (`S/lifecycle.rs:220–235`; `S/damage.rs:53–79`); the platform acquires free scanout BOs (`P/scanout_bos.rs:34–54`).

**R2 — One async state for all routes.** Shared-only, copied-only, and mixed output batches use the same prepare/readiness/group-submit lifecycle. Keep output identity, BO identity, render-job identity, and topology generation with each prepared plane. Advance the state from normal backend/event-loop entries; the core must not wait synchronously for GPU or sink-copy completion.

**R3 — Dependency order.** Preserve this order: materialization and preceding paint → scene render → optional sink copy → grouped scanout. A copied output's sink copy must consume the source render dependency and be submitted with its completion fence exported before that destination can enter the grouped commit; KMS waits if the fence is still unsignalled.

**R4 — Real per-plane fences.** The grouped atomic request carries each output's final sync-file dependency as its primary-plane `IN_FENCE_FD`: scene-render completion for shared output, sink-copy completion for copied output. Preserve descriptor ownership through acceptance or rejection. `None` means the work is already signalled and must be encoded as the signalled/no-fence sentinel (`-1`), not treated as missing readiness (`D/page_flip.rs:220–254, 317–325`). KMS waits on these dependencies; CPU-observed GPU completion is not required before grouped submission.

**R5 — Readiness is not KMS acceptance.** A render-only batch, including a copied source-render completion, is never an accepted page-flip obligation and never receives a KMS commit ID. Do not put it in page-flip/event retirement accounting. Only successful grouped atomic submission assigns the P1 accepted commit ID to every output's pending scene acknowledgement and event association. Rejection publishes no accepted ID.

**R6 — One synchronized replacement.** Submit every output's freshly rendered primary FB in one all-outputs atomic commit; do not split a successful unflip into per-CRTC replacements. Keep the single-device and homogeneous-refresh eligibility restriction (`B/kms/scanout.rs:1045–1062, 1290–1311`); independent AMD replacement can fail with `ENOSPC`. The replacement is vblank-synchronized even if the preceding direct flip used async timing: use the non-async page-flip mode (`PAGE_FLIP_EVENT | NONBLOCK`, without `PAGE_FLIP_ASYNC`; `D/modeset.rs:1785–1860`).

**R7 — Normal scene retirement once.** Match each completion to the accepted P1 ID, then run that output through `SceneCompositor::handle_page_flip_complete` exactly once so its acknowledgement, damage snapshots, descriptor slot, cursor transition, and BO lifecycle retire normally (`S/flip.rs:84–240`). Direct-unflip accounting must not consume or duplicate the scene retirement. Do not perform a redundant post-retirement full invalidation on this successful fresh-frame route.

**R8 — Direct lifetime and Present events.** Keep the direct framebuffer, source pin, fallback-target pin, and release bookkeeping alive until every output's accepted replacement retires. Then emit exactly one Idle/release for the direct Present; do not send a second CompleteNotify (the direct Flip Complete already occurred). A wrong, old, duplicate, or partial event cannot release ownership. Existing release behavior is in `B/kms/scanout.rs:769–773, 1166–1219`.

**R9 — P2 cursor behavior.** Preserve P2 atomic cursor rules. At grouped submission, gather only the applied-binding position delta for visible, applied atomic cursor planes (`P/cursor.rs:521–548`); acknowledge it only after commit acceptance. Unflip carries no cursor framebuffer, hotspot, or geometry changes. Scene acknowledgement performs each cursor transition once; do not also run the old external cursor handoff for that output.

**R10 — Lifecycle changes.** During preparation, VT release, DPMS-off, and RANDR/topology changes cancel or quiesce prepared work safely, recheck VT/DPMS/topology immediately before grouped submission, and discard/rebuild an unsubmitted batch when the output layout changes. A cancellation cannot free a BO while GPU work may still write it; retain direct pins until KMS no longer references the direct framebuffer. After acceptance, the grouped commit and all its events remain tracked until retired or the complete old topology is disabled and its events drained before reset. Relevant lifecycle boundaries include `B/kms/session.rs:464–490, 1006–1029`, `B/kms/crtc_config.rs:34–90`, and `B/kms/randr.rs:1315–1345, 1810–1825`.

**R11 — Pacing and wakeups.** Pending unflip composition is visible scanout work for Present idle pacing, from preparation through accepted replacement retirement. Readiness waits use completion/event notifications and rechecks; they must not poll or arm busy wakes. Keep the compose-wanted and next-wakeup predicates aligned so pending work is neither stranded nor busy-spun (`B/portable/compose.rs:12–28`; `B/kms/present.rs:330–365`).

**R12 — Rejection policy.** A transient grouped rejection retains the direct ownership and prepared/retry state, publishes no accepted ID, and retries through the existing scheduling path. Do not change the persistent grouped-failure policy in this issue: the current error fallback to per-output composed flips remains as-is and is a known item outside this design (`B/kms/scanout.rs:2551–2587`).

## Non-goals

- Redesigning the persistent grouped-failure/error fallback policy.
- P3 cursor-frame handoff changes.
- Tearing or VRR behavior changes.
- New X11/Present protocol behavior.

## Test requirements

### A. End state

Every completed scenario drains through normal production retirement and ends with no pending unflip batch, render jobs, scene acknowledgements, event cookies, or direct pins; only `Free`/`OnScreen` BOs; no fd leaks; and exactly one direct Idle. Assert no second direct CompleteNotify.

### B. Backend-driven tests

Use `RecordingSceneFixture`, the recording atomic executor, and P1 `queue_commit_completion` to drive requests, readiness, grouped submission, and retirement through Backend entry points. Do not call internals to fake production paths. Inspect pixels in the BO named by the recorded primary `FB_ID`, including uncovered areas; never use root readback, which could hide the stale-buffer bug. Existing fixture support is in `tests/commit_identity/{recording_scene.rs,support.rs}`.

1. `issue281_fullscreen_exit_flips_fresh_scene`: compose scene A, direct-present distinct scene B, request unflip (including fullscreen exit), and prove the grouped primary FB contains the fresh full scene. Mutations: select retained A, omit materialization, or do only a partial repaint.
2. `issue281_group_waits_for_every_render_and_retires_once`: two outputs with distinct source crops; hold readiness, release one render and then the other through Backend. Assert no early or per-CRTC replacement, then exactly one grouped commit with correct FBs and fences. Inject wrong, old, duplicate, and partial completion cookies; none may release direct ownership early. Mutations: submit after one output is ready, swallow scene retirement, or release on the first CRTC.
3. `issue281_copied_unflip_fences_sink_copy`: drive copied readiness through Backend; prove the grouped plane names the fresh copied destination and carries the sink-copy completion fence. Mutations: flip before the copy, use only the source-render fence for unfinished sink work, or select the retained destination.
4. Extend `p2_direct_hook_treats_deferred_as_failure_and_unflips` and `p2_scene_direct_copied_and_unflip_commits_carry_only_the_applied_position` with fresh-pixel assertions. Mutations: use an old framebuffer, staged hotspot, cursor FB_ID in unflip, or double cursor handoff.
5. `issue281_pending_unflip_survives_rejection_and_lifecycle`: cover transient grouped rejection/retry and VT release, DPMS-off, and RANDR changes both during preparation and after acceptance. Assert no stale resubmission, phantom event wait, premature BO reuse, or lost direct ownership. Mutations: retain an obsolete job, treat preparation as acceptance, or reset a writing/scanned BO.

### C. Hardware validation

- NVIDIA, single output, Cinnamon-like session: fullscreen `mpv` enters direct scanout, then leave fullscreen. A debug log must show `submitted atomic composed unflip`; user observes no flash.
- AMD grouped-output setup: validate the fresh all-outputs replacement and completion behavior on the hardware affected by the per-CRTC `ENOSPC` constraint.
- Compare master `8e71c788` with the fix. If `mpv` cannot reach direct scanout on the AMD setup, record AMD direct-scanout coverage as uncovered rather than claiming validation.

### F. Repetition and skips

Run deterministic backend cases 5× under 16 busy-loop workers; run GPU-waiting cases 20×. Any unavailable renderer/ICD prerequisite produces a single-line skip naming that prerequisite.

## Implementation boundaries

Reuse production scene composition, render completion registration, scene acknowledgements, and normal page-flip retirement. Keep DRM/GBM/KMS types out of `yserver-render`; do not add `cfg(test)` branches to production bodies or `YSERVER_*` behavior switches. Stay within `tools/check-layout.sh` ceilings. Update `docs/status.md` when implementation lands.

## Provenance

Read the read-only investigation note at `/home/ariel_santangelo/Projects/claude-scratch/unflip-281/design-note.md`, then checked its production claims against the files and line references cited above. Sources are `B/kms/{scanout,present,session,crtc_config,randr}.rs`, `B/portable/compose.rs`, `P/{scene_sink,scanout_bos,cursor}.rs`, `D/{modeset,page_flip}.rs`, and `S/{tick,flip,lifecycle,damage}.rs`; test fixtures are under `crates/yserver/src/kms/render/backend/tests/{commit_identity,cursor_sprites}/`. No copyleft source excerpts were used.

## Open questions

None. User decisions in the issue request are binding for implementation.
