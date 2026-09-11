## Verdict

**1 blocking, 2 major, 2 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE** (reading budget 9/12; declared unassessed areas listed at the end).

Reviewed baseline `73547c6b` (plan @ Task 2.5 revision, includes upstream `a06cf0e0`). Design-review result only; no claim that code compiles, tests pass or implementation is approved.

**Reviewer:** `claude --print`, read-only tool set, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `claude-opus-5`; reasoning effort `medium`; `claude 2.1.268 (Claude Code)`.
Dispatched by `review-claude.sh` because codex was unavailable.
**Not comparable to any codex round**, including reviews citing this same SHA:
the reviewer is part of the instrument. This reviewer also inherits the repo's
CLAUDE.md/AGENTS.md and the user's global instructions, which codex does not.
Prior: the round-3 plan review (same instrument).

## Incorporation audit

| Round-3 finding | Status | Assessment |
|---|---|---|
| B-1 — GBM device holders block the fd-family barrier | **APPLIED, TRADED** | Counted-alias rule at 2.3 (plan 197), 4.3 (286), the three-step discharge order (625–633), real-payload proofs at 2.5 (199) and 9.5 (652). Verified `GbmDevice = gbm::Device<Rc<drm::Device>>` (scanout.rs:75). The correction's step 2 ("GBM BOs first… their GEM close") now makes explicit a second GEM-handle owner alongside the Task-2 right → new B-1 below. It also destroys part of a single-payload entry while the rest stays quarantined → M-1 below. Note the baseline `ScanoutBo` holds `drm: Rc<Device>` directly (scanout.rs:528) even without GBM; the 2.3 general rule covers it, but 4.3/Task 9 text names only `Rc<GbmDevice>`. |
| M-1 — no `PriorBufferReleased` producer | **APPLIED** | `kms_obligations` triples (465–467), registration before IPC (505), `HardwareComplete` row discharges the `old` set only, rejection cancels (511–513), owner-event-only regression (533). Verified the owner emits `HardwareComplete` once per commit only after every `expected_completion` CRTC's out-fence succeeded (device.rs:846–891; closure.rs:196–206 includes every old/new-active CRTC), so the "displacing commit's own out-fence" is correct evidence and per-member partiality is vacuous but harmless. Gap for retained members → m-1. |
| M-2 — `Quiescing` admits no class | **APPLIED** | `begin_quiescing` → `Busy` while Current/Submitted/Successor or an unretired unflip (422); unflip is the last legacy write; matches spec 444–452. No `Closed` precondition was added, so device-loss closure is still reachable. Interaction with owner-grant accounting → M-2. |
| m-1 — no pending-ticket deadline | **APPLIED** | Bounded checked deadline, freeze on expiry, no re-arm, expiry never a proof (420). Consequence under VT-away → m-2. |
| m-2 — "generation-keyed" wording | **APPLIED** | 522 now says "retained identity (`cow_id`/allocation key)"; late-evidence case added to the 7.5a test list (531). |

## Findings

### Blocking

#### B-1 — GBM-allocated shared payloads have two owners of one GEM handle; the second `GEM_CLOSE` is a stale-handle ioctl that can destroy another live buffer

Task 2.3 makes the cleanup right a consuming `RMFB → GEM_CLOSE` sequence with a `FramebufferRemoved` retry state (plan 197). Task 4.3 puts "FB/GEM rights" **and** "GBM BO/device retention" in the same shared payload (286), and the round-3 step 2 states the GBM BO's own GEM close runs at barrier discharge (628). Verified baseline: on the GBM-alloc path `PRIME_FD_TO_HANDLE` on the same fd "returns the existing GEM handle rather than creating a new one when the source is a gbm_bo" (scanout.rs:3115–3120); the kernel does not refcount that handle. So the right's `close_gem(h)` and `gbm_bo_destroy` (which closes `h`) target one handle.

