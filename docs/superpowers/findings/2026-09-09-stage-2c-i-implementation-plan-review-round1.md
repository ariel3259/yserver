## Verdict

**1 blocking, 2 major, 1 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `13637318`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.

Reviewed baseline: `14dd92d818e619357c903ad64e5357f47619111e` plus local documents.
Reviewed plan SHA256: `1473cd880c6e3e872e644697a9a385de844bbde60c1b8694dd387a2dffcd7d6f`.
The authorized first plan-review pass completed successfully before the new
upstream inspection on 2026-09-10. The provenance above is copied from its
completed dispatcher output. Token usage was not captured before the temporary
log became unavailable after the environment transition; no usage is estimated.
The former log path was `/tmp/yserver-stage2ci-plan-review-round1.log`.


This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status |
|---|---|
| None | No prior review of this executable plan; check 1 was skipped as directed. The design round-3 traceability claims were treated as background, not prior plan findings. |

## Findings

### Blocking

#### B-1 — GPU proof application is non-transactional and can discard the retaining batch on validation failure

The plan promises that malformed or foreign proof “changes no allocation” and closes the affected converted route ([plan lines 85–89](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md#L85)). It also requires uncertain batches to remain retained ([plan lines 318–319](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md#L318)).

However, Task 5’s prescribed completion loop applies obligations sequentially with `?` ([plan lines 333–351](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md#L333)). If a signalled multi-allocation ticket contains valid obligation A followed by stale/invalid obligation B:

1. A is discharged and may become reusable.
2. B returns `InvalidProof`.
3. `poll_gpu` exits before installing the next pending-batch vector.
4. The current `CoreRetirementBatch`, including descriptor/command-slot ownership and remaining leases, can leave the authoritative pending collection without a defined quarantine transfer.

The error branch has the same problem if freezing any entry fails before `pending.push(obligation)`. This contradicts the authoritative requirement that evidence be generation-correlated, stale evidence not release newer state, and unknown work retain its reservation ([resource design lines 164–183](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L164)). The governing lifecycle also requires uncertainty ownership to survive IPC/completion failures ([governing spec lines 2127–2132](../specs/2026-08-26-phase-c0-atomic-kms-migration-design.md#L2127)).

Smallest correction: require a two-phase operation that validates the complete ticket-to-obligation set without mutation, then applies it atomically. Any validation/application failure must re-root the intact batch, freeze every still-relevant entry best-effort without early loss, and close the route. Add a regression with a valid first entry and invalid/stale later entry.

### Major

#### M-1 — Capacity has no authoritative completion path from the resource consumer

Task 7 fixes `CommitResourceConsumer::consume` to receive only the event and `ResourceService` ([plan lines 426–439](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md#L426)). Task 8 then embeds a `RoleReservation` in `CommitResources`, but `DirectCapacity` remains a separate owner and only it can `move_role`, `cancel_reservation`, or `finish_role` ([plan lines 480–491](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md#L480)). No cross-task contract delivers a terminalized token back to that capacity instance.

Consequently, when the consumer proves a rejected candidate, old retirement, or exit retirement releasable, it can drop/move the resources but cannot authoritatively vacate or transition the corresponding role. Implementations are pushed toward either leaking the charge indefinitely or freeing it through token destruction, which the plan expressly forbids. The handoff bundle retaining both objects does not solve live-path event delivery.

The spec requires role positions to remain charged until actual dependencies finish and to wake admission upon release ([resource design lines 390–399](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L390)).

Smallest correction: make capacity part of the consuming authority, or define an explicit by-value disposition returned from `consume` that the same serialized orchestration step must apply to the identified `DirectCapacity`. Specify rejection, completion, unknown, ordinary-retirement, and exit-retirement transitions, including the availability wake.

#### M-2 — Writer-gate tests verify the enum, not enforcement at mutating entry points

The governing M-2 contract requires every mutating transport entry to check the per-incarnation authority; checking only a central primary path is insufficient ([conversion design lines 248–270](../specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md#L248)). The normative inventory likewise requires caller inspection before an adapter is complete ([inventory lines 85–91](../specs/2026-09-09-phase-c0-stage-2c-i-resource-adapter-inventory.md#L85)).

Task 6 tests only that `allows_legacy(class)` returns false for enum values ([plan lines 391–401](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md#L391)). Task 10’s caller-audit searches cover resource cleanup and handover symbols, but do not enumerate or exercise the concrete modeset, DPMS, VT, topology, cursor, gamma, and helper mutation entry points ([plan lines 585–594](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md#L585)).

Thus all gate unit tests can pass while a real mutating path never consults the gate. Production Owner remains disabled now, but the delivered activation interface would not establish its claimed exclusion contract.

Smallest correction: add a concrete writer-entry inventory and tests that invoke each actual mutation boundary under `Quiescing`, test-only `Owner`, and `Closed`, asserting no transport/helper dispatch occurs. Require the implementation audit to classify every discovered mutation entry.

### Minor

#### m-1 — Group membership uses raw CRTC numbers despite the stable-generation identity requirement

`CommitResources.crtcs` is specified as `BTreeSet<u32>` ([plan lines 412–419](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md#L412)). Although the plan preserves `crtc_epoch` inside Present event metadata, it does not state that release-set membership itself carries a stable CRTC/topology generation.

The authoritative design requires output work to use stable device/CRTC and generation identities because output-vector or reused numeric identity is insufficient ([resource design lines 289–312](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L289)). After same-incarnation topology replacement, a raw CRTC number can otherwise be resolved against the new topology while retiring an old grouped record.

Smallest correction: type group membership with the existing stable `CrtcKey` plus topology/CRTC generation, or explicitly make the commit-correlated immutable membership record carry and validate that generation without consulting current topology.

## Coverage and implementation checks

- Incorporation: skipped because there is no prior plan review.
- Architecture/contracts: reviewed all ten tasks, dependency order, resource/event producers and consumers, capacity ownership, activation exclusion, and handoff.
- Safety/failure semantics: reviewed proof ordering, retention, cleanup rights, alias/fd-family barriers, unknown outcomes, deadlines, and by-value handoff.
- Compliance/verification: compared the plan with the authoritative resource design, normative inventory, governing §§9.1, 10–10.4 and 12, and the M-2 gate.
- Excerpts used: **12/12** bounded excerpts. No source excerpt was needed to decide the reported contract defects.
- Unassessed rather than presumed sound: exhaustive existing call sites, private backing-field details, dependency signatures, and hardware-specific cleanup behavior.
- Properly deferred to implementation: exact Rust APIs, compilation, focused tests, clippy, formatting, glibc/musl/FreeBSD builds, software Vulkan execution, and DRM hardware validation.

## Author verification status — 2026-09-10

The original verdict is preserved. The user reported another upstream update
immediately after the pass; baseline comparison is underway. These findings
remain open until individually verified and corrected. The plan is not ready
for execution, and this review does not cover the new upstream code.
Relative citation paths were repaired when filing; finding text was unchanged.


## Local dispositions after verification — 2026-09-10

- **B-1 verified and locally corrected.** The prescribed loop could mutate A
  and early-return on B before restoring pending ownership. Task 5 now validates
  the full batch first, commits without fallible/reentrant work, and roots the
  intact batch before best-effort quarantine on error. Added the valid-first /
  stale-later and freeze-failure regression requirements.
- **M-1 verified and locally corrected.** `consume` had no capacity access.
  Task 8 now puts the sole capacity table inside the consumer, specifies
  `on_available` routing and role-token completion, and moves it with that
  consumer during handoff. Reserved-destination transfer is explicit.
- **M-2 verified and locally corrected.** Enum tests did not invoke writers.
  Source inspection confirms page-flip/modeset, cursor, gamma and helper
  mutation paths. Task 6 now inventories concrete entry points and requires
  counting-transport tests beneath each real boundary; Task 10 searches all sinks.
- **m-1 verified and locally corrected.** Raw CRTC membership omitted generation.
  `GroupMember` now captures stable CRTC, topology generation and CRTC epoch;
  matching cannot consult replacement topology for an old record.

These corrections and new upstream DRI3/overlay requirements have not received
another external pass. The original counts/coverage above remain unchanged.
No 2c-i code has been implemented. The pending integration validation concerns
upstream merge correctness, not proof that these planned adapters work.
