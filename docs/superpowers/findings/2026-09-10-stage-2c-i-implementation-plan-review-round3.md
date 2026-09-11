## Verdict

**1 blocking, 2 major, 2 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE** (reading budget 11/12; one hypothesis left at "design question", stated below).

**Reviewer:** `claude --print`, read-only tool set, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `claude-opus-5`; reasoning effort `medium`; `claude 2.1.268 (Claude Code)`.
Dispatched by `review-claude.sh` because codex was unavailable.
**Not comparable to any codex round**, including reviews citing this same SHA:
the reviewer is part of the instrument. This reviewer also inherits the repo's
CLAUDE.md/AGENTS.md and the user's global instructions, which codex does not.

Reviewed baseline: `783089b4` plus the uncommitted local plan, design and
findings documents. Prior: the round-2 plan review.

Design-review result only; no claim that code compiles, tests pass or implementation is approved.

## Incorporation audit

| Round-2 finding | Status | Assessment |
|---|---|---|
| B-1 — teardown release omitted KMS disposition | **APPLIED** | Task 9 adds `KmsDisposition {Outstanding, Discharged, Superseded(DeviceBarrier)}`, two private barrier constructors scoped to the device key, `InvalidProof` while `Outstanding` (plan 607–622), and regression 9.5a incl. foreign-device and stale-generation barriers (642). Matches spec 274–289. Caveat: the barrier it relies on may be unmintable in production for GBM-backed payloads — see B-1 below (new root cause in Tasks 2/4, not a regression of this correction). |
| M-1 — no owner-writer authority contract | **APPLIED** | `OwnerWriteGrant`/`WriterClass`, issuer restricted to `Owner` state on the serialized path, four-field binding validated at the sink, by-value single consumption, monotonic serials, drop-is-not-release, `revoke_owner_writes` as sole clearing path, and 6.5b sink tests (plan 376–416, 446). |
| M-2 — overlay re-claim lacked a physical-retirement transition | **APPLIED, wording overstated** | 7.5a defines the 1→0/0→1 edges, pending-vs-dispatched unflip, stop-path decision, device-loss and failure routes with tests (519–528). Verified against baseline: `deferred_cow_release: bool` (backend.rs:1164), set at 20766, cleared at 20647/20733, consumed at 1853–1857. It is *not* "generation-keyed" as the plan claims; correctness rests on reusing the same `cow_id`/allocation identity, which is adequate. See m-2. |

## Findings

### Blocking

#### B-1 — Quarantined GBM-backed payloads retain the DRM file description that their own KMS barrier requires closed

Task 4.3 places "GBM BO/device retention" inside the retained shared/copied payload (plan 286). In the baseline that device is `type GbmDevice = gbm::Device<Rc<crate::drm::Device>>` (scanout.rs:75), held as `Option<Rc<GbmDevice>>` by the pool (scanout.rs:615): the GBM device *is* a strong reference to the control file description. Task 9 makes `DeviceBarrier::FileFamilyClosed` the only non-`PriorBufferReleased` way to leave `KmsDisposition::Outstanding`, minted only "for the complete `IncarnationFdSet`" after every alias is closed (plan 620; Task 2.3 "a single closed control FD cannot mint `FileFamilyClosed`"). The spec requires that registered aliases retained by allocation contexts "must not become hidden references that prevent or falsely certify closure" and that the plan "define their post-close/device-loss-safe cleanup without issuing stale-handle ioctls" (spec 267–270).

Concrete sequence: acceptance-unknown direct/composed commit whose old BO is GBM-allocated → entry quarantined with `Outstanding` KMS obligation → bundle moved to the recipient (9.3) → helper reaped, backend `Rc<Device>` dropped → the quarantined payload still owns `Rc<GbmDevice>` → `Rc<drm::Device>` never reaches zero → fd never closes → `FileFamilyClosed` can never be minted → `apply_teardown_release` returns `InvalidProof` forever. This is the exact "treat as unavoidable leak" outcome the spec forbids (289), yet the plan's only escape is a barrier the plan's own ownership makes unreachable. The inverse resolution (drop the GBM device first, then call `gbm_bo_destroy`) is the stale-handle ioctl the spec prohibits. Neither Task 2 nor Task 4 decides this; Task 9.5 tests closure with a "fake family inventory", so the cycle is undetectable by the planned evidence.

Smallest correction: define in Task 2/4 that registry-tracked `Rc<drm::Device>` holders inside quarantined payloads are *counted* aliases whose closure is performed by the barrier discharge itself: `FileFamilyClosed` is mintable when the only remaining holders of the description are registry-rooted payload contexts and all submitters/dispatch are detached; discharge then destroys those payloads in order (GBM BO before device drop) and closes the description as the final step. Add a Task 9.5 case using a real `Rc<drm::Device>`-owning payload (not a fake inventory) that asserts the barrier is reachable and no ioctl is issued after close.

### Major

#### M-1 — No producer or correlation contract discharges the old buffer's `KmsRelease` obligation

Task 7.4 registers "one correlated KMS obligation for each affected allocation/output" before IPC (502), and Task 9 says `Discharged` "requires correlated `PriorBufferReleased` for that exact commit/CRTC generation" (620). But nothing in Tasks 5–8 says which event constitutes that proof or who calls `apply_validated_proof(key, kms_obligation)`. The disposition table (506–514) records `HardwareComplete` only "for future damage consumers" and moves `CompletionRetired` old resources into "release waiting" with obligations "remain explicit". Baseline evidence: the owner emits `CompletionRetired { resources: Accepted<R> }` from `try_complete` only after accepted + hardware_complete (+presented) (device.rs:951–978); `PriorBufferReleased` exists nowhere in code except comments deferring it (ledger.rs:10, 183). Governing spec defines it as "the replacement's release dependency proves the previous scanout buffer is no longer used" (2109–2111) — i.e. commit N+1's `HardwareComplete` discharges an obligation registered at commit N, keyed by N's `GroupMember` set.

