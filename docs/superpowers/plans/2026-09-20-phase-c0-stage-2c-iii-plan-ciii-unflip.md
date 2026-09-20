# Stage 2c-iii, plan Ciii — unflip, multi-device, route selection, hardware

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized for hardware work) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters (`c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_ci_`, `c0_conv_cir_`), never `_drm`, `render_acceptance`, unfiltered `--ignored`, or anything that modesets or takes DRM master while the user is looking at the screen; no deletes outside the worktree. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests and the mutations each must catch. Execute tasks in order, one per run. You can run the `_vulkan` tests yourself: nothing is done until its tests pass on the real GPU, in debug and release. Do not ask for approval; a real design choice the plan leaves open, or a claim here that does not hold in the code, is an F8 stop you report.

**Revision 9 (2026-09-20)** — incorporates codex round 8
(`../findings/2026-09-20-stage-2c-iii-plan-ciii-review-round8.md`: **0
blocking**, 2 major, both verified against the tree by the author and
accepted; round 7's M-1 audited PARTIAL and closed here). Both findings are
the same mistake — **the plan left a design choice to the implementer**, which
this plan's own honesty rule would have turned into an F8 stop mid-task.

- **M-1, confirmed: the barrier's proof is decided here, not by the
  implementer.** Revision 8 said "name the milestone or stop", which is not a
  contract. A bare owner milestone cannot serve: `HardwareComplete` carries
  only a commit id, and it is the **scene** that privately owns the
  output/generation transaction and, per member, either applies the repaint
  (`retire_owner_damage_member`) or invalidates that output because the
  member could not be confirmed (`scene.rs:2357`-`2398`). The proof is
  therefore **scene-produced**, per output, on the applied branch only, and
  Task 5 consumes it; submission never discharges an output, and an
  invalidated member never does either.
- **M-2, confirmed: the P3-3 step could not retain anything.** Step 3's unflip
  makes the composed resources current, so a direct commit after it takes
  **composed** as its old state and shares no allocation with its new state —
  retention is recognised only when the same allocation key appears in old and
  new for the same member (`resources/commit.rs:654`-`668`). The retaining
  commit is now a **second direct commit of the same source buffer while the
  first is still current**, placed before the unflip, with the setup commit and
  the P3-3 commit named apart.

**Revision 8 (2026-09-20)** — incorporates codex round 7
(`../findings/2026-09-20-stage-2c-iii-plan-ciii-review-round7.md`: **0
blocking**, 1 major, verified against the tree by the author and accepted;
round 6 audited APPLIED at the type, consumer, test and mutation levels — the
`CommitKey` closed that family).

- **M-1, confirmed: the re-entry barrier had no cumulative owner.** Task 5 said
  re-entry stays blocked "until a composed frame has been presented on every
  affected output" but named neither the per-output proof nor anything that
  accumulates it. The baseline clears `reentry_blocked_until_composed` only
  when **one** `scene.tick` returns as many composed outputs as the device has
  (`backend.rs:21679`-`21686`) — a same-tick submission count. With output A
  repainting on tick 1 and B on tick 2, no tick ever returns both and re-entry
  stays blocked forever; relaxing it to "any composed result" would let direct
  re-enter before B's repaint, which DMG-5 forbids. Task 5 now carries the
  affected-output set and discharges it per output, with a staggered test and
  T45.

**Revision 7 (2026-09-20)** — incorporates codex round 6
(`../findings/2026-09-20-stage-2c-iii-plan-ciii-review-round6.md`: 1 blocking,
verified against the tree by the author and accepted; round 5 audited APPLIED).

- **B-1, confirmed: the direct frame's ordinary milestones.** Rounds 4, 5 and 6
  each found one more consumer correlating by a bare commit number — this time
  the pending direct frame's normal path: `managed_record_direct_presented`
  matches it by `commit_id == Some(commit)` with no device
  (`backend.rs:2844`-`2856`), and `managed_enqueue_retired_direct_completion`
  takes **no argument at all** and unconditionally `take()`s the pending frame
  (`backend.rs:2878`-`2899`), so device B's `CompletionRetired` would publish
  device A's Present and release A's preceding frame without A's own proof.
- **The fix is structural, so this stops being a discovery loop.** Task 1 no
  longer relies on an enumeration alone: it introduces a **`CommitKey`**
  (device + `CommitId`) and makes every correlating consumer take it, so a
  site that still correlates by a bare `CommitId` **fails to compile** rather
  than waiting for another review round. The enumeration below becomes the
  migration checklist, not the safety net — the same move the Ci-refactor made
  with `OwnerBuffer`. Any correlation left device-blind on purpose is named in
  the task's report.

**Revision 6 (2026-09-20)** — incorporates codex round 5
(`../findings/2026-09-20-stage-2c-iii-plan-ciii-review-round5.md`: 1 blocking,
verified against the tree by the author and accepted; round 4's B-1 audited
APPLIED and its M-1 PARTIAL, closed here).

- **B-1, confirmed: the identity inventory was still short by a module.**
  `SceneCompositor` keys `owner_damage_transactions` by a bare `CommitId`
  (`scene.rs:1080`-`1081`) and refuses an installation whose number is already
  present (`:1907`), and the owner-buffer transitions scan every output for
  `buffer.commit_id() == Some(commit)` with no device (`:2085`, `:2175`,
  `:2217`), although the routing has the device. With two owners both minting
  commit 1, device B's `Accepted` would accept **A's** damage transaction and
  buffer, B's `HardwareComplete` could apply and remove A's transaction, and
  B's own installation would be refused because A holds the number. `scene.rs`
  joins the inventory, with the damage key and every owner-buffer scan
  qualified by `(DrmDeviceKey, CommitId)` and T42/T43 to catch dropping it.
- **The tasks are reordered so nothing is built on an unqualified key.**
  Commit identity becomes **Task 1** and multi-device isolation **Task 2**;
  the unflip tasks follow as 3-6, route exclusivity is 7 and the hardware test
  stays 8. Round 5 asked that the unflip retirement's recorded identity use
  the qualified key; doing the identity work first makes that automatic
  instead of a revisit. **References in the revision blocks below use the task
  numbers of their own revision.**

**Revision 5 (2026-09-20)** — incorporates codex round 4
(`../findings/2026-09-20-stage-2c-iii-plan-ciii-review-round4.md`: 1 blocking,
1 major, both verified against the tree by the author and accepted; round 3
audited APPLIED throughout).

- **B-1, confirmed: the retry had no tick edge.** Revision 3 gave the plan two
  fork points, and "retried at the fork point" then read as the **request**
  funnel — which is cause-driven (`backend.rs:2315`), so a one-shot cause
  (a failed successor send) whose materialization fails would never be retried
  and the unflip would stall forever. The retry edge is now named exactly: the
  **transaction** fork in `maybe_composite` (`backend.rs:21586`), which every
  tick reaches while the unflip is requested. T39 catches putting it back in
  the funnel.
- **M-1, confirmed: other consumers still correlate by a bare commit number.**
  Beyond the resource maps of round-3 B-2, three more sites drop the device:
  the `Terminal` handler scans `present_dispositions` comparing only
  `key.commit` although `PresentKey` carries a device
  (`resources/present.rs:4`-`8`, `resources/commit.rs:394`-`400`); the event
  routing has `device_key` and does not pass it to the consumer
  (`backend.rs:20361`); and the pending direct frame records only a `CommitId`,
  which `managed_enqueue_unknown_direct_completion` matches without a device
  (`backend.rs:588`, `:2921`). Device B's terminal event could therefore skip
  device A's Present and release A's pins. The identity work is now **Task 7**
  of its own, with that inventory, its own test and T40/T41; the hardware test
  becomes Task 8.

**Revision 4 (2026-09-20)** — incorporates codex round 3
(`../findings/2026-09-20-stage-2c-iii-plan-ciii-review-round3.md`: 2 blocking,
1 major, all three verified against the tree by the author and accepted;
rounds 1 and 2 audited APPLIED, with round-1 B-1 marked TRADED and closed
here).

- **B-1, confirmed: the request path would recurse.** `admission_request_unflip`
  itself calls `request_direct_unflip` (`admission.rs:727`), so making the
  funnel fork into it is a cycle that never reaches a wake. Decision 3 now
  fixes an acyclic boundary: the funnel owns the legacy flags, the admission
  primitive never calls the funnel, and that callback is removed — with its two
  existing test callers (`backend.rs:55683`, `:55711`) named, because they
  depend on the effect it has today.
- **B-2, confirmed: commit ids collide across devices.** Each
  `DeviceCommitOwner` mints `CommitId`s from **its own** allocator starting at
  1 (`owner/device.rs:217`, `identity.rs:136`-`150`), while the backend keeps
  **one** `CommitResourceConsumer` (`backend.rs:1501`) whose correlation maps
  are keyed by a bare `CommitId` (`resources/commit.rs:130`-`145`) and whose
  matching compares `res.commit_id == Some(commit)` with no device
  (`:286`). Device A's cached `HardwareComplete` is therefore consumed by
  device B's commit of the same number, discharging B's `KmsRelease`
  obligations without B's own completion — §6.2's isolation and §3.2 both
  broken. Task 6 gains the namespacing and the interleaved two-owner test with
  **equal numeric** ids.
- **M-1, confirmed: no capacity rollback on a failed unflip dispatch.** The
  shared failure handler returns the old resources with
  `current_resources.extend(old)` (`admission.rs:1150`) and never restores
  their `DirectRole`, so a failure after the exit-retirement move would leave a
  buffer in the current collection holding an `ExitRetirement` role while it is
  still on screen. Task 2 gains the rollback invariant and T35.

**Revision 3 (2026-09-20)** — incorporates codex round 2
(`../findings/2026-09-20-stage-2c-iii-plan-ciii-review-round2.md`: 1 blocking,
1 major, both verified against the tree by the author and accepted; round 1:
five APPLIED and M-2 TRADED, closed here).

- **B-1, confirmed and dangerous.** Revision 2's routine gate
  `cargo test --lib c0_conv_ciii_ -- --include-ignored` **does** match
  `c0_conv_ciii_owner_route_on_card1_drm` by substring, so the sentence
  claiming it never selects the `_drm` test was false and the implementer
  would have taken DRM master while the user is on the screen. The hardware
  test is renamed **out of** the routine prefix — `c0_hw_ciii_owner_route_on_card1_drm`
  — so no filter the implementer is allowed to run can select it, and the
  coordinator selects it by its own name.
- **M-1, confirmed.** Revision 2's route-exclusivity mutations could not
  produce the observation it demanded: `allows_legacy` is state-only and is
  `false` under `Owner` (`resources/transport.rs:412`-`414`), and each sink
  returns before building its atomic request (`modeset.rs:1718`-`1733`), so a
  forced legacy branch changes no plane and the only visible effect is the
  gate's refusal — which spec §6.3 forbids as the observation. Task 5 now
  defines the observation: a recorder **inside each real sink, before any
  permit check**, counting that the legacy sink was entered at all.

**Revision 2 (2026-09-20)** — incorporates codex round 1
(`../findings/2026-09-20-stage-2c-iii-plan-ciii-review-round1.md`: 2 blocking,
4 major, **all six verified against the tree by the author and accepted**).

- **B-1, three parts, all confirmed.** (a) Revision 1 put the exit-retirement
  move in the **request**, while readiness requires that role **vacant**: the
  seam reserves `ExitRetirement` and moves `Current` into it
  (`backend.rs:20900`-`20930`) and the snapshot then reports
  `ExitRetirementOccupied` for exactly that role (`admission.rs:827`-`830`,
  `:979`-`:987`), so the unflip could never be admitted. The move now belongs
  to the **dispatch**, where the commit's old state is taken; decision 2.
  (b) Revision 1 promised a retry of a failed materialization but named no
  operation that retries it; decision 2 now names the site, and the snapshot
  only reads the flag. (c) No production cause reached
  `admission_request_unflip` — its only callers are tests
  (`backend.rs:55683`, `:55711`), while every real cause calls
  `request_direct_unflip`, which sets `scanout_m2` flags and nothing else
  (`backend.rs:2315`-`2325`). Spec §6.1 requires those causes to enter the
  owner request; decision 3 gives them one entry.
- **B-2 and M-1, confirmed, and the copied route leaves this plan.** The
  shared owner route forks in the **scene's render-completion handler**, which
  pushes a `ComposedOffer` and **drops the producer fd** (`scene.rs:3868`-
  `3875`); the copied route instead performs the sink copy **inside**
  `submit_copied_scanout` and hands its exported fence to KMS as
  `IN_FENCE_FD` (`platform.rs:6381` doc comment), and owner IPC transfers no
  descriptors (`executor/transport.rs:61`-`64`). Converting it is therefore
  not a fork but a producer restructuring — split the copy from the flip and
  make the sink copy's completion the readiness — and M-1 showed its
  exclusivity case cannot exist before that work. It becomes **its own plan**,
  written after this one is accepted; see "Limits stated". Stage 2c-iii is not
  complete until that plan is accepted.
- **M-2, confirmed.** §6.4's hardware run was coordinator prose with no test
  and no name, which §8.2 does not allow. It became a task of its own, with its
  test named, its filter fixed and its two mutations numbered (Task 7 then,
  Task 8 since revision 5).
- **M-3, confirmed.** The three `cargo check --workspace --target` gates of
  spec §8.4 were missing; they are in the gate block below.
- **M-4, confirmed.** Revision 1 had the cursor invariant backwards.
  `submit_composed_scanout` writes **primary-plane properties only**
  (`modeset.rs:1722`-`1770`): the cursor survives the legacy unflip by **not
  being in the request**. Task 4 now requires the same of the owner
  transaction, and T17/T18 add a property instead of omitting one.

**Revision 1 (2026-09-20)** — first draft.

**Goal:** Finish the conversion of stage 2c-iii's producers. The owner route
gains its third producer — the unflip back to composed — and with it the return
path's composed invalidation that plan Cii's third F8 stop handed forward. Then
the two properties the stage has claimed but never demonstrated: that one
device's conductor is self-contained, and that on an `Owner` device no primary
or unflip legacy write is issued from any converted submit site. Last, the tty2
hardware run of spec §6.4 with P3-2/P3-3.

**Architecture:** The unflip is a producer, so it takes the same shape Ci and
Cii took (user's constraint, spec §8.3): its owner-route code lives in **its own
module** — `crates/yserver/src/kms/render/unflip_owner.rs` — reached from **one
fork point** for the request (`request_direct_unflip`, `backend.rs:2315`, the
funnel every cause already uses) and **one** for the transaction (the
`submit_composed_unflip` call site in `maybe_composite`, `backend.rs:21587`).
`submit_composed_unflip` (`backend.rs:3295`) and the legacy degraded fallback
stay exactly where they are. The 2c-i seams this plan finally drives from
production are `managed_handle_direct_unflip` (`backend.rs:20890`,
`#[allow(dead_code)]` today) and `managed_can_enter_direct`; they are not
rewritten unless a task says so. Production is unchanged (C0-R8): no conductor
is installed there, so every production device stays `Legacy`.

**Spec:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`,
revision 4 with §8.3 as amended: §6.1, §6.2, §6.4, §3.2 and §8.2's rows for
those sections. §6.3's copied-route half is the follow-on plan's. Read the Cii
acceptance (`../findings/2026-09-20-stage-2c-iii-plan-cii-accepted.md`),
especially "Carried to Ciii" and the three F8 stops, before the first task.

## Design decisions this plan fixes

1. **The unflip is a producer with its own module and one fork point per half**
   (user, spec §8.3). `maybe_composite` keeps deciding *that* an unflip is due
   — the requested flag and the "never race a composed commit against the
   direct transaction" gate (`backend.rs:21578`-`21585`) — and then asks once
   whether this device takes the owner route. The owner half lives in
   `unflip_owner.rs` and may reuse `composed_commit.rs`'s plane and description
   helpers; the legacy half stays where it is.
2. **What the request does, and what the dispatch does (round-1 B-1a, B-1b).**
   The 2c-i seam `managed_handle_direct_unflip` does four things
   (`backend.rs:20890`-`20940`): it terminalizes unsent direct work, moves
   `Current` into a freshly reserved `ExitRetirement`, materializes the direct
   shadow, and recomputes `reentry_blocked_until_composed`. On the owner route
   they do **not** all belong to the request:
   - **Request:** terminalize unsent direct work, materialize the shadow,
     recompute re-entry. Nothing that occupies a capacity role.
   - **Dispatch:** the exit-retirement move, as part of taking the commit's
     **old state**. The old state is the current direct resources of the
     members the unflip covers, and the role they carry while they retire is
     `ExitRetirement` — which is what 2c-ii's readiness means by "the
     exit-retirement position is free" (`admission.rs:827`-`830`): no *other*
     exit retirement is in flight. Reserving it in the request makes the
     unflip wait on itself forever, which is round-1 B-1a.
   - **Retry (round-4 B-1):** a failed materialization is retried at the
     **transaction** fork — `maybe_composite`'s owner branch, at the
     `submit_composed_unflip` call site (`backend.rs:21586`) — **not** at the
     request funnel, which is cause-driven and may never be entered again.
     Every tick reaches that branch while `scanout_m2.active()`, the unflip is
     requested and a direct frame is current (`backend.rs:21578`-`21586`).
     There the owner route retries **materialization only** — no second
     intent, no second terminalization — and wakes admission; it commits only
     once the shadow is ready. `admission_snapshot` only **reads**
     `unflip_shadow_ready`; a readiness computation has no side effect.
3. **One request entry for every cause, and it must be acyclic (round-1 B-1c,
   round-3 B-1).** Today `admission_request_unflip` (`admission.rs:713`) has
   only test callers, while every real cause — cursor fallback, cursor bind
   failure, a failed successor send, overlay and topology invalidation, the
   composite-tick reasons — calls `request_direct_unflip` (`backend.rs:2315`;
   about twenty call sites). `request_direct_unflip` therefore becomes the
   **per-device request entry**: it keeps its legacy effects and, on an `Owner`
   device, forks once into the owner half. Spec §6.1 requires exactly that. No
   cause is edited; the funnel they already share is what forks.
   **The direction is one-way.** `admission_request_unflip` calls
   `request_direct_unflip` today (`admission.rs:727`), so forking the funnel
   into it without removing that call is infinite recursion. The boundary:
   **the funnel owns the legacy flags; the admission primitive never calls the
   funnel.** That callback is removed, and the two existing test callers
   (`backend.rs:55683`, `:55711`), which today get the flag effects through it,
   are moved onto the funnel or given the effect explicitly — the task says
   which, and neither is deleted. A second cause arriving while an unflip is
   already requested is harmless: no second intent, no second terminalization,
   no second materialization.
4. **One transaction replacing the complete plane set** (§6.1). The owner
   description covers **every** CRTC of the device with that output's retained
   composed framebuffer (`retained_composed_framebuffer`), never a per-CRTC
   subset: AMD rejects independent per-CRTC replacement with `ENOSPC`, which is
   why `submit_composed_unflip` exists in the shape it has. The grouping
   precondition is the one the legacy path checks first,
   `direct_scanout_topology_eligible` (`backend.rs:3457`).
5. **No degraded fallback on an `Owner` device.** When the atomic unflip fails,
   Legacy degrades to per-output composed scene flips (`backend.rs:21588`-
   `21600`). Those are `WriterClass::Primary` legacy writes, and §6.3 forbids
   them on an `Owner` device. A failed owner unflip dispatch therefore takes
   the Ci-refactor's **shared dispatch-failure path** (its decision 4) and
   fails closed. This is a stated door, not an oversight: recovering a device
   whose transport closed while it scans out a client buffer is stage 4's
   lifecycle work, and C0-R8 keeps it out of production.
6. **The return path belongs to the unflip commit's retirement** (Cii F8 3,
   DMG-5). On the legacy path the return effects hang off counting per-output
   retirements (`retire_direct_output`, `backend.rs:3355`-`3378`): stop direct,
   block re-entry until composed, `invalidate_all_scanout_damage`, mark the
   scene structure dirty. On the owner route the **unflip commit's own
   retirement** runs them, once, and the retired commit is recognised as the
   unflip from what its dispatch recorded — never by re-deriving it from scene
   state, and never by a second counter. Until that retirement no composed
   buffer of an affected output may be scanned out again without a full
   repaint; the fixture proves it **per output** (spec §6.1, round-1 M-3 of the
   design review).
7. **The unflip transaction carries no cursor and no gamma property**
   (round-1 M-4). `submit_composed_scanout` writes primary-plane properties
   only (`modeset.rs:1722`-`1770`); the cursor is preserved because it is
   absent from the request, and the gamma likewise. The owner transaction must
   be the same, and the mutations **add** a cursor or gamma property rather
   than omitting one. This plan adds no cursor or gamma producer: those are
   stage 4's.
8. **Route selection is read per site, per device** (§6.3). Every converted
   submit site of §4.1, §5.3 and §6.1 reads the transport state of **its own**
   device before it writes. Each site carries its own mutation ("force the
   legacy branch"), and the mutation must break a named test **by an
   observation other than the transport gate's refusal** — the gate is the
   defence the fork is supposed not to need. The enumerated sites are fixed in
   Task 7 and the enumeration itself is the scope boundary.
9. **Multi-device state is already per device; what Ciii owes is the
   evidence** (§6.2). `admission_conductors` is a `BTreeMap<DrmDeviceKey,
   AdmissionConductor>` and each conductor carries its own admission, layout
   generation, composed intents, maintenance store and receipts
   (`admission.rs:266`-`286`); the transport gate and the owner are per device
   in the platform. The device-blind helpers are known and are not all
   defects: `admission_note_layout_change_all_devices` (`admission.rs:793`) is
   device-blind **by design** and `scanout_m2` is one direct group for the
   whole backend. A device-blind path that changes **another** device's
   conductor, gate, owner or damage state is a defect, fixed in the task that
   finds it; a device-blind path that is deliberate is recorded in the task's
   report, not rewritten.
10. **Test names start with `c0_conv_ciii_`**, so one filter selects this plan.
    `_vulkan` tests carry `#[ignore = "needs live Vulkan ICD"]` and build on
    Ci's owner-route live fixture (`for_tests_with_vk_live_scene_real_drm`,
    `backend.rs:6843`; the plain variant has no real DRM node). That fixture
    asserts **exactly one** KMS device (`backend.rs:6898`), so the
    multi-device evidence of Task 2 is not built on it; `test_kms_device` plus
    `install_transport_gate` is the shape the platform's own multi-device tests
    already use (`platform.rs:8798`, `platform.rs:9797`). The hardware test of
    Task 8 is named **outside** this prefix — `c0_hw_ciii_owner_route_on_card1_drm`,
    `#[ignore]`d, `_drm` suffix — so no filter the implementer may run selects
    it (round-2 B-1), and **codex does not run it**: it needs DRM master from
    tty2.

## Limits stated

- **The copied composed route is a separate plan** (round-1 B-2/M-1), written
  after this one is accepted: the sink copy must be split from the flip and its
  completion must become the readiness, because the owner route drops the
  producer fd and owner IPC carries no descriptors. Until then a device with an
  output on the copied route, or on an unmanaged scanout pool, cannot enter
  `Owner` and stays `Legacy` (spec §2.1), so §6.3's exclusivity still holds on
  every `Owner` device. **Stage 2c-iii is not complete until that plan is
  accepted.**
- Cursor and gamma **producers**, the cursor coordinate lane, topology and
  cursor-recovery dispatch: stages 3 and 4. `Topology` and `CursorRecovery`
  stay `Unsupported` here.
- C.0 §16.3 revision 5 is owed before stages 3/4, not here.
- The Ci F8 stops 1-3 stay open and are not re-litigated: the Legacy dormancy
  bug, the missing restore `TerminalState`, device loss without an owner
  signal. Ci's F8 4 (no fixture builds a copied output) goes to the copied-route
  plan.
- Recovery from a transport closed by a failed dispatch is stage 4's
  (decision 5).

## Global Constraints

- **Production is byte-for-byte unchanged**: without an active conductor the
  unflip, composed, direct and promotion paths take today's calls with the same
  arguments, the same pins and the same ordering. Each task keeps a named
  Legacy characterisation test green.
- Owner milestones reach the scene only through `route_owner_event_batch`;
  tests deliver them that way (a stub behaviour or crafted events handed to it
  — say which), never by calling a handler directly.
- No hand-built `PendingAck`, `BoPhase`, `OwnerBuffer`, `DirectPresentFrame`,
  scanout pool or prepared generation in any test.
- Resources travel by value; nothing is bare-dropped; every token is consumed
  exactly once; confirmation is at the send; no retry on refusal.
- No side effect inside `debug_assert!`, and none inside a readiness
  computation; fail closed, never panic, in non-test code; no test-only hook
  that bypasses the path it is named after.
- **Honesty rule (F8).** An unreachable scenario, a seam that does not behave
  as stated, a fixture that cannot carry what a test needs, or a real design
  choice left open: stop and report.

## Checks every task must keep green

```bash
cargo build -p yserver --bin yserver
cargo build --release -p yserver --bin yserver
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features tcp-transport -- -D warnings
cargo clippy --all-targets --features xdmcp -- -D warnings
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_conv_ciii_; done
cargo test -p yserver --lib c0_conv_ciii_ -- --include-ignored --test-threads=1
cargo test --release -p yserver --lib c0_conv_ciii_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_cii_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_ci_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_conv_cir_ -- --include-ignored --test-threads=1
cargo test -p yserver --lib c0_adm
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

None of these filters selects Task 8's hardware test: it is named outside the
`c0_conv_ciii_` prefix, as `c0_hw_ciii_owner_route_on_card1_drm` (round-2 B-1),
precisely so that no filter the implementer is allowed to run can reach it. It
takes DRM master from tty2 and is the coordinator's (decision 10).

**Portability (spec §8.4, round-1 M-3).** The **last task** (Task 8) and the coordinator
at acceptance additionally run, for `x86_64-unknown-linux-gnu`,
`x86_64-unknown-linux-musl` and `x86_64-unknown-freebsd`:

```bash
cargo check --workspace --target <target>
```

Baseline before Task 1 (commit `5a34c6ec`): `c0_conv_ci_` 38/38 and
`c0_conv_cii_` 26/26 with `--include-ignored` in debug and release;
`c0_conv_cir_` 7/7; `c0_adm` 129/0; `c0_2ci` 180/0/21; `--lib` 1933/0/142.
Every task ends at those numbers plus its own new tests. The full hardware gate
(306/306 at `5a34c6ec`) is the coordinator's, after the last task, with the
user's go-ahead.

## Exit criteria

| Criterion (spec) | Tests | Mutation that must fail them |
| --- | --- | --- |
| Every production cause of an unflip reaches the owner request on an `Owner` device, and only the legacy flags on a `Legacy` one, through an **acyclic** entry (§6.1, decision 3; round-3 B-1) | `c0_conv_ciii_every_unflip_cause_reaches_the_owner_request_vulkan` (at least three distinct causes, one raised twice) | T1: fork only for the cursor cause; T2: take the owner request under `Legacy`; T38: restore the admission primitive's call back into the funnel |
| An unflip is not admitted until the exit-retirement position is free, every affected output has its retained composed framebuffer **and** the direct shadow is materialized (§6.1) | `c0_conv_ciii_unflip_readiness_waits_on_each_precondition` (one case per precondition, each with the others satisfied) | T3: report `Ready` while the shadow is unmaterialized; T4: report `Ready` while a **foreign** exit retirement is in flight |
| The request materializes the shadow and terminalizes unsent direct work, and occupies no capacity role (§6.1, decision 2) | `c0_conv_ciii_unflip_request_prepares_without_occupying_capacity_vulkan` | T5: reserve `ExitRetirement` in the request, as revision 1 did |
| A failed materialization leaves the unflip requested, unadmitted and retried on the next tick even when its cause is one-shot, never dispatched (§6.1, decision 2; round-4 B-1) | `c0_conv_ciii_unflip_shadow_failure_defers_admission_vulkan` | T6: treat a failed materialization as ready; T7: retry it inside the readiness computation; T39: retry only in the request funnel, so a one-shot cause stalls |
| `Unflip` is dispatched; `Topology` and `CursorRecovery` stay `Unsupported` (§6.1) | `c0_conv_ciii_unflip_dispatches_and_others_stay_unsupported` | T8: keep `Unflip` in `decision_requires_unsupported`; T9: drop `Topology` from it as well |
| The unflip commit's old state is the current direct resources of the members it covers, carrying the exit-retirement role (§6.1, 2c-i §8.5, decision 2) | `c0_conv_ciii_unflip_commit_takes_the_exit_retirement_old_state_vulkan` | T10: leave the old state empty; T11: take the device's whole current state instead of the members' |
| The unflip commit replaces the **complete** plane set of its device in one transaction, each CRTC with that output's retained composed framebuffer (§6.1) | `c0_conv_ciii_unflip_commit_covers_every_crtc_vulkan` | T12: describe only the CRTCs named in the barrier; T13: skip the topology-eligibility precondition |
| The owner route takes its own module behind one fork point; Legacy is unchanged (decision 1) | `c0_conv_ciii_legacy_unflip_unchanged_vulkan`, `c0_conv_ciii_owner_unflip_commits_instead_of_submitting_vulkan` | T14: force `submit_composed_unflip` under `Owner`; T15: take the owner route under `Legacy` |
| A failed owner unflip dispatch fails closed and never degrades to per-output legacy flips (decision 5) | `c0_conv_ciii_unflip_dispatch_failure_fails_closed_vulkan` | T16: fall through into the degraded per-output path under `Owner` |
| A failed unflip dispatch leaves capacity as it was: old resources in `current_resources` with the `Current` role, no exit-retirement reservation outstanding (round-3 M-1) | `c0_conv_ciii_unflip_dispatch_failure_fails_closed_vulkan` | T35: skip the role restoration on the unflip failure row |
| The unflip commit's retirement stops direct scanout, blocks re-entry until composed, and invalidates every affected output's composed buffers exactly once (§6.1, DMG-5, Cii F8 3) | `c0_conv_ciii_unflip_retirement_returns_to_composed_vulkan` | T17: drop the invalidation on the owner return path; T18: run the return effects at `HardwareComplete` instead of `CompletionRetired` |
| Every affected output is repainted in full before it is scanned out again; **per output** (§6.1, DMG-5) | `c0_conv_ciii_unflip_return_repaints_each_output_in_full_vulkan` (at least two outputs, distinct damage before the direct entry) | T19: invalidate only the reference output; T20: apply the pre-entry damage to the returning composed buffer |
| Direct re-entry stays blocked until **every** affected output has proven its full repaint, across as many ticks as it takes (§6.1, DMG-5; round-7 M-1) | `c0_conv_ciii_staggered_return_holds_the_reentry_barrier_vulkan` | T45: clear the barrier after the first output's proof; T46: discharge an output at submission instead of at its proof |
| The proof is the scene's application of the repaint, not its submission and not an invalidated member (§6.1, DMG-5; round-8 M-1) | `c0_conv_ciii_an_invalidated_member_does_not_discharge_its_output_vulkan` | T47: emit the return proof on the invalidating branch as well |
| The retired unflip is recognised from what its dispatch recorded, not from scene state (decision 6) | `c0_conv_ciii_unflip_retirement_returns_to_composed_vulkan`, driven with an ordinary composed commit retiring first | T21: run the return effects for any retiring composed commit while an unflip is requested |
| The unflip transaction carries no cursor and no gamma property, so both survive it unchanged (§6.1, C.0 §12, decision 7) | `c0_conv_ciii_unflip_carries_no_cursor_or_gamma_vulkan` | T22: add a cursor-plane property to the transaction; T23: add a gamma property |
| On an `Owner` device no primary or unflip legacy write is issued, at any enumerated site (§6.3) | the enumeration of Task 7, each site with its own case in `c0_conv_ciii_owner_device_issues_no_legacy_primary_write_vulkan`, observed by the **sink-entry recorder**, never by the gate's refusal (round-2 M-1) | T24-T26: force the legacy branch at each enumerated site in turn |
| The sink-entry recorder itself observes entry, not authorization (round-2 M-1) | `c0_conv_ciii_sink_recorder_counts_entry_before_the_permit` | T34: move the recorder after the permit check |
| On a `Legacy` device behaviour is identical to today (§6.3) | `c0_conv_ciii_legacy_device_unchanged_vulkan` plus the full software gate | T27: read another device's transport state at one enumerated site |
| An event, wake, refusal or bound violation on one device changes nothing on another (§6.2) | `c0_conv_ciii_devices_are_independent` (one case per kind: owner event batch, admission wake, refused offer, bound violation) | T28: route the batch to every conductor; T29: close every device's gate on a bound violation |
| A device's layout generation, composed intents, maintenance and receipts belong to that device alone (§6.2) | `c0_conv_ciii_conductor_state_is_per_device` | T30: bump every conductor's layout generation on one device's change |
| A grouped direct unit never crosses devices (§6.2) | `c0_conv_ciii_direct_group_never_crosses_devices_vulkan` | T31: drop the single-device precondition from `direct_scanout_topology_eligible` |
| Two owners' equal numeric `CommitId`s never correlate across devices (§6.2, §3.2; round-3 B-2) | `c0_conv_ciii_equal_commit_ids_do_not_cross_devices` | T36: key the completion cache by `CommitId` alone; T37: match a releasing resource without comparing its device |
| A terminal or `CompletionUnknown` on one device leaves another device's Present disposition and pins alone (§6.2; round-4 M-1) | `c0_conv_ciii_foreign_terminal_leaves_a_pending_present_alone` | T40: drop the device comparison from the `present_dispositions` scan; T41: match the pending direct frame by `CommitId` alone |
| One device's milestones never accept, apply, remove or block another device's damage transaction or owner buffer (§6.2; round-5 B-1) | `c0_conv_ciii_foreign_milestones_leave_a_damage_transaction_alone` | T42: key `owner_damage_transactions` by `CommitId` alone; T43: scan owner buffers without comparing the device |
| A `Presented` or `CompletionRetired` on one device never samples, publishes, promotes or releases another device's pending direct frame (§6.2, §3.3; round-6 B-1) | `c0_conv_ciii_foreign_milestones_leave_a_pending_direct_frame_alone` | T44: publish the pending frame at retirement without matching its `CommitKey`, as today's no-argument helper does |
| On real hardware in `Owner`: a composed frame, a direct frame, the unflip back, and a commit retaining an allocation of the old state for the same member (§6.4) | `c0_hw_ciii_owner_route_on_card1_drm` | — (the run itself is the evidence; its two mutations are below) |
| A displaced buffer's `KmsRelease` is discharged by the real completion — P3-2 (§6.4, debt spec §9.3/§9.5) | the same test, steps 1-3 | T32: drop the displaced buffer's registration |
| A retained allocation registers no `KmsRelease` and is not released — P3-3 (§6.4) | the same test, step 4 | T33: register the retained allocation |

---

### Task 1: Commit identity is device-qualified

**Files:** `crates/yserver/src/kms/render/resources/commit.rs`,
`resources/present.rs`, `backend.rs` (the event routing and the pending direct
frame), `scene.rs` (the damage transactions and the owner-buffer scans); tests.

**Why this is a task and not an assumption.** Each `DeviceCommitOwner` mints
`CommitId`s from **its own** allocator starting at 1 (`owner/device.rs:217`,
`identity.rs:136`-`150`), so two owner devices issue the same numbers. The
consumers that correlate by those numbers are shared.

**The mechanism (round-6 B-1).** Correlation stops being possible by number:
this task introduces a **`CommitKey`** — a `DrmDeviceKey` and a `CommitId`
together — and **every consumer that correlates takes a `CommitKey`**, never a
bare `CommitId`. A site that still correlates by number then fails to compile,
which is what ends the per-round discovery of one more consumer. The owner's
own internal records, which never leave their device, may keep the plain id;
any other correlation deliberately left device-blind is **named in the task's
report** with why.

**The migration checklist (round-3 B-2, round-4 M-1, round-5 B-1, round-6
B-1).** Every site below is converted; it is a checklist, not the safety net —
the type is:
- the single `CommitResourceConsumer` (`backend.rs:1501`) and its
  `hardware_completed_commits`, `commit_members` and `reserved_retirements`,
  keyed by a bare `CommitId` (`resources/commit.rs:130`-`145`);
- its releasing/rejected-resource match, `res.commit_id == Some(commit)`, with
  no device (`resources/commit.rs:286`);
- the `Terminal` handler's scan of `present_dispositions`, comparing only
  `key.commit` although `PresentKey` already carries a device
  (`resources/present.rs:4`-`8`, `resources/commit.rs:394`-`400`), and the
  `Presented` consumer beside it;
- the owner-event routing, which holds `device_key` and does not pass it to the
  consumer (`backend.rs:20361`);
- the pending direct frame's recorded identity and
  `managed_enqueue_unknown_direct_completion`, which matches it by `CommitId`
  alone (`backend.rs:588`, `:2921`);
- the scene's `owner_damage_transactions`, keyed by a bare `CommitId`
  (`scene.rs:1080`-`1081`), whose installation refuses a number already present
  (`:1907`) and whose `Accepted`/`HardwareComplete`/retirement/terminal paths
  are called with the number alone (`:1956`-`:2004`);
- the owner-buffer transitions, which scan every output for
  `buffer.commit_id() == Some(commit)` without a device (`scene.rs:2085`,
  `:2175`, `:2217`);
- the pending direct frame's **ordinary** milestones (round-6 B-1):
  `managed_record_direct_presented`, which matches the frame by `CommitId`
  alone (`backend.rs:2844`-`2856`), and
  `managed_enqueue_retired_direct_completion`, which takes **no argument** and
  unconditionally takes the pending frame (`backend.rs:2878`-`2899`), together
  with the routing that calls them (`backend.rs:20328`-`20358`,
  `:20401`-`:20465`). Both take the frame's `CommitKey` and do nothing unless
  it matches.

Splitting the consumer per device is **not** the shape: `DirectCapacity` is one
direct group for the whole backend by design (decision 9). What changes is the
**key**, to `(DrmDeviceKey, CommitId)`; `route_owner_event_batch` already
carries the device at every consumption site.

**Invariants (spec §6.2, §3.2):** no device can discharge, cache, consume,
terminalize, accept, apply, refuse or restore another's work, even when both
owners issue the same numeric `CommitId`. A cached `HardwareComplete` belongs to one device; a
`CompletionRetired` of the same number on another device neither consumes it
nor discharges any `KmsRelease` obligation. A `Terminal` or `CompletionUnknown`
on one device neither changes another device's Present disposition nor
releases another device's pins, and a `Presented` or `CompletionRetired` on one
device neither samples, publishes, promotes nor releases another device's
pending direct frame — **release-before-proof is impossible across devices**. One device's damage transaction is neither
accepted, applied nor removed by another device's milestone of the same
number, and installing a transaction on one device is never refused because
another device already holds that number. Every later task's recorded commit
identity — the unflip retirement's included — uses this qualified key
(round-5 B-1).

**Named tests:**
- `c0_conv_ciii_equal_commit_ids_do_not_cross_devices` — two owners whose
  commits carry the **same numeric** `CommitId`, interleaved: A's
  `HardwareComplete` is cached, B's `CompletionRetired` of the same number
  arrives first, and B discharges nothing its own completion has not proven,
  while A's cache survives for A.
- `c0_conv_ciii_foreign_terminal_leaves_a_pending_present_alone` — A's direct
  Present is pending; B's commit of the same number reaches
  `FailedBeforeSubmit` and then `CompletionUnknown`; A's disposition and A's
  pins are untouched.
- `c0_conv_ciii_foreign_milestones_leave_a_pending_direct_frame_alone` — A's
  direct frame is pending **with its `Presented` sample already recorded**; B's
  commit of the same number delivers `Presented` and then `CompletionRetired`;
  A's pending frame, A's current frame, the publication queue and A's pins are
  all unchanged (round-6 B-1).
- `c0_conv_ciii_foreign_milestones_leave_a_damage_transaction_alone` — A and B
  each hold a live damage transaction and an owner buffer under the **same**
  numeric `CommitId`; B's `Accepted`, `HardwareComplete`, `Terminal` and
  `CompletionRetired` are interleaved, and A's transaction, A's buffer and A's
  damage are untouched; B's own installation is not refused (round-5 B-1).

- [ ] Steps: enumerate and report the sites; tests; red; implement; checks;
  stop dirty and report.

---

### Task 2: Multi-device

**Files:** `admission.rs`, `backend.rs` and `platform.rs` where a device-blind
path is found; tests.

**Interfaces:** none new unless a defect is found. Commit identity is **Task 1's**; this task proves the conductor, gate, owner and direct-group halves of
§6.2.

The fixture is **not** the live single-device one (decision 10): two seeded KMS
devices with their own gates, the shape `platform.rs:8798` and
`platform.rs:9797` already use.

**Invariants (spec §6.2):** one conductor per `DrmDeviceKey`, each with its own
admission, layout generation and transport state; an event, wake, refusal or
bound violation on one device changes nothing on another — not its layout
generation, not its composed intents, not its maintenance store, not its
receipts, not its gate, not its owner; a grouped direct unit never crosses
devices. Device-blind paths that are
deliberate (decision 9) are named in the report rather than rewritten; a
device-blind path that changes another device's state is a defect, fixed
here.

**Named tests:**
- `c0_conv_ciii_devices_are_independent` — four cases: an owner event batch, an
  admission wake, a refused offer, and a bound violation that closes a gate.
- `c0_conv_ciii_conductor_state_is_per_device`.
- `c0_conv_ciii_direct_group_never_crosses_devices_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 3: One request entry, and a complete readiness

**Files:** `backend.rs` (`request_direct_unflip` at `:2315`, the seam at
`:20890`), `crates/yserver/src/kms/render/admission.rs`
(`admission_request_unflip` at `:713`, `admission_snapshot` at `:810`),
`crates/yserver/src/kms/owner/admission/snapshot.rs` (the new `WaitReason`),
the new `unflip_owner.rs`; tests.

**Interfaces:** `request_direct_unflip` keeps its legacy effects and gains the
one fork into `unflip_owner`, which reaches `admission_request_unflip` for the
device of the direct group. The request half of the seam runs there:
terminalize unsent direct work, materialize the shadow, recompute
`reentry_blocked_until_composed`. `Unflip` readiness gains its third
precondition, with a `WaitReason` of its own, read from a flag the request (or
the fork point's retry) sets.

**Invariants (spec §6.1, decisions 2 and 3):** every production cause of an
unflip reaches the owner request on an `Owner` device and only the legacy flags
on a `Legacy` one; the call graph is **acyclic** — the admission primitive
never calls the funnel back (round-3 B-1) — and a repeated request while one is
already outstanding changes nothing; the request occupies **no** capacity role; an unflip is
admitted only when the exit-retirement position is free of any **other** exit
retirement, every affected output has its retained composed framebuffer, and
the shadow is materialized; a failed materialization leaves the unflip
requested, unadmitted and retried at the fork point on a later tick, and never
fails the request; `admission_snapshot` has no side effect.

**Named tests:**
- `c0_conv_ciii_every_unflip_cause_reaches_the_owner_request_vulkan` — at least
  three distinct causes, each driven through its real caller, and one of them
  raised **twice** (the idempotence half); the test would not terminate if the
  funnel and the primitive still called each other.
- `c0_conv_ciii_unflip_readiness_waits_on_each_precondition` — three cases,
  each with the other two satisfied, each naming its own `WaitReason`.
- `c0_conv_ciii_unflip_request_prepares_without_occupying_capacity_vulkan`.
- `c0_conv_ciii_unflip_shadow_failure_defers_admission_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 4: Unflip dispatch, in its own module

**Files:** `unflip_owner.rs`; `admission.rs`
(`decision_requires_unsupported` at `:181`, `admission_dispatch_decision` at
`:1077`); `backend.rs` at the transaction's fork point (`:21587`); tests.

**Interfaces:** `unflip_owner` produces the device's unflip `CommitDescription`
and its `CommitResources`, and is reached from `admission_dispatch_unflip`,
which is reached from `admission_dispatch_decision`. `Unflip` leaves
`decision_requires_unsupported`; `Topology` and `CursorRecovery` stay. The
failure handling is the Ci-refactor's shared mechanism, with the unflip's own
row; nothing new is added to the policy table beyond what the route needs.

**Invariants (spec §6.1, decisions 2, 4 and 5):** the commit's **old state** is
the current direct resources of the members the unflip covers — not the
device's whole current state (the 2c-ii defect Ci fixed) — and the role they
carry while they retire is `ExitRetirement`, reserved here and not in the
request; the description covers every CRTC of the device, each with that
output's retained composed framebuffer, in one transaction;
`direct_scanout_topology_eligible` is a precondition of the dispatch, not an
assumption; resources move by value; a dispatch failure fails closed through
the shared path, never reaches the degraded per-output fallback, and **leaves
capacity exactly as it was before the dispatch** — the old resources back in
`current_resources` carrying the `Current` role, with no `ExitRetirement`
reservation outstanding. The shared handler restores the vector but not the
role (`admission.rs:1150`), so the unflip's failure row restores it (round-3
M-1);
production's `submit_composed_unflip` and its degraded fallback are untouched,
including the `scanout_m2` bookkeeping they do (`unflip_awaiting_outputs`,
`degraded_composed_unflip`).

**Named tests:**
- `c0_conv_ciii_unflip_dispatches_and_others_stay_unsupported`.
- `c0_conv_ciii_unflip_commit_takes_the_exit_retirement_old_state_vulkan`.
- `c0_conv_ciii_unflip_commit_covers_every_crtc_vulkan` — two outputs, each
  named plane carrying that output's retained composed framebuffer.
- `c0_conv_ciii_legacy_unflip_unchanged_vulkan` and
  `c0_conv_ciii_owner_unflip_commits_instead_of_submitting_vulkan`.
- `c0_conv_ciii_unflip_dispatch_failure_fails_closed_vulkan` — the transport
  closes, no legacy write is issued, the direct frame's pins are still held,
  **and** the old resources are back in `current_resources` with the `Current`
  role and no exit-retirement reservation outstanding (round-3 M-1).

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 5: The return path

**Files:** `unflip_owner.rs`, `admission.rs` (the retirement routing),
`backend.rs` (the return effects, beside `retire_direct_output`'s legacy
counterpart at `:3355`-`:3378`), `scene.rs` (the per-output return proof);
tests.

**Interfaces:** the unflip commit's `CompletionRetired` runs the return
effects once: stop direct scanout, block re-entry until composed, invalidate
every affected output's composed buffers, mark the scene structure dirty. The
commit is recognised as the unflip from state its dispatch recorded.

**The re-entry barrier is cumulative and per output (round-7 M-1).** The
baseline clears `reentry_blocked_until_composed` only when **one**
`scene.tick` returns as many composed outputs as the device has
(`backend.rs:21679`-`21686`): a same-tick submission count, which a staggered
return never satisfies. On the owner route the unflip's retirement **records
its affected-output set**, and each output leaves that set when it supplies
its own proof; re-entry unblocks when the set is empty, however many ticks
that takes. Legacy's rule is untouched.

**The proof is scene-produced, per output, at application (round-8 M-1).** A
bare owner milestone cannot carry it: `HardwareComplete` names only a commit,
while the scene privately owns the output/generation transaction and, per
member, either applies the repaint through `retire_owner_damage_member` or
**invalidates** that output because its submitted generation could not be
confirmed (`scene.rs:2357`-`2398`). So the scene emits a **return proof**
keyed by the commit's `CommitKey` and the member's output, **only on the
applied branch**, and Task 5 discharges that output from the affected set when
it arrives. Submission never discharges an output; an invalidated member never
discharges its output, which therefore still owes its repaint.

**Invariants (spec §6.1, DMG-5, Cii F8 3):** the invalidation happens on the
**return**, at the unflip commit's retirement, and exactly once; every affected
output is invalidated, not only the reference one; each affected output is
repainted **in full** before it is scanned out again, and no damage recorded
before or during the direct interval survives into that repaint; an ordinary
composed commit retiring while an unflip is outstanding runs none of these
effects; direct re-entry stays blocked while any affected output still owes
its repaint, and unblocks once none does — never on a single tick's count,
never at submission, never on an invalidated member, and never before the last
output's proof.

**Named tests:**
- `c0_conv_ciii_unflip_retirement_returns_to_composed_vulkan` — also drives an
  ordinary composed retirement first (mutation T21).
- `c0_conv_ciii_unflip_return_repaints_each_output_in_full_vulkan` — at least
  two outputs, with distinct damage recorded on each before the direct entry;
  the assertion is per output.
- `c0_conv_ciii_staggered_return_holds_the_reentry_barrier_vulkan` — two
  outputs returning on **different ticks**, with `HardwareComplete` **withheld
  after submission**: the barrier holds while only submissions have happened,
  holds after A's proof alone, and clears only once B's proof arrives; then
  direct can re-enter (round-7 M-1, round-8 M-1).
- `c0_conv_ciii_an_invalidated_member_does_not_discharge_its_output_vulkan` —
  a `HardwareComplete` whose member cannot be confirmed invalidates that
  output and leaves it owing its repaint (round-8 M-1).

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 6: The cursor and the gamma the unflip must not touch

**Files:** `unflip_owner.rs`; tests.

**Interfaces:** none new.

**Invariants (spec §6.1, C.0 §12, decision 7):** the unflip transaction carries
**no** cursor-plane property and **no** gamma property, exactly as
`submit_composed_scanout` does (`modeset.rs:1722`-`1770`); the cursor is
therefore neither dropped nor flashed and the current gamma survives. This task
adds **no** cursor or gamma producer: those are stage 4's. The test asserts on
the submitted request's properties, not on a later readback.

**Named tests:**
- `c0_conv_ciii_unflip_carries_no_cursor_or_gamma_vulkan`.

- [ ] Steps: tests; red; implement; checks; stop dirty and report.

---

### Task 7: Route selection and exclusivity

**Files:** the enumerated submit sites; tests.

**Interfaces:** none new. The task's first product is **the enumeration
itself**, written into the task's report: every converted site that issues a
primary or unflip legacy write, with its file, line and `WriterClass`. The
enumeration is this criterion's scope boundary — a site missing from it is a
hole, so it is derived from the `allows_legacy` call sites of
`WriterClass::Primary` and `WriterClass::Unflip` and from the submit sites of
§4.1, §5.3 and §6.1, not from memory. As measured at `5a34c6ec` those are the
composed scene flip (`scene.rs:6330`), the direct submit and the retirement
promotion that shares it (`backend.rs:2695`, reached from
`submit_queued_direct_successor` at `:2709`) and the composed unflip
(`backend.rs:3327`). The copied scanout flip (`platform.rs:6400`) is **not**
in scope here: it belongs to the copied-route plan, and until then no device
with a copied output can enter `Owner` (round-1 M-1).

**The observation (round-2 M-1).** `allows_legacy` is state-only and returns
`false` under `Owner` (`resources/transport.rs:412`-`414`), and each sink
returns before building its atomic request (`modeset.rs:1718`-`1733`): a forced
legacy branch therefore changes no plane, and its only visible effect would be
the gate's refusal — which §6.3 forbids as the observation. So this task first
adds a **sink-entry recorder**: in each real legacy sink, as its **first**
statement, **before** any permit check, a `#[cfg(test)]` record of the entry
with its `WriterClass` and device. It must not change control flow, must not
depend on the permit's answer, and is scaffolding — add it to the C.0
structural-debt inventory item (f), to be deleted with the legacy branches.
"No legacy write is issued" is then **"no sink was entered"**, which is
independent of the gate.

**Invariants (spec §6.3, decision 8):** each site reads the transport state of
**its own** device before writing; on an `Owner` device no primary or unflip
legacy write is issued from any of them — no sink is entered — and on a
`Legacy` device the behaviour is identical to today, sinks included. Each
site's mutation forces its legacy branch, and the test that fails does so on
the recorder's count, never on a refusal error or on missing owner progress.

**Named tests:**
- `c0_conv_ciii_owner_device_issues_no_legacy_primary_write_vulkan` — one case
  per enumerated site, each asserting the recorder counted nothing.
- `c0_conv_ciii_sink_recorder_counts_entry_before_the_permit` — a `Legacy`
  device's sink and an `Owner` device's refused sink both record their entry,
  so the recorder is proven to observe entry and not authorization.
- `c0_conv_ciii_legacy_device_unchanged_vulkan`.

- [ ] Steps: enumerate and report the sites; the recorder; tests; red;
  implement; checks; stop dirty and report.

---

### Task 8: The hardware test of spec §6.4

**Files:** the test module that holds the owner-route `_drm` tests; tests only
— no production code unless an F8 stop says otherwise.

**Codex writes this test and does not run it** (decision 10): it takes DRM
master on card1 and must run from tty2. Codex's gate for this task is the
software one; the coordinator runs the test and its two mutations.

**Interfaces:** one `#[ignore]`d test, `c0_hw_ciii_owner_route_on_card1_drm` —
named outside the `c0_conv_ciii_` prefix so no filter the implementer may run
selects it (round-2 B-1) — chosen by `cargo test -p yserver --lib
c0_hw_ciii_owner_route_on_card1_drm -- --ignored --test-threads=1`. It establishes `Owner` on card1 with the
writer-coverage evidence of the debt spec §4.4 and drives, with the real
conductor, helper and producers:

1. a composed frame: `Accepted` → `HardwareComplete` → damage applied;
2. a direct frame from a Vulkan-rendered PRIME-imported buffer: out-fence
   `Success`, leases released at `PriorBufferReleased`;
3. **the P3-3 commit: a second direct commit presenting the same source
   buffer while the first is still current**, so the same allocation key
   appears in the old and the new state for the same member. Step 2 is the
   setup, step 3 is the commit under test, and they are asserted apart;
4. the unflip back to composed, through Tasks 3-5.

**Why that shape, and not a direct commit after the unflip (round-8 M-2).**
The unflip's retirement makes the composed resources current, so any direct
commit after it takes **composed** as its old state and shares no allocation
with its new state; retention is recognised only when the same allocation key
appears in old and new **for the same member**
(`resources/commit.rs:654`-`668`). The retaining commit must therefore follow
a direct commit, not a composed one.

**Invariants (spec §6.4, debt spec §9.3/§9.5):** each step's milestones arrive
in order and are asserted; the retained allocation of step 3 gets **no**
`KmsRelease` obligation and is not released, while any allocation the same
commit displaces **is** registered and released normally (**P3-3**); a
displaced buffer's `KmsRelease` is discharged by the real completion in
steps 1-2 and 4 (**P3-2**). If a second direct commit retaining its
predecessor's allocation for the same member is not reachable on card1, that
is an **F8 stop**, reported — not substituted by a fixture and not weakened
into a different commit shape.

**Reachability, rechecked by the plan's author on the implemented Ci/Cii code
(2026-09-20, required by spec §6.4 before P3-2/P3-3 are anchored):**
`register_kms` (`resources/mod.rs:657`) has one caller,
`register_commit_dependencies` (`resources/commit.rs:627`, registering exactly
the old-state allocations not retained by the new state for the same member,
`:668`), and that function has **three non-test callers**, all on the paths
this test drives: `admission.rs:1280` and `:1338` (primary dispatch) and
`admission.rs:1533` (direct dispatch). The condition of spec §3.2 holds.

**Named tests:**
- `c0_hw_ciii_owner_route_on_card1_drm`.

- [ ] Steps: write the test; software checks; the three `cargo check
  --workspace --target` gates; stop dirty and report. Do **not** run the
  `_drm` test.

---

## What the coordinator does

After each task: read the diff, re-run the checks above outside codex, apply
this plan's mutations **by line** against the recorded mutation text, confirm
each is caught by its named test, then commit. `_vulkan` tests and their
mutations run on the GPU with the user's go-ahead, from tty when the user asks
for it.

After Task 8, with the user's go-ahead and the GPU free: the full hardware gate
(`render_acceptance -- --ignored`, `c0_2ci -- --ignored`, the library's other
ignored tests — 306/306 at `5a34c6ec`, plus this plan's new `_vulkan` tests),
then, from tty2, `c0_hw_ciii_owner_route_on_card1_drm` and its two mutations
T32 and T33 under the same filter, then the acceptance finding and
`docs/status.md`. The acceptance records that stage 2c-iii still owes the
copied-route plan.
