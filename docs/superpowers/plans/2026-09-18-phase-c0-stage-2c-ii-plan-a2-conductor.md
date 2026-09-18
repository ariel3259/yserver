# Stage 2c-ii, plan A2 — the admission conductor

> **Implementer:** codex (model `gpt-5.6-luna`, reasoning effort `xhigh`), `--sandbox workspace-write`, run with `< /dev/null`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time. Tick steps (`- [ ]` → `- [x]`) only with the evidence each one names. Before writing code, read `AGENTS.md` and, as plain markdown, the Superpowers skills `executing-plans/SKILL.md` and `test-driven-development/SKILL.md` under `~/.claude/plugins/cache/claude-plugins-official/superpowers/*/skills/`. **The implementer never commits**: this worktree's git directory is read-only inside the sandbox. Stop with the tree dirty after each task; the coordinating session verifies and commits.

**Revision 2 (2026-09-18)** — incorporates codex round 1 (`../findings/2026-09-18-stage-2c-ii-plan-a2-review-round1.md`: 2 blocking, 3 major, all verified against the code and accepted).
- **B-1:** `begin` consumes the ledger and drops it on every pre-install refusal. Task 1 now adds an owner method that builds the ledger through a closure called only after the last refusal point, and returns the closure uncalled on refusal.
- **B-2:** direct offers and unflip requests are now two-sided transactions over the decider and the managed seam.
- **M-1:** current direct eligibility is an explicit snapshot input.
- **M-2:** retirement order is proven with an operation trace.
- **M-3:** refusals are tested with a non-empty current state.

Four mutations were added (N14–N17).

