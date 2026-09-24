# Stage 3b-i-2 — client modeset on the Owner: direct, copied route, position, mixed server

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back), run with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters, each by its own command — `c0_3bi_`, `c0_3aii_`, `c0_3a_`, `c0_2b_add_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm` — with `--include-ignored` only when the prompt records the user's GPU approval, otherwise without it; **never** `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master: the hardware additions of Task 5 are **written, never run**, by the implementer; no deletes outside the worktree; remove temporary instrumentation before finishing; never edit `docs/status.md`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code or in C.0, stop and report it (F8); never silently substitute a test shape, never weaken an existing assertion.

**Revision 1 (2026-09-24, coordinator).**

**Goal:** remove every `OwnerRefused(NotYetSupported(..))` plan 3b-i-1 left:
an Owner client modeset works with a direct frame on the device, on the
copied route, as a position-only change and in a server that also has a
Legacy device; and prove that a modeset on one device leaves every other
device alone. At the end the `modeset` field of `TestWriterCoverageEvidence`
(`render/resources/transport.rs:135`) is proven. Production stays `Legacy`
(C0-R8).

**Prerequisite:** plan 3b-i-1 (`docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-1-plan-modeset-execution.md`)
accepted — its client-modeset slot, `TopologyWork`, description builder,
output-instance identity, retired-output bundle, preparation, infallible
promotion, old-pool release and failure table are this plan's base.

**Authority** (read before Task 1):
- Stage 3b design `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
  revision 9: §3.3 (position-only), §4.1 (copied route preparation), §5.1
  (retired copied pool table, in-flight work by identity), §5.2, §5.3, §5.4,
  §5.5, §8.1.
- Stage 2c-iii copied-route design `docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md`
  (ownership states, lifecycle-quiescence normalization).
- C.0 §9.2, §10.2, §13.

## Design decisions this plan fixes

1. Test names start with `c0_3bi_` (the sub-stage prefix); Vulkan tests end in
   `_vulkan` and use `owner_live_fixture` / `copied_owner_live_fixture`
   (`render/backend.rs:55010`); a **two-device** Owner fixture is added in
   Task 5 if none exists (the 3a-ii §5.3.1 second-device fixture is the model).
2. Tests come from production entries (`Backend::begin_crtc_config`,
   `route_owner_event_batch`), as in 3b-i-1.
3. Every gate as in 3b-i-1 (fmt, clippy ×3, the filters, `--lib`,
   `c0_2ci`, `yserver-core`, every integration file).

## Review Focus

1. A direct frame current on the device when a mode change arrives, and the
   client presents again during the unflip (Task 1
   `c0_3bi_present_during_modeset_unflip_stays_composed_vulkan`).
2. A copied output disabled while its copy is in flight at each stage (Task 2
   table).
3. A position change that also changes the root extent while another output
   of the device shows a direct frame (Task 3
   `c0_3bi_position_change_invalidates_direct_eligibility_vulkan`).
4. A Legacy `xrandr --off` in a mixed server while the Owner device has a
   composed flip in flight (Task 4 `c0_3bi_mixed_legacy_modeset_spares_owner`).
5. Two devices, a mode change on A while B's composed flip is in flight
   (Task 5 `c0_3bi_modeset_on_a_leaves_b_alone_vulkan`).

---

## Task 1 — the composed unflip before dispatch (design §5.3)

**Deliver:** preparation no longer refuses `DirectActive`. When a direct frame
is current on the modeset's device, or the modeset would make the topology
direct-ineligible (`direct_scanout_topology_eligible`, `render/backend.rs:4017`,
or the whole-root match of `direct_present_eligibility`), preparation requests
the Owner composed unflip (`request_direct_unflip`, `:2709`) on the device that
holds the direct frame, and the modeset is dispatched only after that unflip
**retired**. The unflip's terminal outcome is handed to the parked modeset:
retired → proceed; rejected before submit → `Failed`
(`Preparation(Unflip)`), nothing changed; `CompletionUnknown` → that device
`Poisoned`, the modeset `Failed`. Direct re-entry stays blocked until the
modeset's promotion (the existing entry probation restarts on the new
topology). The unflip is a stage of the request's bound (design §7.3).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_modeset_waits_for_unflip_retirement_vulkan` | direct frame current, mode change: no modeset validation until the unflip's composed commit retired; then the modeset dispatches | **F1** dispatch while the unflip is pending |
| `c0_3bi_unflip_outcomes_reach_the_modeset_vulkan` | the unflip rejected, and (separately) made `CompletionUnknown`: the modeset ends `Preparation(Unflip)` / `Failed` with the device `Poisoned`, and nothing of the modeset was sent | **F2** proceed after a rejected unflip |
| `c0_3bi_present_during_modeset_unflip_stays_composed_vulkan` | a direct-eligible Present arrives between the unflip request and the modeset promotion: it is composed, not flipped | **F3** re-admit direct before promotion |

## Task 2 — the copied route (design §4.1, §5.1 table, §5.4)