Sequence (ordinary managed retirement, Task 4.6): right discharge → `RMFB(fb)` → `GEM_CLOSE(h)` → `Discharged`; later payload destruction → `gbm_bo_destroy` → `GEM_CLOSE(h)` again. If between them any pool acquisition, DRI3 import or probe on the same fd allocated a handle, the idr hands out the lowest free number — `h` — and the second close destroys a *live* entry's handle (its pending `AddFB2` fails, its own right later fails with `EINVAL`). The same collision recurs on 2.3's partial-failure retry (`FramebufferRemoved` re-issuing `close_gem` while the gbm_bo still owns `h`). Spec 269–270 forbids stale-handle ioctls; the plan's "one authoritative owner" model is broken for exactly the payload class the round-3 correction addresses.

Smallest correction: one GEM closer per payload. For GBM-alloc payloads the `DrmCleanupRight` carries FB identity plus a *non-closing* GEM record (`GemOwner::Gbm`), and the gbm_bo drop, ordered after `RMFB`, is the sole `GEM_CLOSE`; for Vulkan-export payloads (no gbm_bo) the right owns the GEM close as today. Add to 4.6 a counting-transport assertion of exactly one `CloseGem(h)` across right discharge plus payload destruction, including the partial-failure retry path.

### Major

#### M-1 — Barrier discharge destroys part of an entry whose model has one payload and one availability state

Task 1 defines `AllocationEntry` as owning "the payload and a single availability state" (85, 87). Steps 2–3 (628–631) destroy the GBM BO, `GbmDevice` and device alias while "shared dma-buf and Vulkan contexts… keep their independent proofs" — i.e., the entry survives half-destroyed, `Superseded(FileFamilyClosed)` set, with a frozen GPU batch that "normal proofs alone do not unfreeze" (608). No payload variant or entry state expresses "file-owned part gone, shared part retained", so an implementer must either destroy the whole payload at step 2 (destroying a VkImage under possibly-executing GPU work) or leave the device alias alive (barrier unmintable — the round-3 B-1 leak). Additionally the stated order (gbm_bo before VkImage) inverts the baseline's dependency comment (scanout.rs:3476–3478; 552 "kept alive so the gbm_bo outlives" the imported image). It is kernel-safe via dma-buf refcounts, but the plan asserts it without evidence.

Correction: split the Task-4 payload into `file_owned: Option<FileOwnedBacking>` (right, gbm_bo, `Rc<Device>`) and `shared: SharedBacking` (image/memory/view/transfer, `VkContext`); step 2 takes only `file_owned`; the availability ledger tracks the two halves' dispositions separately (the KMS/file-owned pair vs GPU/read/FOREIGN). Require the Task 10.2 live-Vulkan smoke to run gbm_bo-before-VkImage destruction under validation layers, since 2.5/9.5 fixtures cannot prove driver behavior.

#### M-2 — `OwnerWriteGrant` has no defined consumption point relative to unknown-outcome dispatch, and 9.3 cannot close a gate with grants outstanding

The contract (410–416) says one grant per dispatch, consumed by value, and `close`/`begin_quiescing`/handover refuse while `outstanding_owner_writes() != 0`; `revoke_owner_writes` is the only clearing path. Owner-mediated mutations go through `KmsIoExecutor::send`/`dispatch_blocking_at_boundary` (442), whose outcome is unknown until reply. The plan never states whether `consume_owner_write` happens at send or at reply. If at send, the outstanding guard protects nothing that matters (the in-flight request is invisible to it). If at reply, then under `ExecutorStalled` Task 9.3's mandatory first step "close transport" (650) refuses forever unless the handoff revokes first — and the plan says a revoked/dropped grant "closes admission", not what it means for a request that may already have executed in the kernel. 6.5b would enshrine whichever the implementer picks. Round 3 listed this as unassessed; it is now the only undefined edge of an otherwise complete contract.