**Revision 3 (2026-09-18)** — incorporates codex round 2 (`../findings/2026-09-18-stage-2c-ii-plan-a2-review-round2.md`: 2 blocking, 1 major, all verified and accepted; round 1's B-2, M-1, M-2 and M-3 audited APPLIED, B-1 PARTIAL):
- **B-1:** a `Skip` deferred with no predecessor left in flight was stranded. It is now published at the end of the conductor operation that created it.
- **B-2:** a preparation failure after `lock` had no disposition, so the token could stay locked. Preparation is now transactional (reserve first, then move — the existing seam's `reserve(..)?` after `attach` drops the attached resources), and every post-`lock` exit aborts the token.
- **M-1:** the ledger builder now owns `take_current` and `composed_resources`, and is tested across every refusal point of `begin`.

Mutations N18–N20 were added.

**Goal:** Connect plan A1's decider to the real `DeviceCommitOwner` and to 2c-i's managed seams: one conductor per device that assembles the readiness snapshot, admits, dispatches through `lock` → `begin` → `send_on` → `confirm`/`abort`, disposes of refusals, orders retirement before publication, and withdraws a direct successor whose layout changed. Fixture-level only (R8).

**Architecture:** A new file `crates/yserver/src/kms/render/admission.rs` holds `AdmissionConductor` (the decider plus the conductor's own state) and an `impl KmsBackend` block with the conductor's operations, so they borrow `KmsBackend`'s fields disjointly instead of holding references into it. `KmsBackend` stores conductors by device; only tests install one. What 2c-ii cannot observe — the producer side, which 2c-iii converts — reaches the conductor through an injected `AdmissionSource`.

**Tech Stack:** Rust; the existing owner (`kms/owner/device.rs`), executor stubs (`kms/executor/test_support.rs`) and 2c-i managed seams in `kms/render/backend.rs`.

**Spec:** `docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md` revision 3, **section 7** (the conductor), with sections 4 (readiness), 6 (the token) and 10.2 (exit criteria). A1's decider is implemented in `crates/yserver/src/kms/owner/admission/` (`3dadb11b`..`459de718`): use **its actual API**, read it before Task 1.

## Design decisions this plan fixes (user-approved, 2026-09-18)

1. **An injected request source.** Nothing outside tests builds a `CommitDescription` today; that arrives with the producers in 2c-iii. The conductor asks an `AdmissionSource` for the description and for the producer-side readiness it cannot observe. The primary's resources reach the ledger **as an input, by value**: the conductor never builds an empty lease set on its own (spec §1, F13b-D1).
2. **The direct seam is split around `begin`, and `begin` gets a ledger builder.** `managed_dispatch_direct_successor(commit)` needs the `CommitId` that `begin` allocates, while `begin` needs the ledger. Worse, `begin` takes the ledger by value and drops it on every refusal before it installs the record (round-1 B-1: `begin_with_context` refuses on the legacy transport, identity exhaustion, a build or context error, or an occupied slot, and no `DispatchError` returns the ledger). Task 1 therefore adds `DeviceCommitOwner::begin_with_ledger`, which calls a ledger-building closure with the allocated `CommitId` **only after the last refusal point** and returns the closure uncalled on refusal. It also splits the seam into *prepare* (before `begin`), *undo* (when `begin` refuses) and the retirement keying after `begin`. The existing function stays as a composition of these, so the 2c-i tests keep their meaning.
3. **One device in 2c-ii.** The managed seams' state (`commit_consumer`, `scanout_m2`) is backend-global, not per device. The conductor is keyed per device in shape, but in 2c-ii it is exercised on the primary device only. This is a stated limit, not a refactor.
4. **Layout generation and eligibility.** No layout counter exists today. A2 adds one per conductor and an `admission_note_layout_change` entry point, the wake that withdraws a successor that lost eligibility; hooking real border/geometry change sites into it is 2c-iii's, when there is a production caller. **Current eligibility likewise** (round-1 M-1): spec §4 requires the successor to pass `scanout_direct_eligible` now, but that predicate's inputs are computed inline in `try_present_direct` (VT state, CRTC clock epoch, hardware cursor, root coverage), which a Vulkan-less fixture cannot satisfy. In 2c-ii eligibility therefore reaches the snapshot through `AdmissionSource::direct_eligible`, and an ineligible successor is invalidated like a layout change. Extracting `try_present_direct`'s computation into a real predicate and wiring it in is 2c-iii's.
5. **Helper-process tests.** The conductor's dispatch tests use stub executors, which spawn a helper process — the suite family known to flake. The gate runs the conductor tests five times.

## Limits this plan states rather than hides

- **A ready unflip cannot be dispatched in these fixtures.** Its readiness needs a retained composed framebuffer per output (`PlatformBackend::retained_composed_framebuffer`), which a Vulkan-less fixture cannot produce. The conductor-level unflip test proves the *waiting* disposition; the ready path is proven at decider level by A1. The real unflip producer, in 2c-iii, closes this.
- **Everything is fixture-level** (spec §10.1): the transport reaches `Owner` only in tests. No production path calls the conductor.
- **Where the in-flight direct frame lives.** 2c-i's `managed_dispatch_direct_successor` moves the *role* but leaves the frame in `scanout_m2.queued_successor`. On a confirmed direct dispatch the conductor moves the frame into `scanout_m2.pending`, as the legacy `submit_queued_direct_successor` does, so the existing retirement machinery publishes its completion.

## Global Constraints

- The conductor acts **only** when the device's transport gate (`PlatformBackend::transport_gate(device)`) is in `TransportState::Owner`. With no gate, or any other state, every conductor entry point is inert: it dispatches nothing and changes no decider state (R8; spec §7).
- **Confirmation is at the send boundary** (spec §6): `confirm` only after `send_on` returns `Ok` (the owner reports `Dispatched`); `abort` after a `begin` refusal or a pre-IPC `DispatchError::Refused`. Never confirm at `begin`.
- **At most one dispatch per wake**, and none while the owner's slot is occupied.
- **No retry on refusal or capacity pressure**: a refusal records its reason and returns; the next real wake re-evaluates (2c-i §6).
- **Retirement order** (spec §7): on `CompletionRetired`, first enqueue the predecessor's completion and the deferred `Skip`s onto `scanout_m2.completed`, then admit and dispatch, and **never** drain `completed` inside the handler — the core publishes after the handler returns.
- **A deferred `Skip` waits only behind an in-flight predecessor** (round-2 B-1). At the end of every conductor entry point (`admission_wake`, the retirement hook, `admission_note_layout_change`, `admission_request_unflip`), if no direct predecessor is in flight (`scanout_m2.pending` is `None`), `deferred_successor_skips` is appended to `completed`, after anything already enqueued. The core still publishes; nothing is drained inside the handler.
- **Every token is consumed exactly once** (spec §6): every exit after `lock` — refusal, preparation failure, unsupported tier, or success — ends in exactly one `confirm` or `abort`.
- **Resources travel by value** into the ledger and back through `CommitResourceConsumer::consume`; nothing is bare-dropped (a bare-dropped `RoleReservation` closes admission).
- Test names start with `c0_adm_conductor_`, so `cargo test c0_adm` covers A1 and A2 together and `c0_2ci` is untouched.
- `cargo test -p yserver --lib` needs the `yserver` binary built (helper tests). If helper tests fail with `HelperExited` or a device-lock timeout, run `cargo build -p yserver --bin yserver` and rerun before reporting.
- **Honesty rule (F8).** If a scenario below is unreachable in these fixtures, a seam does not behave as this plan says, or an interface cannot carry an invariant, **stop and report it**. A silently substituted test is the defect.

## Exit criteria covered by this plan

After Task 5 the coordinator applies each mutation to your code and runs the tests; a surviving mutation goes back to you as a finding.

| Spec criterion (A2's part) | Tests | Mutation that must fail them |
| --- | --- | --- |
| Inert unless the transport is `Owner` (§7, R8) | `c0_adm_conductor_is_inert_without_an_owner_transport` | N1: skip the `Owner` check |
| Snapshot: capacity, producer readiness and eligibility combined (§4) | `c0_adm_conductor_snapshot_waits_on_ordinary_retirement`, `c0_adm_conductor_snapshot_combines_producer_readiness`, `c0_adm_conductor_unflip_waits_without_a_composed_return` | N2: ignore `OrdinaryRetirement` occupancy; N3: report `Ready` without asking the source |
| Direct offer: descriptor and managed frame replaced together; the victim idles once with its `Skip` deferred (§3, C.0 §9.1) | `c0_adm_conductor_direct_offer_replaces_frame_and_descriptor_together` | N4: skip `set_direct_successor` on offer |
| The split seam: undo restores exactly (decision 2) | `c0_adm_conductor_seam_undo_restores_the_successor_charge`, existing `c0_2ci_backend_managed_dispatch_direct_successor_charges_submitted_then_retires` | N5: undo cancels the charge instead of moving it back to `Successor` |
| `begin_with_ledger` builds the ledger only past the last refusal point, and hands the builder back uncalled on refusal (round-1 B-1, round-2 M-1) | `c0_adm_conductor_begin_with_ledger_returns_the_builder_uncalled_on_refusal`, `c0_adm_conductor_composed_begin_refusal_keeps_resources_with_the_source` | N14: call the builder before the slot reservation; N20: fetch `composed_resources` before `begin_with_ledger` |
| A preparation failure after `lock` aborts the token and moves nothing (round-2 B-2) | `c0_adm_conductor_preparation_refusal_aborts_the_token` | N18: return without `abort` when preparation fails |
| A `Skip` created with no predecessor in flight is published, not stranded (round-2 B-1) | `c0_adm_conductor_retirement_wake_refusal_publishes_the_skip`, `c0_adm_conductor_retirement_wake_invalidation_publishes_the_skip`, `c0_adm_conductor_layout_change_with_nothing_in_flight_publishes_the_skip` | N19: skip the end-of-entry-point append |
| Direct offer and unflip request are two-sided transactions (round-1 B-2) | `c0_adm_conductor_direct_offer_is_refused_before_the_seam_while_unflip_is_pending`, `c0_adm_conductor_unflip_request_terminalizes_the_queued_frame` | N15: the unflip request drops the descriptor but leaves the frame and its `Successor` charge |
| An ineligible direct successor is never dispatched and is terminalized (round-1 M-1) | `c0_adm_conductor_ineligible_direct_successor_is_invalidated` | N16: the snapshot treats every successor as eligible |
| Dispatch confirms only at the send (§6) | `c0_adm_conductor_dispatch_confirms_after_send`, `c0_adm_conductor_pre_ipc_refusal_consumes_no_admission_state` | N6: confirm right after `begin` |
| Refusal disposition (§7) | `c0_adm_conductor_pre_ipc_refusal_consumes_no_admission_state`, `c0_adm_conductor_refusal_disposition_covers_every_pre_ipc_cause`, `c0_adm_conductor_begin_refusal_undoes_the_seam`, `c0_adm_conductor_send_refusal_restores_a_nonempty_current` | N7: drop the refusal's events instead of consuming them; N8: leave a refused direct successor queued; N17: ignore `ResourcesStillCurrent` |
| At most one dispatch per wake; none while the slot is occupied | `c0_adm_conductor_one_dispatch_per_wake` | N9: loop decide/dispatch until `None` |
| A mismatch at `lock` closes the transport before the owner holds anything (§7) | `c0_adm_conductor_lock_mismatch_closes_the_transport` | N10: ignore a `lock` error |
| Retirement order: predecessor, `Skip`s, admission, then publication (§7; stage 2c §4) | `c0_adm_conductor_retirement_enqueues_then_admits_before_publication` (asserts on the operation trace) | N11: admit before enqueueing; N12: drain `completed` inside the handler |
| A successor whose layout changed is never committed, including on a retirement wake (stage 2c v1.5.0 table) | `c0_adm_conductor_layout_change_withdraws_the_queued_successor`, `c0_adm_conductor_layout_change_blocks_retirement_promotion` | N13: `admission_note_layout_change` bumps the counter without withdrawing |

## File structure

| File | Responsibility | Tasks |
| --- | --- | --- |
| `crates/yserver/src/kms/render/backend.rs` | the split direct seam (prepare / bind / undo); a `admission_conductors` field; the `CompletionRetired` hook in `route_owner_event` | 1, 2, 4 |
| `crates/yserver/src/kms/render/admission.rs` (new) | `AdmissionSource`, `AdmissionConductor`, the conductor's `impl KmsBackend` block | 2–5 |
| `crates/yserver/src/kms/render/mod.rs` | registers `admission` | 2 |
| tests | in `admission.rs` (`#[cfg(test)] mod tests`) or beside the existing `c0_2ci_backend_managed_*` tests — your call, report it | 1–5 |

## Gate (every task)

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
for i in 1 2 3 4 5; do cargo test -p yserver --lib c0_adm; done
cargo test -p yserver --lib c0_2ci
cargo test -p yserver --lib
```

Report all five `c0_adm` runs. Task 5 additionally runs clippy with `--features tcp-transport` and `--features xdmcp`, and `cargo check --workspace --target` for `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl` and `x86_64-unknown-freebsd`.

---

### Task 1: `begin_with_ledger`, and the direct dispatch seam split around it

**Files:** `crates/yserver/src/kms/owner/device.rs` (one new method), `backend.rs` (the managed seams near `managed_dispatch_direct_successor`), tests.

**Owner interface (public, like `begin`):**

```rust
impl<R> DeviceCommitOwner<R> {
    /// `begin`, with the ledger built by `ledger(commit)` only after every
    /// refusal point (transport, identity, build, completion context, slot).
    /// On refusal the builder comes back **uncalled**, with the error.
    pub fn begin_with_ledger<F>(&mut self, desc: &CommitDescription, ledger: F)
        -> Result<(CommitId, Vec<OwnerEvent<R>>), (DispatchError<R>, F)>
    where
        F: FnOnce(CommitId) -> Submitted<R>;
}
```

It must refuse in exactly the cases `begin` refuses, with the same context `begin` builds; `begin` itself is unchanged.

**Seam interfaces (crate-private):**

```rust
/// What `prepare` hands to the conductor: the successor's resources, now in
/// `Submitted`, and the pre-reserved retirement role if it displaces a Current.
pub(crate) struct PreparedDirectDispatch {
    pub(crate) resources: CommitResources,           // carries the Submitted role
    pub(crate) retirement: Option<RoleReservation>,  // OrdinaryRetirement, when displacing
}

impl KmsBackend {
    /// Before `begin`: everything `managed_dispatch_direct_successor` did except
    /// keying the retirement by a commit. `Ok(None)` exactly where the old seam
    /// returned `Ok(None)`, with the same side effects (including
    /// `direct_admission_scheduled` when OrdinaryRetirement is occupied).
    pub(crate) fn managed_prepare_direct_dispatch(&mut self)
        -> Result<Option<PreparedDirectDispatch>, ResourceError>;
    /// After `begin` returned `commit`: stamp the resources with it and key the
    /// retirement reservation (`prereserve_retirement`). Returns the resources.
    pub(crate) fn managed_bind_direct_dispatch(&mut self, prepared: PreparedDirectDispatch,
        commit: CommitId) -> CommitResources;
    // In the conductor, "bind" is split in two: the builder closure stamps the
    // resources with `commit` (`with_commit_id`) and builds the ledger, and the
    // retirement reservation is keyed by `commit` (`prereserve_retirement`)
    // after `begin_with_ledger` returns Ok. `managed_bind_direct_dispatch` is
    // the composition, for the existing seam and its tests.
    /// `begin` refused: move the role back to `Successor`, restore
    /// `queued_successor_role`, and discharge the retirement reservation.
    pub(crate) fn managed_undo_direct_dispatch(&mut self, prepared: PreparedDirectDispatch)
        -> Result<(), ResourceError>;
}
```

`managed_dispatch_direct_successor(commit)` stays, implemented as prepare + bind, with unchanged behaviour.

**Invariants:**

0. **Prepare is transactional** (round-2 B-2). When displacing, it reserves `OrdinaryRetirement` **before** moving the successor's role and attaching. On any error it restores every step already taken — `queued_successor_role` back in `Successor`, the reservation cancelled — and returns `Err` with nothing moved and nothing dropped. The existing seam reserves after `attach` with `?`, which drops the attached resources; the split version must not inherit that. Since the existing `managed_dispatch_direct_successor` becomes prepare + bind, it gets the fixed order too. This fixes a latent 2c-i defect, which is hard to reach because the seam checks `is_vacant(OrdinaryRetirement)` first; name it in your report.
1. prepare + bind is observably identical to the old `managed_dispatch_direct_successor` (same roles, same occupancy, same `reserved_retirements` entry).
2. prepare + undo leaves `DirectCapacity` exactly as before prepare: the same roles occupied, the successor charged as `Successor` again, no `OrdinaryRetirement` reservation, and admission **not** closed.
3. Nothing is bare-dropped on any path.

**Named tests:**

- `c0_adm_conductor_seam_undo_restores_the_successor_charge` — with a current direct frame (so prepare pre-reserves retirement) and a prepared successor: record `occupied()` and the successor's role before prepare; prepare, then undo; `occupied()` equals the recorded value, `queued_successor_role` is `Some` with role `Successor`, `OrdinaryRetirement` is vacant, `is_admission_closed()` is false.
- `c0_adm_conductor_seam_prepare_then_bind_matches_the_old_seam` — the same fixture twice: once through `managed_dispatch_direct_successor(commit)`, once through prepare + bind with the same `commit`; roles, occupancy and `reserved_retirements` keys agree.
- The existing `c0_2ci_backend_managed_dispatch_direct_successor_charges_submitted_then_retires` must still pass unmodified.
- `c0_adm_conductor_begin_with_ledger_returns_the_builder_uncalled_on_refusal` — an owner-level test (in `device.rs`'s test module, where the private fields are reachable) with **one case per refusal point of `begin_with_context`, in its order**: the legacy transport (`new_legacy` owner), identity exhaustion (force `next_seq` to `u64::MAX`, as `sequence_exhaustion_refuses_without_reserving` does), a description `build_atomic_request_with_modeset` rejects, a completion context `begin` rejects (e.g. `page_flip_event = true`), and an occupied slot. The builder is a closure that records whether it ran **and owns a non-empty value whose drop is observable**. In each case `begin_with_ledger` returns `Err` with the builder, the builder never ran, and the owned value is intact inside it. With a free slot and a valid description the builder runs exactly once, with the returned `CommitId`.

- [ ] **Step 1:** Write the two tests. **Step 2:** run and record the failure. **Step 3:** split the seam. **Step 4:** gate. **Step 5:** stop dirty and report.

---

### Task 2: The conductor, its activation, its intents and the snapshot

**Files:** `admission.rs` (new), `kms/render/mod.rs`, `backend.rs` (a field and its initialisers), tests.

**Interfaces:**

```rust
/// What 2c-ii cannot observe because producers are converted in 2c-iii.
pub(crate) trait AdmissionSource {
    /// Producer-side readiness: for a composed intent, a reusable buffer and
    /// finished producer waits; for a direct one, its pre-submit source waits.
    fn producer_readiness(&self, key: IntentKey) -> Readiness;
    /// The atomic description of an admitted primary.
    fn describe(&mut self, admitted: &Admitted) -> CommitDescription;
    /// A composed admission's new-state resources, moved into the ledger.
    fn composed_resources(&mut self, crtc: CrtcId, generation: u64) -> Vec<CommitResources>;
    /// Whether the queued direct successor passes current direct eligibility
    /// (spec §4). 2c-iii replaces this with the real predicate (decision 4).
    fn direct_eligible(&self, source_generation: u64) -> bool;
}

pub(crate) struct AdmissionConductor {
    /* the A1 `Admission`, a `Box<dyn AdmissionSource>`, the layout generation,
       the next direct source generation, and whatever Tasks 3–5 need */
}

impl KmsBackend {
    // Only tests install a conductor (R8).
    pub(crate) fn install_admission_conductor_for_tests(&mut self, device: DrmDeviceKey,
        source: Box<dyn AdmissionSource>);
    pub(crate) fn admission_offer_composed(&mut self, device: DrmDeviceKey, crtc: CrtcId,
        generation: u64) -> Result<(), AdmissionError>;
    /// `managed_prepare_direct_candidate` plus `set_direct_successor`, kept in step.
    pub(crate) fn admission_offer_direct(&mut self, device: DrmDeviceKey, source_id: DrawableId,
        candidate: PresentScanoutCandidate, event: CompletedPresentEvent)
        -> Result<bool, ResourceError>;
    pub(crate) fn admission_request_unflip(&mut self, device: DrmDeviceKey,
        crtcs: BTreeSet<CrtcId>) -> Result<(), AdmissionError>;
    pub(crate) fn admission_snapshot(&self, device: DrmDeviceKey, retirement_wake: bool)
        -> Option<ReadinessSnapshot>;   // None when no conductor, or not Owner
}
```

`KmsBackend` gains `admission_conductors: BTreeMap<DrmDeviceKey, AdmissionConductor>`, empty in every constructor.

**Invariants:**

1. **Activation.** Every entry point above is inert unless a conductor is installed **and** `platform.transport_gate(device)` is `Owner`. Inert means no decider change, no seam call, and `admission_snapshot` returns `None`.
2. **A direct offer is a two-sided transaction** (round-1 B-2). While the decider has an unflip barrier, the offer is refused **before** the managed seam is touched: nothing charged, nothing queued. Otherwise it keeps both sides in step: The descriptor's `source_generation` comes from a conductor-owned monotonic counter, `layout_generation` from the conductor's layout counter, `topology_generation` from the owner's `topology_generation()`, and `crtcs` the CRTCs of **every** output the frame targets — a direct frame scans out on all of the device's outputs (`awaiting_outputs = 0..outputs.len()` in the existing seam), which is the grouped unit's output set. When the managed seam queues the frame, the decider gets the descriptor; when the seam replaces a victim (it defers the victim's `Skip` and idles it once), the decider's displaced descriptor is that victim's. If the seam refuses (`Ok(false)`), the decider is unchanged. If the decider nevertheless refuses after the seam queued the frame, the frame goes back out through the never-submitted path (discharge its `Successor` charge, defer its `Skip`), so no frame or charge outlives its descriptor.
3. **An unflip request is two-sided too.** When `request_unflip` displaces a descriptor, the conductor terminalizes **that exact** queued frame (matched by its source generation) through the never-submitted path: discharge `queued_successor_role` (never a bare drop) and `defer_direct_successor_skip`. This is step 1 of the existing `managed_handle_direct_unflip`; factor it out rather than duplicating it, and leave that seam's other steps (the exit retirement and shadow) to the unflip's own dispatch, which is outside A2.
4. **Snapshot** (spec §4):
   - Composed: the source's `producer_readiness`.
   - Direct: `Ready` only if the source reports `Ready` **and**, when a Current exists to displace, `OrdinaryRetirement` is vacant — otherwise `Waiting(OrdinaryRetirementOccupied)`. Eligibility has two parts: the decider's layout/topology comparison (the snapshot carries the conductor's layout generation and the owner's topology generation), and `direct_eligible`. A successor for which `direct_eligible` is false is **invalidated** by the same path as a layout change (Task 5's invariant 2) the first time a snapshot sees it, and is never reported ready.
   - Unflip: `Ready` only if `ExitRetirement` is vacant **and** `retained_composed_framebuffer` exists for every output; otherwise `Waiting(ExitRetirementOccupied)` or `Waiting(ComposedReturnNotEstablished)`.
   - `retirement_wake` as passed.

**Named tests** (a helper that installs a gate in `Owner` — built from the existing gate tests' `issue_handover_permit` + `publish_owner` path — is yours to write):

- `c0_adm_conductor_is_inert_without_an_owner_transport` — with a conductor installed: no gate, then a gate in `Legacy`, then in `Quiescing`: offers and `admission_snapshot` do nothing (`None`, decider empty, no seam charge). In `Owner`, the same offer takes effect.
- `c0_adm_conductor_snapshot_waits_on_ordinary_retirement` — Owner; a current direct frame occupying `Current` and `OrdinaryRetirement` occupied; a direct offer the source reports `Ready`: the snapshot reports it `Waiting(OrdinaryRetirementOccupied)`.
- `c0_adm_conductor_snapshot_combines_producer_readiness` — a composed intent the source reports `Waiting(NoReusableBuffer)` is reported so; flipping the source to `Ready` changes the snapshot.
- `c0_adm_conductor_unflip_waits_without_a_composed_return` — Owner; an unflip requested; the fixture has no retained composed framebuffer: `Waiting(ComposedReturnNotEstablished)`.
- `c0_adm_conductor_direct_offer_is_refused_before_the_seam_while_unflip_is_pending` — request an unflip, then offer a direct candidate: refused; `capacity.occupied()` unchanged, `scanout_m2.queued_successor` empty, decider direct slot empty.
- `c0_adm_conductor_unflip_request_terminalizes_the_queued_frame` — offer a direct candidate, then request an unflip: the decider's direct slot is empty **and** `scanout_m2.queued_successor` and `queued_successor_role` are empty, the frame's event is idled once and its `Skip` deferred once, `occupied()` dropped by one, admission not closed.
- `c0_adm_conductor_ineligible_direct_successor_is_invalidated` — a source whose `direct_eligible` is false for the offered successor, `producer_readiness` `Ready`: the first snapshot does not report it ready, the successor is withdrawn and terminalized (as above), and a wake dispatches nothing.
- `c0_adm_conductor_direct_offer_replaces_frame_and_descriptor_together` — two successive direct offers: `scanout_m2.queued_successor` holds the second frame, the decider's queued descriptor is the second, the first frame's event appears **once** in `scanout_m2.idled` and **once** in `scanout_m2.deferred_successor_skips` with `COMPLETE_MODE_SKIP`, and `capacity.occupied()` did not grow.

- [ ] **Step 1:** Write the tests. **Step 2:** run and record the failure. **Step 3:** implement. **Step 4:** gate. **Step 5:** stop dirty and report.

---

### Task 3: Dispatch — `lock`, `begin`, `send_on`, `confirm`/`abort`, and refusals

**Files:** `admission.rs`, tests.

**Interfaces:**

```rust
impl KmsBackend {
    /// One admission wake (spec §7 step 2): if Owner and the owner's slot is
    /// free, snapshot → decide → lock → build the request → begin → send_on →
    /// confirm or abort. Returns what happened, for tests and telemetry.
    pub(crate) fn admission_wake(&mut self, device: DrmDeviceKey, retirement_wake: bool)
        -> AdmissionOutcome;
}

pub(crate) enum AdmissionOutcome {
    Inert,                                  // no conductor, or not Owner
    SlotBusy,
    NothingAdmissible,
    Dispatched(Confirmed),
    BeginRefused(/* the DispatchError or its kind */),
    SendRefused(RefusalCause),
    TransportClosed,                        // lock mismatch
    PreparationRefused,                     // direct prepare returned Err or Ok(None) after lock
    Unsupported(Tier),                      // topology/unflip admitted; aborted, left queued
}
```

Building the request:

- **Composed:** `describe` + `composed_resources`; the ledger is `Submitted::new(old, new)` with `old` = `commit_consumer.take_current()` and `new` = the source's resources.
- **Direct:** `managed_prepare_direct_dispatch`, then `begin_with_ledger` with a builder that takes the current state (`commit_consumer.take_current()`), stamps the prepared resources with the `CommitId`, and returns `Submitted::new(old, vec![resources])`. After `Ok`, key the retirement reservation by the `CommitId` (`prereserve_retirement`). On `Err((error, builder))` the builder is uncalled: nothing was taken, so undo the prepared seam and abort.
- **Composed** uses `begin_with_ledger` the same way: **both** `commit_consumer.take_current()` and `source.composed_resources(..)` are called **inside** the builder (round-2 M-1), so a refused `begin` never takes the current state and never takes resources from the source.
- **Direct preparation after `lock`:** if `managed_prepare_direct_dispatch` returns `Err` or `Ok(None)`, `abort` the token and return `PreparationRefused`. Transactional prepare means nothing was moved. The successor stays queued, and there is no retry.
- **Topology and unflip admissions:** out of A2's dispatch — if the decider admits one, return it **unconsumed** (`abort`) with an outcome that says so. A1 proves their order; their commits are lifecycle/unflip work outside this plan. Report if this is not reachable as stated.

**Invariants:**

1. **Confirm only at the send.** `send_on` → `Ok` ⇒ `confirm`. On a direct confirm, move the frame from `scanout_m2.queued_successor` into `scanout_m2.pending` and discharge nothing else. (`SendError::Ipc` already returns `Ok` from `send_on`; there is nothing extra to do for it.)
2. **`begin` refused** ⇒ `abort`; for a direct admission `managed_undo_direct_dispatch`. The builder never ran, so the current state was never taken. Nothing else changes.
3. **`send_on` refused before IPC** (`DispatchError::Refused { cause, events }`) ⇒ `abort`; pass **every** event in `events` to `commit_consumer.consume` (they carry the ledger back: `ResourcesReleased` and `ResourcesStillCurrent`); for a direct admission, `withdraw_direct` the descriptor and send its frame through `defer_direct_successor_skip` (idle once, `Skip` deferred). Composed and maintenance desired state stay in the decider. The six causes are handled identically.
4. **Mismatch at `lock`** ⇒ close the device's transport (the gate's `force_close`, or the existing close path — say which) and return `TransportClosed`. The owner holds nothing at that point.
5. **One dispatch per wake**; `SlotBusy` without calling `decide` when the owner's slot is occupied.

**Named tests** (these need a backend with stub executors — `backend_with_stub_executors_with_behaviour_for_tests`, `NeverReply` for a send that succeeds and `reaped_executor_for_tests`-style reaping for `Reaped`; the transport in `Owner`):

- `c0_adm_conductor_dispatch_confirms_after_send` — a ready composed intent; one wake: `Dispatched`, the owner's live record is dispatched, the decider's `sequence()` is 1 and the composed slot is empty. A second ready direct offer in a separate fixture: `Dispatched`, and the frame is now in `scanout_m2.pending`, its role `Submitted`.
- `c0_adm_conductor_pre_ipc_refusal_consumes_no_admission_state` — a ready direct offer and a reaped executor: `SendRefused(Reaped)`; decider `sequence()` still 0 and not locked; the direct descriptor withdrawn; its frame's event in `idled` and `deferred_successor_skips` once each; `capacity.is_admission_closed()` false; the owner's slot free.
- `c0_adm_conductor_refusal_disposition_covers_every_pre_ipc_cause` — the conductor's refusal handling, driven with a constructed `DispatchError::Refused { cause, events }` for each of `Reaped`, `Stalled`, `AlreadyInFlight`, `ReservationMismatch`, `BoundaryViolation` and `TransportGateRefused`: each leaves the same state as the `Reaped` case. (Factor the handling so it can be driven this way; say how.)
- `c0_adm_conductor_begin_refusal_undoes_the_seam` — a source whose `describe` returns a description `begin` rejects (e.g. `page_flip_event = true`, which `begin` refuses with `InvalidCompletionContext`): `BeginRefused`, the decider unchanged, the successor charge back in `Successor`, `current_resources` restored.
- `c0_adm_conductor_send_refusal_restores_a_nonempty_current` — round-1 M-3: with direct A **current** (its `CommitResources` holding the `Current` role) and a ready successor B, a reaped executor: after the refusal, `current_resources` holds A again with role `Current`, B's resources went to `rejected_resources` still holding their role (2c-i's `consume` of `ResourcesReleased` records them there; nothing is bare-dropped), the pre-reserved `OrdinaryRetirement` was cancelled, and admission is not closed. Repeat with a composed admission over a non-empty current. If both shapes go through one event-consumption function, say so and show it.
- `c0_adm_conductor_preparation_refusal_aborts_the_token` — a queued, ready direct successor; close `commit_consumer.capacity` admission (so prepare fails) after the snapshot is taken but before preparation (use a `#[cfg(test)]` hook between `lock` and prepare), then wake: `PreparationRefused`, the decider **not locked** afterwards, `sequence()` unchanged, the successor charge still `Successor`, nothing in `rejected_resources`. A second case makes prepare return `Ok(None)` the same way (occupy `OrdinaryRetirement` through the hook) with the same assertions.
- `c0_adm_conductor_composed_begin_refusal_keeps_resources_with_the_source` — a composed admission whose source hands out non-empty resources (with an observable drop) and a description `begin` rejects: `BeginRefused`, the source's `composed_resources` was **never called**, `current_resources` untouched, nothing dropped.
- `c0_adm_conductor_one_dispatch_per_wake` — two ready composed intents on different CRTCs: one wake dispatches one; a second wake returns `SlotBusy` while the first is in flight.
- `c0_adm_conductor_lock_mismatch_closes_the_transport` — with a `#[cfg(test)]` hook that changes a generation between `decide` and `lock` (yours to add; the mismatch is otherwise unreachable on one thread): `TransportClosed`, the gate no longer `Owner`, the owner's slot free.

- [ ] **Step 1:** Write the tests. **Step 2:** run and record the failure. **Step 3:** implement. **Step 4:** gate (five `c0_adm` runs). **Step 5:** stop dirty and report.

---

### Task 4: Retirement ordering

**Files:** `backend.rs` (`route_owner_event`'s `CompletionRetired` handling), `admission.rs`, tests.

**Invariant (spec §7, stage 2c §4):** when `route_owner_event` sees `OwnerEvent::CompletionRetired` for a device with an active conductor, in this order:

1. `commit_consumer.consume` the event (as today);
2. enqueue the predecessor's completion — move `scanout_m2.pending`'s event into `scanout_m2.completed` with the frame's pins released, as the legacy retirement does — and then append `scanout_m2.deferred_successor_skips`;
3. `admission_wake(device, true)`;
4. return **without** draining `scanout_m2.completed`: the core publishes through `drain_completed_present_events` afterwards.

A device without an active conductor behaves exactly as today.

**Named test:**

- `c0_adm_conductor_retirement_enqueues_then_admits_before_publication` — Owner; direct A dispatched and in flight; offer B, then C (C displaces B, so B's `Skip` is deferred); feed `CompletionRetired` for A through `route_owner_event`. Then, **before** draining: C is dispatched (the owner's live record is C's commit, dispatched). Then `drain_completed_present_events`: A's completion first, B's `Skip` after it, and nothing of C.

**Prove the order with an operation trace** (round-1 M-2): observing the result afterwards cannot tell "enqueue, then admit" from "admit, then enqueue". Give the conductor a `#[cfg(test)]` trace, a `Vec` of steps appended as they happen — at least `Consumed(commit)`, `Enqueued { completions, skips }`, `Decided`, `Dispatched(commit)` — and have the handler record into it. The test asserts the exact sequence `Consumed(A) → Enqueued → Decided → Dispatched(C)`, the `Enqueued` entry naming A's completion and B's `Skip`. It then asserts that `drain_completed_present_events` returns A's completion before B's `Skip` and nothing of C, and that nothing was drained inside the handler. N11 must break the trace assertion and N12 the drain assertion.

- `c0_adm_conductor_retirement_wake_refusal_publishes_the_skip` — direct A in flight, successor B queued and ready; the executor reaped **before** A's `CompletionRetired` is routed: the retirement wake tries B and gets `SendRefused(Reaped)`. Before the handler returns, `completed` holds A's completion then B's `Skip`; `deferred_successor_skips` is empty.
- `c0_adm_conductor_retirement_wake_invalidation_publishes_the_skip` — the same, but B turns ineligible (`direct_eligible` false) before the retirement: the wake invalidates B; `completed` holds A's completion then B's `Skip`.

- [ ] **Step 1:** Write the tests. **Step 2:** run and record the failure. **Step 3:** implement. **Step 4:** gate. **Step 5:** stop dirty and report.

---

### Task 5: Layout invalidation

**Files:** `admission.rs`, tests.

**Interface:**

```rust
impl KmsBackend {
    /// A geometry, layout or border change that may affect a queued direct
    /// successor: bump the layout generation, withdraw a successor queued under
    /// the old one and terminalize it through 2c-i's never-submitted path, then
    /// wake admission. Inert unless Owner.
    pub(crate) fn admission_note_layout_change(&mut self, device: DrmDeviceKey)
        -> AdmissionOutcome;
}
```

**Invariants:**

1. After a layout change, a successor queued under the previous layout generation is **never committed**, by any tier or wake — including a retirement wake that would otherwise promote it.
2. Its termination is 2c-i's never-submitted path: the `Successor` charge discharged (never bare-dropped), the frame idled once with its `Skip` deferred behind the predecessor.
3. The wake runs afterwards, so another ready primary can take the slot.

**Named tests:**

- `c0_adm_conductor_layout_change_withdraws_the_queued_successor` — Owner; a queued, ready direct successor; `admission_note_layout_change`: the decider's direct slot is empty, the frame's event is idled once and its `Skip` deferred once, `occupied()` dropped by one, admission not closed, and no dispatch of that successor happened.
- `c0_adm_conductor_layout_change_with_nothing_in_flight_publishes_the_skip` — no direct predecessor in flight, successor B queued; `admission_note_layout_change`: B's `Skip` is in `completed` when the call returns, not left in `deferred_successor_skips`.
- `c0_adm_conductor_layout_change_blocks_retirement_promotion` — direct A in flight, successor B queued, a layout change, then `CompletionRetired` for A: B is not dispatched on the retirement wake, and its `Skip` is published after A's completion.

- [ ] **Step 1:** Write the tests. **Step 2:** run and record the failure. **Step 3:** implement. **Step 4:** the full gate, including Task 5's additions. **Step 5:** stop dirty and report.

---

## What the coordinator does after each task

1. Reads the diff against the task's interfaces and invariants, and checks that every named test sets up its stated scenario.
2. Reruns the task's gate, including the five `c0_adm` runs.
3. After Task 5: applies N1–N20 to **your** code, one at a time, confirms each run compiled, and records which tests fail. A survivor goes back as a finding naming the invariant and the mutation.
4. Commits each task with `Implemented-By: codex (model gpt-5.6-luna, reasoning effort xhigh)` and `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