**Deliver:** preparation accepts a copied route: the new pool is allocated on
the source (render) and sink (KMS) devices through the 2c-i/2c-iii resource
ownership, and the commit is issued only on the sink device. The retired-output
bundle holds the whole `OutputScanout::Copied` pool, and every copied
completion (`prepare_owner_copy_after_render_completion`,
`render/copied_owner.rs:312`, and the render-completion path around
`render/scene.rs:4531`/`:4557`) resolves by output instance (3b-i-1 Task 3).
A retired copied frame follows design §5.1's table exactly: A in flight →
wait for A's fence; A completed and B never prepared → no B is ever prepared,
the handoff is cancelled, the source normalized to `RendererDiscard`; B in
flight → wait for B's fence and receipts; B completed and never submitted to
KMS → source released on B's read obligation
(`release_source_after_read_retirement`, `copied_owner.rs:44`), destination
normalized to local discard; B submitted to KMS → the displacing commit's
`KmsRelease`, then the FOREIGN return. The normalization is 2c-iii's
`reset_after_lifecycle_quiescence` (`vk/scanout.rs:1339`) applied per frame on
per-fence proof. The bundle destroys the pool on both devices only after every
frame's last row.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_copied_mode_change_vulkan` | a copied output's mode change: the pool is prepared on both devices, only the sink device receives a commit, the promoted state matches Legacy's | **F4** issue a commit on the source device |
| `c0_3bi_retired_copied_frame_stages_vulkan` | disable and mode change with a frame at each stage (a) A in flight, (b) A done / B never prepared, (c) B in flight, (d) B done / never submitted: each follows its row, no B is prepared for a retired frame, the pool survives until every frame's last row | **F5** prepare B for a retired frame (design mutation 29); **F6** destroy the bundle before every frame's proof |
| `c0_3bi_late_copy_completion_vulkan` | a copy job completes after its output was disabled and an index shift, and after a mode change: proofs serviced from the bundle, nothing offered or submitted, the new pool untouched | **F7** route the copied completion by index or key (design mutation 25) |

## Task 3 — position-only changes (design §3.3)

**Deliver:** a request that changes only `x`/`y` of an enabled output is a
logical transaction: it takes the client-modeset slot, is admitted on
`Tier::Topology` with an **empty** description, sends **no KMS call** (no
`TEST_ONLY`, no commit), and promotes only when the device slot holds no
`Submitting` or accepted record. Promotion updates the existing
`OutputSceneState` in place (origin, full damage) — same object, same ring,
releases and owner buffers — plus `platform.outputs`, the registry, the root
and input extents, and the topology generation (queued older-generation
composed intents invalidated). A change of root extent or of direct
eligibility follows Task 1. Reply, timestamps and events match Legacy's.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_position_only_sends_no_kms_call_vulkan` | move an output: admitted on `Tier::Topology`, no executor send of any kind, state and registry equal Legacy's | **F8m** dispatch it to KMS (design mutation 13) |
| `c0_3bi_position_only_waits_behind_an_accepted_flip_vulkan` | an accepted composed flip held through the request: promotion and the result wait until it completes; a composed intent queued for the old origin is invalidated | **F9** promote without waiting for the slot (design mutation 24) |
| `c0_3bi_position_only_updates_in_place_vulkan` | with an unsignalled deferred release and a current composed buffer: the scene state is the same object after, the release waits for its fence, the buffer keeps its owner | **F10** replace the scene state (design mutation 22) |
| `c0_3bi_position_change_invalidates_direct_eligibility_vulkan` | a position change that grows the root while a direct frame is current: the unflip (Task 1) retires before promotion | **F11** promote while direct is current |

## Task 4 — the Legacy path in a mixed server (design §5.5)

**Deliver:** a modeset on a **Legacy** device while any device is `Owner`
scopes every server-wide Legacy step — the all-off
(`quiesce_before_topology_mutation`, `render/backend.rs:3649`), the event
drain, the scene drain, the BO reset, the CRTC-config epoch bump, the relight
(`relight_after_direct_teardown`) and the failure recovery
(`recover_failed_crtc_config`) — to the Legacy devices, exactly as 3a-ii Task 4
did for DPMS (`legacy_devices`, the `_for_devices` helpers). The scene rebuild
on this path rebuilds only the Legacy devices' outputs and re-associates the
Owner devices' kept state by output instance (3b-i-1 Task 3). A server with no
Owner device runs today's code unchanged.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_mixed_legacy_modeset_spares_owner` | Legacy + Owner fixture, `xrandr --off` on the Legacy output while the Owner device has a composed flip in flight: no write reaches the Owner device, its flip completes, its scene state is the same object | **F12** run the unscoped all-off (design mutation 9) |
| `c0_3bi_pure_legacy_modeset_unchanged` | a Legacy-only server: the existing all-off/relight sequence and its characterization tests unchanged | **F13** take the scoped path without an Owner device |

## Task 5 — two devices, coverage and the hardware additions

**Deliver:** the two-device evidence of design §8.1 and the coverage flip.
The `modeset` field of `TestWriterCoverageEvidence` becomes proven only with
the evidence of both 3b-i plans cited in the change. The hardware test of
3b-i-1 (`c0_hw_3b_modeset_owner_on_card1_drm`) gains: a position change ×4, and
— if a second device is present — a mode change on one device while the other
composes, asserting the other device receives no lifecycle commit.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bi_modeset_on_a_leaves_b_alone_vulkan` | two Owner devices, a mode change on A while B has a composed flip in flight: zero lifecycle commits on B's executor, B's flip completes, B's scene state and pool are the same objects | **F14** rebuild every device's scene |
| `c0_3bi_enable_on_b_unflips_a_vulkan` | all outputs on A with a direct frame current, enable an output on B: A's unflip retires before B's modeset dispatches | **F15** dispatch on B while A's direct is current |
| `c0_3bi_modeset_writer_coverage_proven` | the coverage evidence reports `modeset` proven, citing the 3b-i tests | **F16** leave the field unproven while claiming the evidence |