Correction: define consumption at executor acceptance of the request (the serialized send boundary); unknown outcome thereafter belongs to the owner's quarantine, not to the gate count. State in 9.3 that handoff calls `revoke_owner_writes` before `close` and treats every revoked grant as possibly dispatched (owner `Quarantined`, never "cancelled"). Add the stalled-executor close case to 6.5b/9.6.

### Minor

#### m-1 — A member retained across a grouped commit must register no `KmsRelease`

7.4 registers an obligation "for each displaced (`old`) allocation and member" (505), but `ResourcesStillCurrent` requires `old` to be the complete previous current set. A grouped commit changing only CRTC 1 has `C@crtc2` in both `old` and `new`; the `HardwareComplete` row discharges its obligation while it is still scanning out, and `CompletionRetired` moves `C` into release-waiting and current simultaneously. Safety then rests on the `new` lease being a `Kms` use (blocking `Write`), which 7.4 does not require. State that a displaced pair is `(allocation, member)` with `new[member] != old[member]`, that retained members register nothing, and add the case to 7.6.

#### m-2 — The 6.3 pending deadline closes managed admission on a legitimately suspended GPU

Non-exportable tickets now freeze on a bounded deadline (420). Under VT-away/DPMS-off (the exact scenario 6.1 tests) GPU work submitted before the switch may not progress until VT return; on expiry the batch freezes, admission closes for the incarnation and, per 608, no later proof unfreezes it. Define the deadline as a wall-clock bound on *serviced* time (paused while the seat is inactive) or require VT-return to reset it; otherwise a long VT switch is a route-closing event.

## Coverage and implementation checks

- **Incorporation audit:** all five round-3 findings checked against task text; B-1 applied but traded (new B-1, M-1); M-1/M-2/m-1/m-2 applied; two consequences (M-2, m-2).
- **Architecture/contracts:** checked `HardwareComplete` emission semantics against the M-1 correction (all-or-nothing per commit, fence-backed for any active CRTC), GEM-handle ownership between right and gbm_bo, gate-count vs executor dispatch, retained-member set semantics.
- **Safety/ownership:** double `GEM_CLOSE` with handle reuse (B-1), partial payload destruction at the barrier (M-1), close-refusal during stalled handoff (M-2). Raw-fd duplicates of the control description: only `device_lock.rs:226` (test-only, lock file, not the DRM description) and dma-buf clones found — no unregistered alias class identified.
- **Spec compliance/verification:** spec 255–296 and 440–475 checked; 2.5/9.5 real-payload proofs cannot establish driver-side safety of gbm_bo-before-VkImage (M-1); 4.6 lacks a once-only GEM-close assertion (B-1). Build/clippy/portability/software-Vulkan gates remain correctly assigned to 10.4.
- **Excerpts used: 9/12** — device.rs 700–759, 830–899; scanout.rs 480–539, 3100–3149, 3420–3441; device_lock.rs 200–249; spec 255–299, 440–475; closure.rs grep-with-context. Locator greps: owner `HardwareComplete`, scanout GEM/GBM symbols, ledger `Submitted`, `try_clone/dup`, spec `quiesc|unflip`.
- **Unassessed (not presumed sound):** Task 3 storage/relayout and DRI3 metadata retention; Task 5 read/scratch adapter; copied-route dual-context ownership; 6.5a caller completeness for cursor/gamma; Task 8 role transitions beyond the M-2 precondition; whether the kernel-event (`kernel_event`) `HardwareComplete` producer at device.rs:908/934 has the same all-members rule as the fence path.
- **Deferred to compiler/tests:** all signatures, borrows, fixture construction, portability builds, Vulkan/DRM execution.

Follow-up design question requiring an answer before execution: for GBM-allocated shared payloads, which single owner issues `GEM_CLOSE`, and how is the entry's file-owned half separated from its Vulkan half so the barrier can discharge one without the other (B-1, M-1)?
