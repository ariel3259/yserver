# Stage 2c-iii design — codex review, round 3

**Target:** `docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md`
revision 3 (`841a97c7`), with round 2 as the prior review.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
**Not comparable** to rounds 1–2: `da807b70` raised the reading allowance from
12 to 24 excerpts. Coverage this round: complete for declared scope, 24/24.

**Author verification (2026-09-19), every finding checked against the tree:**

- **B-1 — CONFIRMED.** `present_consumers` is `Vec<u32>` of CRTCs; the closure
  rejects one outside the kernel event set (`owner/closure.rs:67`, `:215`).
  And `managed_enqueue_retired_direct_completion` (`backend.rs:2638`) republishes
  the frame's event at retirement, so an owner-side completion would have been a
  second authority. Fixed differently from the reviewer's suggestion, which was
  a transfer into the commit consumer: the direct frame stays the **single**
  authority (it already carries the event and is moved intact at confirm);
  confirmation binds it to the `CommitId`; the owner's `Presented` only supplies
  the MSC/UST sample; `present_consumers` names CRTCs only.
- **M-1 — CONFIRMED.** Debt §9.2 defines P3-3 as a buffer retained across the
  flip; revision 3's composed → direct → unflip sequence retains none, and §9.3
  requires a hardware mutation per invariant. Fixed: §6.4 step 4 (a
  retained-allocation commit, F8 if unreachable on card1) and two hardware
  mutations in §8.2.

Revision 4 incorporates both.

---

## Verdict

**1 blocking, 1 major, 0 minor**

Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a bounded design review, not a claim that the code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Round-2 finding | Status | Assessment |
|---|---|---|
| B-1 — composed Present had two terminalization owners | **APPLIED** | Composed commits are now explicitly non-Present, while composited Presents remain owned by the GPU batch ([target §3.3 lines 200–217](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:200)). The context-aware entry correctly moved to Cii ([lines 348–353](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:348)). A separate direct-carriage defect remains as B-1 below. |
| B-2 — damage transaction installed after event visibility | **APPLIED** | Section 4.2 now installs the transaction inside the CommitId-aware ledger closure and requires returned events to be routed only afterward ([target lines 282–292](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:282)). The mutation explicitly moves installation back after confirmation ([line 501](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:501)). |
| M-1 — pool-release gates not independently proven | **APPLIED** | Section 4.2 names `CompletionRetired`, discharged `KmsRelease`, and the GPU fence separately ([target lines 304–314](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:304)); section 8.2 now drops each gate independently ([line 499](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:499)). |

## Findings

### Blocking

#### B-1 — Direct Present handoff names the wrong carrier and leaves the old completion authority alive

Section 3.3 says the prepared frame retains serial/FIFO/target identity and that the description carries “it as `present_consumers`” ([target lines 218–236](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:218)). But `CommitDescription::present_consumers` is `Vec<u32>` ([build.rs lines 27–35](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/build.rs:27)) representing CRTC members of the kernel event set: closure construction rejects a consumer CRTC outside that set ([closure.rs lines 67–68](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/closure.rs:67), [lines 215–217](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/closure.rs:215)). It cannot carry a Present serial, FIFO position, or notification payload.

The existing accepted-direct authority also survives the proposed conversion. Preparation stores the complete `CompletedPresentEvent` in `DirectPresentFrame` ([backend.rs lines 20059–20075](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20059)); confirmation moves that frame intact into `pending` ([lines 2620–2631](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2620)); `CompletionRetired` later clones its event into the legacy completed queue ([lines 2638–2658](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2638), [lines 19895–19906](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:19895)).

Concrete failure: after owner `Presented` terminalizes the request, retirement republishes the same `CompletedPresentEvent`, permitting a second CompleteNotify/FIFO wake. Encoding the serial in `present_consumers` instead is rejected or interpreted as a CRTC. This violates exact-once, separately keyed terminalization ([stage-2c lines 119–125](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:119); [C.0 lines 2258–2285](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2258)).

Required correction: distinguish the CRTC `present_consumers` set from the protocol ledger payload. Specify the atomic confirmation-time transfer of each request’s `PresentKey`/notification state from the queued frame into the commit consumer; accepted pending state must no longer republish that event at retirement. Preserve the existing frame-owned path only for never-submitted displacement, and define how its deferred Skip waits behind the owner-terminalized predecessor.

### Major

#### M-1 — The claimed P3-3 closure has no retained-member hardware case

P3-3 specifically requires a buffer retained across a flip to register no `KmsRelease` obligation and remain unreleased ([debt lines 458–468](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:458)). The debt procedure also requires P3-2/P3-3 mutations under the same hardware filter ([lines 476–488](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:476)).

The proposed hardware sequence only performs composed → direct → unflip ([target lines 441–459](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:441)); it requires no commit containing both a changed member and an unchanged retained allocation. Section 8.2 mutates omitted registration, but provides no retained-member over-registration mutation ([target lines 503–508](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:503)).

Thus an implementation that registers and later releases retained members can pass vacuously while the design declares the F8 closed.

Required correction: require a real-kernel commit that changes one managed member while retaining another, assert that the retained allocation gets no obligation or release, and run separate hardware mutations for omitted displaced registration (P3-2) and erroneous retained-member registration (P3-3).

### Minor

None.

## Coverage and implementation checks

- Incorporation: all three round-2 findings audited.
- Architecture/contracts: traced the complete implemented successor offer, replacement, never-submitted cleanup, confirmation, retirement enqueue, deferred Skip publication, and admission wake ordering.
- Safety/ownership: checked Present exact-once authority, failure/refusal handling, pool-release gates, and retained-resource evidence.
- Compliance/evidence: checked stage-2c producer, damage, activation and terminalization clauses; debt §§4.4/9.5; and C.0 §§10.2/10.4.
- Excerpts used: **24/24**.
- Not independently re-audited beyond implicated contracts: unrelated details of C.0 §§9, 13, and 18, and exact owner API implementation shapes. They are not deemed sound by omission.
- Deferred to implementation: Rust signatures and borrowing, formatting, clippy, portability builds, deterministic execution, mutation execution, and the tty2 hardware run.