Consequence: the plan's central retirement path has no producer. `CommitResources` has no field carrying `ObligationId`s (459–465), so even a consumer that wanted to discharge cannot correlate. Tests in 1.1/4.1/7.1 apply KMS proofs directly from the test body, so the matrix (Task 10, "Grouped A/B frame… shared source retained until every required replacement") can pass without the production discharge route ever existing. An implementer guessing the obvious — discharge at the *same* commit's `HardwareComplete` — violates "a current buffer's own presentation never proves it idle" (Global constraints).

Smallest correction: in Task 7 add an `OwnerEvent<CommitResources>` disposition row: on `CompletionRetired`/`HardwareComplete` of commit N+1, for each member of its `old` set, correlate `(producing commit N, GroupMember)` to the `KmsRelease` obligation registered at N and discharge it per member (partial grouped replacement discharges only matching members). Carry `Vec<(AllocationKey, ObligationId, GroupMember)>` in `CommitResources` or a consumer map keyed by commit. Add a regression where the only KMS proof comes through the owner event, not a direct `apply_validated_proof` call, and where the same commit's own completion does not discharge its `new` set.

#### M-2 — `Quiescing` admits no writer class, so a drain that needs an unflip cannot complete

Task 6.5 asserts `allows_legacy(class) == false` for every class, including `Unflip`, immediately after `begin_quiescing` (424–432), and `authorize_owner_write` returns `Detached` outside `Owner` (412). Publication of `Owner` requires the consumed `LegacyDrained` proof (408). If `begin_quiescing` may be entered while direct scanout is active, the drain requires a composed unflip (Task 8.5, spec 444–452), which is a mutation no state permits; the gate then either wedges or the implementer adds an undocumented bypass at exactly the sink 6.5a is meant to close. The plan states no ordering rule (e.g., "quiescing is refused while `scanout_m2.active()`" or "unflip is the last legacy write and precedes `begin_quiescing`"). Production handover is unreachable in 2c-i, but the gate vocabulary and its sink tests are in scope and would enshrine the wedge.

Smallest correction: state the precondition — `begin_quiescing` refuses (`Busy`) while any direct ownership unit is Current/Submitted, or `Unflip` is the single class `Quiescing` still permits under legacy authority, with the 6.5/6.5a assertions adjusted to whichever is chosen.

### Minor

#### m-1 — 1 ms retry on non-exportable tickets has no pending-ticket deadline

Task 6.3 polls tickets lacking exportable FDs at 1 ms (420) and retains *failed* tickets for teardown, but a hung GPU returns `Ok(false)` indefinitely; nothing in the service classifies a never-signalling ticket. Spec 215–217 wants wakes driven by ticket updates and no readiness loop; a 1 ms permanent timer under VT-away is a spin in slow motion. Define a bounded pending deadline after which the batch is frozen/quarantined (route closed) rather than re-armed.

#### m-2 — 7.5a describes the baseline flag as "generation-keyed"; keying is by retained identity

Verified: it is `deferred_cow_release: bool` plus `cow_id` reuse. The design is sound because the 0→1 edge reuses the same allocation key and a fresh generation appears only after `finish_cow_release`. Reword, and add to the 7.5a test list the case the bullets assert but the tests omit: after a completed `finish_cow_release` and fresh 0→1 allocation, a late stop-path/unflip evidence for the old identity retires nothing.

## Coverage and implementation checks

- **Incorporation audit:** all three round-2 findings checked against task text; one wording overstatement (m-2).
- **Architecture/contracts:** checked owner↔consumer event delivery (verified `OwnerEvent` variants and `try_complete` emission), KMS-obligation producer (gap → M-1), gate state machine vs drain (M-2), capacity ownership in the consumer, bundle composition, `R` bounds on `DeviceCommitOwner` (no `Send` bound found; no conflict with `Rc` leases).
- **Safety/ownership:** checked teardown barrier reachability against real GBM/DRM ownership (B-1), overlay re-claim sequences against backend.rs 1829–1857, 20632–20648, 20755–20771 (sound), grant consumption/drop semantics.
- **Spec compliance/verification:** checked spec §2 upstream integration, §3, §4 (both subsections), §5–6, governing milestone list 2100–2128. Verification gaps: Task 9.5's fake family inventory cannot expose B-1; Tasks 1/4/7 tests inject KMS proofs directly and cannot establish M-1's route. Build/clippy/portability/software-Vulkan gates are correctly assigned to implementation (Task 10.4).
- **Excerpts used: 11/12** — spec 94–176, 177–296, 357–476; governing 2100–2129; backend.rs 1826–1870, 20625–20704, 20722–20781; owner/device.rs 41–110, 930–989; owner/ledger.rs 1–60, 170–199. Locator greps only for headings, `deferred_cow_release`, `PriorBufferReleased`, owner type declarations, GBM/device holders.
- **Unassessed (not presumed sound):** Task 3 storage/relayout adapter internals, Task 5 read/scratch adapter, copied-route dual-context ownership beyond the GBM alias question, 6.5a caller completeness, cursor/gamma sinks, `consume_owner_write` ordering relative to executor-mediated (unknown-outcome) dispatch.
- **Deferred to compiler/tests:** all signatures, borrows, fixture construction, portability builds, Vulkan/DRM execution.

Follow-up question requiring an answer before execution: for GBM-backed retained payloads, which owner performs the last close of the DRM description, and in what order relative to `gbm_bo_destroy` (B-1)?
