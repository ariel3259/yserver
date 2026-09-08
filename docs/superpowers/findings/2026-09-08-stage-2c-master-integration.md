# Stage 2c comparison with current upstream master

Date: 2026-09-08. Local source/baseline review, not an adversarial spec/plan
review. The frozen external review instrument was not run and no instrument
provenance or adversarial verdict is claimed.

## Compared inputs and merge

- Feature tip: `523a96c71d4d32290f6102087bb50eba6a42a2a8`.
- Upstream master: `f6c799672274a479a39da36587c134f73b94867c`.
- Common base: `02bafec3`.
- Six upstream commits: `8d448583` damage follow-ups; `9ec6d035`, `a1e33aa8`,
  `735c7c6f` recipes; `b45b7e06` Vulkan selection; `f6c79967` capture tools.

The merge has one textual conflict: appended sections of `docs/status.md`.
Both sections were retained, along with the local stage-2c status entry.
`kms/render/backend.rs` is the only overlapping code file and auto-merged;
the new owner code and upstream scene/store changes touch different files.
That is not proof of semantic compatibility; fresh validation is logged under
`/tmp/yserver-master-integration-20260908/` and summarized in `docs/status.md`.

## Required design changes

| Area | Source evidence | Consequence for 2c |
| --- | --- | --- |
| Repaint owed without new paint | `scanout_damage.rs::owes_repaint`, backend compose predicate and scene `walk_needed` | 2c-iii invalidation must retain repaint demand; quarantine must block dispatch without spinning on that demand. |
| Visibility dormancy | `store.rs::DormantReason`, `damage`, `reconcile_offscreen_no_draw` | 2c-i allocation extraction preserves logical state; 2c-iii retains distinct paint/structural re-arm rules. |
| Per-output work detection | `scene.rs::pending_presentation_for_output`, `dormancy_inputs`, `last_pieces` | 2c-ii readiness and 2c-iii wake integration cannot replace these with a global pending-damage boolean. Skipped outputs contribute retained visibility. |
| Snapshot attribution | `scene.rs::emit_node`, `ContentDamage`, `handle_page_flip_complete` | Rework the 2c-iii ack contract around exact submitted output snapshots. Hidden/OtherOutput/OffOutput non-empty snapshots are not carried or acknowledged there. |
| Scene identity | `scene_diff.rs` exact place rectangles, rank-change comparisons and presence signatures | Preserve shape/restack fixes and wake on actual storage/redirect changes when introducing allocation generations. |
| Device context | `vk/device.rs::validate_render_device_inventory`, `select_physical_device_candidate` | Retained allocations use their actual Vulkan context; a DRM render-node key alone no longer uniquely identifies that context. |
| Capture harness | `tools/vng-shot.sh`, `ppm-regions.py`, `qemu-monitor.py` | Available for future visual regression checks; guest captures do not replace physical C.0 evidence. |

The three blocks and the Accepted / HardwareComplete / Presented split remain
valid. Neither the seven-tier policy nor the decision to avoid new Present
protocol credits needs rewriting. The main design's damage section and 2c-i's
adapter constraints have been updated with the above requirements.

## Validation cases retained for implementation

Reuse upstream regressions for owed repaint, HiddenDamage re-arming,
skipped-output dormancy, OffOutput snapshots, exact shapes and pivot restacks.
Add owner-integrated permutations: page before fence, fence before page, unknown
after acceptance, newer paint after capture, and bundles with distinct output
snapshots. Test that allocation/cache eviction preserves storage while its
lease exists and that actual storage replacement still wakes the scene.

Upstream's `2026-09-04-post-merge-followups.md` explicitly leaves damage spanning
two outputs as an open concern. This review does not certify that scenario as
fixed. The 2c-iii plan must specify capture/ack ownership for both bundle and
separate scheduling and test the differing completion orders.

## Scope limits

Fresh validation passed on the combined source: exact CI clippy, nightly format
check, full workspace tests outside the sandbox, and musl/FreeBSD checks. The
first sandboxed workspace run stalled in the lightweight disconnect test and
was terminated. That test passed alone outside the sandbox, followed by the
complete workspace suite there; no source workaround or test exclusion was
needed. Logs are in the integration directory named above.

This pass reviewed the upstream delta and its intersections with the two 2c
drafts. It did not implement leases, run a desktop/hardware campaign, or close
the remaining allocation/destructor adapter design. The governing C.0 spec
retains its historical approved baseline; the 2c drafts identify the new
implementation baseline without rewriting the approval history.
