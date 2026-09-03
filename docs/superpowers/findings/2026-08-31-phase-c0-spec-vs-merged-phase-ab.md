# Phase C.0 spec — adversarial review against the merged Phase A+B

**Provenance:** Adversarial review produced by Opus on 2026-08-31; moved from
the original artifact `2026-08-31-phase-c0-spec-vs-merged-phase-ab.md` without
changing the review body below.

**Reviewed spec:** `2026-08-26-phase-c0-atomic-kms-migration-design.md` (Date: 2026-08-26, `Depends on: PR #129`)
**Reviewed against:** `master` @ `fc76b743` — PR #129 squash-merged 2026-08-31, plus the
four upstream commits it landed on (`f4433567`, `db795693`, `fe0d3369`, `c414be1a`).
**Maintainer directives:** `joske/yserver#129` and `#115` comment threads, retrieved via
`gh api repos/joske/yserver/issues/comments`. See "Maintainer intent" below — **every
divergence in §§A-D was requested by the maintainer**, so this review's job is to say how
C.0 must be rewritten to absorb them, not to argue the code back toward the spec.
**Supersedes:** `2026-08-28-phase-c0-spec-gaps-after-phase-ab-review.md`, which was
written against the branch tip `ff31f14c` and without the maintainer thread.

## Why this is not a re-run of the 2026-08-28 review

The merge is **not** `ff31f14c` plus a commit message. `git diff ff31f14c master -- crates/`
is 600 insertions / 188 deletions across 9 files. One commit that never existed on the
reviewed branch — *"fix(present): preserve clocked async target identity"* — landed inside
the squash and **reverses the central premise of the previous review**, and one upstream
commit (`f4433567`, non-desktop connectors) changes the output-discovery model the spec's
protocol-domain definition rests on.

Net effect on the earlier findings:

| 2026-08-28 item | Status against `fc76b743` |
| --- | --- |
| 1. C.1 async latest-wins contradicts amended Phase A rule | **Resolved by the code, and re-opened one layer down** (§A below) |
| 2. Absolute-vblank arming not inventoried | **Stands, unchanged** (§C) |
| 3. Second Present-completion wake source | **Stands**, and now has a second-order consequence (§A) |
| 4. NVIDIA hardware cursor is policy-gated | **Stands, but my earlier reading of *why* was wrong — see §E, now blocker-class** |
| 5. Stale evidence base for the #129 dependency | **Worse** — the handoff is now self-contradictory (§H) |
| 6. Outdated Phase B baseline description | **Stands**, and the baseline moved again (§G) |
| 7. Unsupported "bounded" claim in §9.1 | **Moot** — async no longer parks; replaced by §A |
| 8. Broken `phase-c2` reference | **Stands** — file still absent (§H) |

---

# Maintainer intent — the constraints C.0 must now be rewritten *to*

Every divergence in §§A-D was made **at the maintainer's explicit request** on
`joske/yserver#129`. They are not drift to be corrected back toward the spec; they are
decisions the spec must absorb. Source: `gh api repos/joske/yserver/issues/comments`,
`joske` (Jos Dehaes, upstream maintainer). Quoted verbatim.

**M1 — no gating env vars (standing policy).** 2026-08-28 09:00:

> *"please remove all these gating env vars, I know the agents like to do that, but it's
> just bad practice. It will get merged and then it's a maintenance burden."*

**M2 — core supersession follows Xorg `present_scmd.c` exactly.** Same comment:

> *"Xorg's present_scmd.c requires both CRTC and target MSC equality before scrapping.
> Please preserve async requests with different effective targets … For
> effective_target_msc=None, the safe behavior is not to supersede until equivalence is
> known."*

**M3 — the Xorg/wlroots successor model, mandated, and not scoped to async.**
2026-08-28 14:08, after MATE compositor-off exercised the unredirected path at 42-48 fps:

> *"When a direct flip is pending, another eligible authoritative-root Present must not
> call request_direct_unflip() or fall through to Copy/composition. Please follow the
> Xorg/wlroots model: keep one hardware flip in flight and queue/coalesce an eligible
> successor, preferably latest-wins, **then submit it after retirement**. The queue must
> remain bounded while preserving Present completion/Idle/Skip ordering."*

Note the population: *"another eligible authoritative-root Present"* — synchronized
included. And the submission point is specified, not incidental.

**M4 — clocked async needs Xorg target identity; blanket `None` parking is rejected.**
2026-08-31 07:29, with Warframe's real async stream:

> *"the game's own FPS collapses from ~200 to exactly 60. present_skips remains zero,
> showing that async frames are being held by the core scheduler rather than coalesced
> through the new backend successor slot. The client exhausts its buffers and becomes
> vblank-blocked. The backend successor queue is correct, but clocked async Presents need
> Xorg-style target identity and same-target supersession in core or another path that
> **immediately releases superseded async buffers** while retaining only the newest
> successor. Blanket effective_target_msc=None parking is incompatible with uncapped
> direct scanout."*

**M5 — the NVIDIA cursor policy stays out of the branch.** `ariel3259`, 2026-08-31 11:46,
accepted by the maintainer 21 minutes later:

> *"it was tested on cs2 with nvidia cursor policy disable (only for test, not in the
> branch)"*

The policy itself is **not** a #129 artifact. Provenance, verified against the tree:

- designed and implemented 2026-07-26 on `perf/nvidia-staging-buffer-pool`
  (`docs/superpowers/plans/2026-07-26-nvidia-cursor-sw-on-drag.md`);
- reaches `master` at `a6f8909c` "Support reverse-PRIME (#95)", 2026-08-25 —
  `git log -S nvidia_policy_disabled -- crates/` finds no earlier commit;
- temporarily disabled 2026-08-29 for the direct-scanout captures, restored 2026-08-31.

Worth pinning because it is easy to misremember as contemporaneous with the legacy-cursor
switch: `nvidia_policy_disabled` **does not exist** at `0b3fd0c8`
*"fix(kms/cursor): use legacy ioctls for HW cursor"* (2026-05-29). The policy is a
*consequence* of living on the legacy cursor path, filed three months later against measured
nvidia-drm behavior — not part of that change. It rests on a driver gap, not on a Phase A/B
decision; see §E, which this review previously got wrong.

**M6 — PR size and stacking.** `#115`, 2026-08-24 19:40:

> *"your PRs are huge and not properly stacked (the subsequent PRs should target the
> previous PR branch, not master again)"*

**M7 — the four upstream commits under #129 are the maintainer's own.** `c414be1a`,
`db795693`, `fe0d3369`, `f4433567` are authored by Jos Dehaes. §F's `non-desktop`
filtering is upstream policy to accommodate, not a divergence to challenge.

### Consequences for the resolution of each finding

| Finding | Resolution direction fixed by |
| --- | --- |
| §A | **M3** excludes the "gate the slot on async" option. **M2** freezes the core rule. The spec must adopt the two-layer model. |
| §B | **M3** mandates submit-at-retirement. C.0 cannot relocate it; §9.2.1 must be rewritten around it. |
| §D | Open, but any added out-fence latency lands on the path **M4** measured. Must be justified against it. |
| §E | **M5** + the 2026-07-26 plan: the policy predates #129 and rests on a measured nvidia-drm driver gap. C.0's goal 3 must answer it, not assume it away. |
| §F | **M7** — accommodate. |
| §I | **M4** — immediate buffer release is a hard requirement; §10.4 is imprecise about it. |
| §J | **M6** — §18's single merge boundary is at risk. |
| §K | **M1** — must be written into C.0 as an explicit constraint. |
| §L | New scope, not a 2026-08-28 finding. **M3**'s *"submit it after retirement"* is the open question — ordering only, or the instant too? **M1** forbids exposing the margin as a lever. |

---

# BLOCKERS

**§E is also blocker-class** — it challenges C.0 goal 3 directly — but is kept below with
the rest of the NVIDIA material so the driver-capability argument reads in one piece.

## A. Phase B now ships the very latest-wins slot §9.1 reserves for C.1 — and applies it to *synchronized* Presents

**What changed in the merge.** `effective_present_target_raw`
(`crates/yserver-core/src/core_loop/process_request.rs:9833`) no longer collapses a
current-or-past async target to `None`; it returns `Some(effective)` unconditionally,
matching Xorg. `classify_msc_due` (`crates/yserver-core/src/present_scheduler.rs:93`) lost
its async-park arm entirely — `None` now means *no clock*, full stop. Async presents
therefore execute immediately again, and the flood is absorbed instead by a **new bounded
successor slot in the KMS backend**:

- `ScanoutM2State::queued_successor: Option<DirectPresentFrame>` (`crates/yserver/src/kms/render/backend.rs:345`)
- `queue_direct_successor` (`:1860`) — one slot, latest wins; the displaced frame goes to
- `defer_direct_successor_skip` (`:1845`) — `COMPLETE_MODE_SKIP`, `emit_idle = false`,
  buffer idled immediately, CompleteNotify held back
- `submit_queued_direct_successor` (`:1805`), called from `retire_direct_output` (`:2296`)

This is, almost verbatim, §9.1's third bullet: *"one C.1 async direct latest-wins intent;
replacing its never-submitted victim completes that victim as `Skip` and releases its
pins/wakes immediately"* — and §10.4's *"C.1's never-submitted replacement victim is
completed once as `Skip`"*, and acceptance 41. The spec assigns that mechanism to C.1. It
has already shipped, in C.0's base, before C.0 exists.

**The blocker is not the duplication — it is the population it applies to.**
`try_present_direct` (`crates/yserver/src/kms/render/backend.rs:15375`) never consults
`candidate.options`; the only use of `options` in the whole scanout path is the diagnostic
log at `:3181`. Its single caller (`process_request.rs:10121`) is the generic Present
execution path, and `923710d0` deliberately admitted **explicit-sync** fullscreen presents
to it. So the queued-successor slot swallows *synchronized* Presents.

Reachability is concrete, and it runs through the other new mechanism (2026-08-28 item 3):

1. Direct frame P1 is submitted; `scanout_m2.pending = Some(P1)`; flip in flight.
2. A tagged absolute sequence event arrives. `on_crtc_sequence_event` (`:9117`)
   unconditionally calls `record_vblank_clock`, so the **general** clock advances while
   P1's flip is still pending.
3. Synchronized P2 with `eff <= clock_msc` → `classify_msc_due` → `ExecuteNow` (the
   `!msc_is_after` arm, which never consults `flip_in_flight`) → `try_present_direct` →
   `queue_direct_successor`.
4. Synchronized P3 arrives the same way → P2 is displaced → **P2 receives
   `COMPLETE_MODE_SKIP`**.

§9.1 forbids exactly this: *"one synchronized Present/direct intent, **which is not
replaceable by async latest-wins** and retains protocol FIFO ordering until submitted or
explicitly rejected through the existing fallback."* §17's acceptance bullet repeats it
(*"bounded primary categories preserve synchronized Present ordering, async latest-wins
`Skip`"*), and §12 asserts *"Phase B's primary-plane eligibility and **synchronized flip
behavior** remain unchanged"* — which is now false in its base.

Note this is a *different* skip from the core's `supersede_covered_pending_presents`
(`process_request.rs:9390`), which is still target-scoped (`effective_target_msc == Some(target)`
on both sides, `:9405`) and still coverage-checked via `present_supersession_covers`. The
backend slot has **no target equivalence check and no coverage check at all** — it discards
P2 for P3 on nothing but arrival order on the same plane.

**The resolution is not open — the maintainer has already chosen.** M3 mandates the slot
and explicitly scopes it to *"another eligible authoritative-root Present"*, so gating it on
`PRESENT_OPTION_ASYNC` is off the table. M2 independently freezes the core rule at Xorg
`present_scmd.c` equivalence. C.0 must therefore stop describing one mechanism and describe
**two layers with different equivalence units**:

- **Present layer (core), M2.** Scrap requires CRTC + target-MSC equality plus coverage.
  `None` never establishes equivalence. This is a statement about *requests*, and it is
  Xorg-parity. Unchanged by C.0.
- **Plane-ownership layer (KMS owner), M3.** A never-submitted intent for the single primary
  plane of one CRTC is physically unpresentable once a newer intent exists for that plane.
  This is a statement about *the plane*, not about request equivalence, and it is why it
  legitimately applies to synchronized frames where the Present-layer scrap does not.

Concrete C.0 rewrites:

1. **§9.1, third and second bullets.** Delete *"which is not replaceable by async
   latest-wins"* from the synchronized bullet. Merge the synchronized-direct and
   "C.1 async direct" bullets into one **primary-plane latest-wins slot** owned by C.0, not
   C.1, admitting any eligible authoritative-root direct intent regardless of the async
   option bit. State the plane-ownership argument above as its justification, and state
   explicitly why it does not license the Present-layer scrap M2 rejected.
2. **§10.4, last bullet.** Retarget from *"C.1's never-submitted replacement victim"* to the
   C.0 primary-plane slot.
3. **§12.** Replace *"synchronized flip behavior remain unchanged"* — it is changed, by
   maintainer request, in the base.
4. **§17 acceptance 41 and the §16 bullet.** *"bounded primary categories preserve
   synchronized Present ordering, async latest-wins `Skip`"* must become the ordering
   guarantee M3 actually asked for: *completion/Idle/Skip ordering preserved*, with the
   Skip published behind the in-flight predecessor (see §I).
5. **§14.** C.1 no longer introduces the latest-wins slot; it inherits it and adds only
   `PAGE_FLIP_ASYNC`. Re-scope accordingly.

One thing C.0 *should* add that the merged code lacks: the backend slot performs **no**
coverage or target check (`queue_direct_successor`, `backend.rs:1860`), while the core's
`present_supersession_covers` does. For an authoritative-root fullscreen frame that is
sound by construction — full-plane replacement — but the spec should say so as an
invariant, and say what must happen if the eligibility predicate ever admits a
partial-coverage frame.

## B. The direct-successor resubmit sits inside the DRM event handler, ahead of every §9.2.1 admission tier

`retire_direct_output` now runs, in order (`backend.rs:2292-2296`):

```
bind_direct_cursor_on_all_outputs();   // legacy set_cursor2 + move_cursor
submit_queued_direct_successor();      // atomic_commit, plane props only
```

Under C.0 that second line is *a new primary commit taking the sole device slot,
dispatched from the completion path of the commit that just released it*, with no
consultation of any queue.

This is a mechanical violation of §9.2.1:

- *"After that commit completes, no new primary commit may take the device slot while an
  aged maintenance ticket exists"* — a cursor or gamma intent that aged behind P1 is
  jumped by P2 at every single retirement.
- The tier order (topology barrier → unflip/recovery → aged maintenance → primary →
  non-aged maintenance) is bypassed entirely.
- §9.3's *"The retirement/event wake immediately attempts the newest queued **cursor**
  intent"* is contradicted: the retirement wake attempts the newest queued **primary**.

Because a fullscreen no-vsync stream keeps `queued_successor` permanently occupied, the
steady state is: every device slot freed by a retirement is immediately reclaimed by the
next primary. Under C.0's single-device-slot rule (§9, *"exactly one dispatched-or-submitted
live atomic transaction per DRM device"*) that is **unbounded maintenance starvation**, and
it lands squarely on the two §16.3 gates C.0 must pass:

> *"For `N=1`, continuous cursor motion must retain at least 95% of the idle-cursor
> direct-scanout FPS … input-to-submit p99 must not exceed one output period plus 2 ms"*

Today those gates are met only because the cursor bind on the preceding line is a **legacy
ioctl** (`cursor_plane.rs:473` `show` → `show_legacy` → `set_cursor2` + `move_cursor`),
which the module's own comment justifies precisely by its non-interference: *"Legacy cursor
ioctls don't EBUSY-collide with atomic scanout commits on the same CRTC … we avoid the
atomic-cursor-vs-atomic-pageflip storm that motivated the (now-abandoned)
bundle-cursor-atomic branch."*

C.0 deletes that escape hatch by construction (goal 3, §5). This is the largest C.0
redesign item, and the obvious fix is **unavailable**: M3 specifies the submission point
(*"then submit it after retirement"*), so C.0 cannot relocate the resubmit into a generic
admission pass and call it solved. §9.2.1 must be rewritten around a mandated
submit-at-retirement. Required additions:

1. **Make retirement-time direct-successor promotion a named admission tier**, not an
   event-handler side effect — and place it in the tier list explicitly. It cannot simply
   be tier 4 ("the oldest ready primary replacement"), because tier 3 (aged maintenance)
   would then preempt it and break M3's *"keep one hardware flip in flight"*.
2. **Specify the yield rule that keeps §9.2.1's `N - 1` starvation bound true** when the
   primary stream refills its own slot at every retirement. The honest options are (i)
   absorb cursor/gamma into the successor commit so maintenance never needs its own slot
   (see 3 below, and §D), or (ii) let an aged maintenance ticket displace one successor
   promotion per `N` retirements and state the resulting frame-drop budget. Option (i) is
   the one consistent with both M3 and §9.2.1; it requires §D. **§L proposes a third
   ingredient**: retaining the primary intent until near its target vblank leaves the device
   slot free for most of the interval, which is the yield this bound currently has to invent.
   It does not replace absorption — a cursor moving *between* primary frames still needs a
   seat — but it changes what option (ii)'s frame-drop budget has to cover.
3. **State the absorption rule.** §9.2.1 says *"Every admitted synchronous request includes
   all compatible newest cursor and gamma state"* — the direct successor is a **synchronous**
   primary, so it is an absorption target, not the C.1 async class exempted in the same
   section. `submit_direct_scanout` builds plane-only requests today (§D), so this is a
   real conversion, and under option (i) it is what makes the bound hold.
4. **Re-derive the §16.3 `N=1` thresholds.** The 95% / one-output-period numbers were
   measured against a legacy cursor path that could not collide with the scanout commit.
   Under a single device slot with a self-refilling primary stream they are unproven, and
   M4 already established what the failure looks like from the client side (buffer
   exhaustion, vblank-blocked, 200 → 60 fps).

## C. Absolute-vblank arming still violates §6's event-identity rule — and demonstrably contaminates the clock

Unchanged from the 2026-08-28 review, still un-inventoried, restated here with the concrete
failure.

§6.1 requires an `EventToken` that is *"Non-zero `u64` allocated from one monotonic
namespace … never reused within that incarnation and resolves to exactly one typed target"*,
and §10 requires sequence arms to resolve to
`SequenceArm { hardware_crtc, clock_epoch, purpose, target }`.

The shipped encoding (`backend.rs:1521`):

```rust
const ABSOLUTE_SEQ_TAG: u64 = 1 << 63;
fn absolute_seq_user_data(crtc_id: u32) -> u64 { ABSOLUTE_SEQ_TAG | u64::from(crtc_id) }
```

- `user_data` is a raw CRTC id plus a one-bit kind discriminator. Every arm for a given
  CRTC shares an identical token — the opposite of "never reused".
- The relative arm uses bare `u64::from(crtc_id)`, so a CRTC id of 0 yields the **zero**
  token §6.1 forbids.
- No clock epoch is carried: `absolute_vblank_targets` is keyed by `CrtcKey` only
  (`backend.rs:1029`).

The consequence is not cosmetic. In `on_crtc_sequence_event` (`backend.rs:9117`):

```rust
let completion_eligible = tagged || self.present_completion_is_idle_for(crtc_key);
```

`tagged` is decoded purely from the user_data bit. **No membership check against
`absolute_vblank_targets` is performed.** A tagged sequence event delayed across a
clock-epoch change, or belonging to an arm already retired by the `retain` above it, still
sets `completion_eligible` and calls `record_completion_clock`. That is exactly the case
§16.3 says must be impossible:

> *"inject delayed `DRM_EVENT_CRTC_SEQUENCE` events to prove only a **fresh sequence-arm
> token matching CRTC, epoch, purpose, target, and event type** can advance the reference"*

**Spec work required:**

- Add the absolute-arming path to C.0's conversion inventory as an explicit site
  (raw crtc tag → tokenized `SequenceArm`), naming `trait_def.rs:962/976`,
  `run.rs:1634/1687/1717`, `backend.rs:1521/9192/15891`, and the
  `!present_absolute_vblank_arm_supported` idle fallback at `process_request.rs:9955-9965`.
- Map `absolute_vblank_targets`' per-CRTC target *set* onto `SequenceArm`'s
  `purpose`/`target`, and state the terminal disposition of a live arm when the Present it
  was armed for is scrapped, superseded, or Skip-ed by §A's successor slot.
- State the disposition of `arm_present_completion_idle_vblanks` on `FlipDrivenSoftware`
  and non-supporting drivers, for which §6 already disables QUEUE_SEQUENCE arming.
- Cross §6 with §10.2: state explicitly whether a tagged absolute sequence may release a due
  Present completion while that CRTC's page flip is in flight (the shipped behavior), which
  milestone it establishes, and which it does not — it is clearly not `HardwareComplete`.

## D. Phase B's direct-scanout commit does not satisfy §6.3's evidence matrix, and §12 hides that it must change

`submit_direct_scanout` (`crates/yserver/src/drm/modeset.rs:1581`):

```rust
device.atomic_commit(
    AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK,
    request,
)
```

Against §6.3's **Nonblocking primary Present** row this is deficient on three counts:

1. **No `OUT_FENCE_PTR`.** The row requires *"Successful canonical out-fence status for
   all `ExpectedCompletionCrtcs`"* for `Completed`, and `COMMIT-2` forbids inferring
   `HardwareComplete` from anything else. Only the C.1 async row is exempt, and this is not
   it. §10.2's ownership table (adopt one fd per CRTC; a `-1` holder ⇒ `CompletionUnknown`)
   has no counterpart in the shipped path.
2. **Zero `user_data`.** §10 is explicit: *"The current `drm::Device::atomic_commit`
   wrapper, which leaves user data zero, is not used for live owner submissions unless
   extended equivalently."* Every direct-scanout page event today is correlated by
   `awaiting_outputs` index, not by token.
3. **Plane-only property list.** No cursor, no gamma, no CRTC entries — so §9.2.1's
   *"Every admitted synchronous request includes all compatible newest cursor and gamma
   state"* cannot hold for the highest-rate commit class on the device, and §12's *"Direct
   entry must attach the current cursor state atomically or prove that the already-submitted
   cursor plane state remains valid"* is satisfied today only by the separate **legacy**
   `bind_direct_cursor_on_all_outputs` call (§B).

§12 opens with *"Phase B's primary-plane eligibility and synchronized flip behavior remain
unchanged. C.0 changes only how cursor state reaches KMS and how atomic commits are
ordered."* That is not true: converting this call site to the §6.3 evidence contract changes
Phase B's primary-plane submission itself. §12 needs a paragraph naming `modeset.rs:1581` as
a C.0 conversion site, and §18's merge-boundary argument needs to absorb it — out-fence
adoption on the direct path is not separable from the cursor/gamma conversion, because they
contend for the same single slot.

---

# GAPS AND STALE PREMISES

## E. NVIDIA is policy-gated to software cursor for a measured driver reason C.0 does not answer

The merge kept the policy and only renamed its constructor
(`crates/yserver/src/kms/render/platform.rs:2555`):

```rust
let cursor = KmsCursorState::new_with_nvidia_policy(drm_device_is_nvidia(&device.device));
```

`KmsCursorState::available_on` (`platform.rs:718`) short-circuits on
`!self.nvidia_policy_disabled`, so `cursor_mode()` never returns `Hw` on NVIDIA. Both
direct-scanout gates require it — `scanout_m1_probe_eligible` (`backend.rs:164`) and
`scanout_direct_eligible` (`backend.rs:132`) each take `cursor_hw` as a hard conjunct.
**Fullscreen direct scanout cannot engage on NVIDIA in the current tree.** The merge even
added the counter that proves it: `m1_gate_reject_cursor` (`backend.rs:227`).

This is not an oversight — it is deliberate and known to both parties. M5: the CS2
validation that closed the PR ran *"with nvidia cursor policy disable (only for test, not
in the branch)"*, and the maintainer merged 21 minutes later. So the merged tree's
direct-scanout path is, on NVIDIA, validated-but-unreachable by design.

### The policy is a driver-capability finding, not a legacy-cursor preference

The origin document is `docs/superpowers/plans/2026-07-26-nvidia-cursor-sw-on-drag.md`,
implemented on `perf/nvidia-staging-buffer-pool` more than a month before #129. Measured on
GTX-1050 / nvidia-drm, XFCE Thunar drag:

> *"`CursorPlane::move_to` → legacy `drmModeMoveCursor` **blocks ~11.5ms mean / 16.3ms max
> (~1 vblank), 1200× in one drag, ebusy=0** — cleanly blocking the single-threaded loop on
> every cursor move. Legacy cursor move is µs on amdgpu/Intel but blocks ~1 vblank on
> nvidia-drm."*

`ebusy=0` with a ~1-vblank block is the signature of a driver with **no async plane-update
path**: nvidia-drm does not provide the asynchronous callbacks the kernel's legacy-cursor
fast path requires, so `drmModeMoveCursor` degenerates into an ordinary vblank-paced
synchronous commit inside the kernel. It is not colliding with the scanout flip — it is
simply waiting for vblank, every time.

That is a different failure mode from the amdgpu/Intel one in §2 of the spec (the 2026-05
EBUSY storm), and the same document already recorded the verdict on the obvious fix:

> *"**Why NOT atomic cursor (rejected)** … yserver already tried atomic cursor commits — it
> "screwed up rendering (much slower)" (the atomic-cursor-vs-atomic-pageflip EBUSY storm;
> abandoned bundle-cursor-atomic branch). A nonblock atomic cursor also tends to EBUSY
> behind the scanout flip → vblank-paced cursor. **So atomic is a dead end on this
> codebase.**"*

### Why this is blocker-class for C.0, not a §12 footnote

**My earlier reading was backwards.** I wrote that the policy *"exists because of legacy
cursor behavior on NVIDIA"* and that lifting it was *"the plausible reading"*. It is the
opposite: the policy exists because on nvidia-drm **both** available paths — legacy and
atomic — are vblank-paced, and SW composition was the only one measured smooth. C.0 goal 3
(*"Eliminate legacy cursor and legacy gamma state-changing ioctls … on every device that
reports C.0 ready"*) therefore proposes, for NVIDIA, the exact option a hardware-measured,
codex-reviewed, user-confirmed document already rejected — and the spec never cites that
document. §19 cites `docs/status.md` "HW cursor drag-lag fix" but not
`2026-07-26-nvidia-cursor-sw-on-drag.md`, so §2's problem statement carries only half the
evidence base.

The spec's implicit answer to the 2026-05 failure is sound: the device-local owner plus
§9.2.1 admission plus §9.4 (*"`EBUSY` is a scheduling signal, not a reason to retry-spin"*)
prevents the storm. **That answer does not address the NVIDIA failure mode at all**, because
NVIDIA's measurement was `ebusy=0`. What C.0 *does* offer NVIDIA is different and needs to
be stated as the actual claim:

- §6.3 `COMMIT-5` moves the blocking ioctl into the `KmsIoExecutor` — *"The X11 core never
  executes or waits synchronously for a potentially blocking KMS ioctl"*. The 11.5 ms
  single-threaded-loop stall, which is what the 2026-07-26 drag-stall actually was,
  genuinely goes away.
- It does **not** make the cursor update faster. §4 already concedes above-vblank motion to
  C.2. So C.0's NVIDIA offer is precisely: *hardware cursor plane, non-blocking, vblank-paced*.

**The unanswered question is whether that beats today's SW composited cursor on NVIDIA**,
which the 2026-07-26 document calls *"empirically smooth"* and user-validated.

§16.3's performance table cannot answer it *as written*: it compares C.0 *"with the current
legacy path under the same input/output mode"*, but on NVIDIA the current path is **not**
legacy HW cursor — it is SW composition. So the spec's two-arm shape names the wrong
baseline on this driver.

**The missing arm is obtainable, though — the decision can be made on data, not judgement.**
The policy is the single source-level gate quoted at the top of this section
(`platform.rs:2555`), and `KmsCursorState::new()` (`platform.rs:700`) is already the
policy-off constructor used
everywhere else, so flipping that one call site yields legacy HW cursor on nvidia-drm. That
is exactly what the 2026-08-29 → 2026-08-31 direct-scanout captures did, and it is a
**development-section source edit, not a shipped runtime lever** — so it satisfies M1 (§K)
where an env var would not. All three NVIDIA arms are therefore measurable on the same box.

### Required C.0 changes

1. **§2** must record the nvidia-drm measurement and the "atomic is a dead end" verdict
   alongside the 2026-05 EBUSY history, and **§19** must cite
   `docs/superpowers/plans/2026-07-26-nvidia-cursor-sw-on-drag.md`.
2. **§3 goal 3 / §12** must state explicitly whether C.0-ready includes nvidia-drm, and if
   so, that the claim being made is *non-blocking vblank-paced HW cursor*, justified by
   `COMMIT-5` and not by the EBUSY argument.
3. **§16.3** must make the NVIDIA row a **three-arm** comparison instead of the generic
   two-arm one, all on the 2026-07-26 workload (XFCE/Thunar drag) with drag smoothness and
   input-to-retirement as the gate:

   | Arm | How it is obtained | What it answers |
   | --- | --- | --- |
   | Legacy HW cursor | flip `platform.rs:2555` to `KmsCursorState::new()` | reproduces the 11.5 ms / 16.3 ms block and pins the regression this policy was created to avoid |
   | SW composited cursor | current default, no change | the shipping baseline C.0 must actually beat |
   | C.0 atomic HW cursor | C.0 build | whether `COMMIT-5` off-thread submission converts a vblank-paced commit into an acceptable cursor |

   The legacy arm matters even though it is the rejected option: without it the other two
   numbers have no scale, and it re-verifies on current hardware that the 2026-07-26 finding
   still holds before C.0 argues past it. §16.3 must also state that the flip is a
   development-section source edit and must not become a runtime gate (§K).
4. **§12** must state the prize and the risk together: lifting the policy is what unlocks
   fullscreen direct scanout on NVIDIA at all (both gates require `cursor_hw`), and it is
   simultaneously the case where §B is worst — under M3's submit-at-retirement, an atomic
   cursor commit and a direct successor contend for the single device slot on a driver where
   every commit costs ~1 vblank and has no async escape. §B and §E are the same problem on
   this driver.
5. If the three-arm result says nvidia-drm **stays** SW-cursor, then §16.3's *"run the
   C.0-ready matrix on NVIDIA proprietary and AMDGPU/RADV"* is unsatisfiable as written and
   the C.0→C.1 chain is unreachable on the maintainer's primary test hardware. Say so
   explicitly, re-nominate the validation hardware, and keep the NVIDIA three-arm table as
   the recorded justification rather than deleting the row.

## F. New: `non-desktop` connector filtering is not in the protocol-domain model

`f4433567` is **the maintainer's own commit** (M7, Jos Dehaes, 2026-08-31), so this is
upstream policy for C.0 to accommodate, not a divergence to argue with. It added
`connector_is_non_desktop` (`crates/yserver/src/drm/modeset.rs:266`) and wired it into five
discovery sites:

- `probe_connectors` (`:296`) and `probe_connector_snapshots` (`:324`) — skip
- `discover_outputs` (`:558`) — skip
- `discover_output_for_connector` (`:618`) — returns `PermissionDenied`
- `discover_kms_candidates` (`crates/yserver/src/platform/drm.rs:210`) — a card whose only
  connected connector is `non-desktop` is **not treated as KMS-capable and is never opened**

Three things the spec now needs to say:

1. §6.2 keys `atomic_kms_pipeline_capable` on `(device_identity, protocol_domain)` and
   `CAP-2` lists what may construct a new domain. A connector-class filter is now part of
   what determines the domain's membership; it must be named there, alongside the
   `IdentityChangingHotplug` / `TopologyRebuild` distinction in `REC-6` — a connector
   flipping `non-desktop` at runtime is a domain change, not a mode change.
2. §13's *"Every active CRTC maps to its owning device and a distinct cursor plane"* and its
   "reports C.0 unavailable only for its own outputs" rule now have a device class that is
   never enumerated at all. State whether an unopened non-desktop-only card is `Removed`,
   out of scope, or absent from the model.
3. `connector_is_non_desktop` is deliberately **fail-open** — a `get_properties` failure
   returns `false`. §5 correctly places read-only property discovery outside the commit
   owner, but §6.2's completion-property coverage discovery is fail-**closed**. C.0 should
   state which discipline governs which discovery class rather than leaving two opposite
   defaults undocumented.

## G. Baseline drift in §12 (superset of the 2026-08-28 item 6)

Three Phase B levers the spec's baseline sentence implicitly points at no longer exist
anywhere in `crates/` (verified: zero hits):

- `YSERVER_HW_CURSOR` — deleted. `SceneCompositor::hw_cursor_strategy_enabled` is gone and
  `build_scene` is called with a literal `true` (`crates/yserver/src/kms/render/scene.rs:1235`).
  The hardware-cursor strategy is now unconditional except for the per-device NVIDIA policy
  in §E.
- `YSERVER_PHASE_B_FLIP_VISIBILITY` / `pre95_visibility` — deleted;
  `phase_b_flip_in_flight_for_scheduler` (`backend.rs:156`) is unconditionally
  `scene || direct || unflip`.
- `YSERVER_HW_CURSOR_NVIDIA`, `nvidia_cursor_policy_disabled` (the env lever),
  `direct_fallback_target_materializable` — all deleted.

§12 should pin the current baseline explicitly: hardware-cursor strategy on by default and
device-policy-gated only; direct and composed-unflip transactions always visible to Present
pacing with no runtime gate; no Phase B env levers remain.

Also new and un-inventoried in §15: the merge added nine `m1_gate_*` telemetry counters
(`backend.rs:223-232`) with a per-gate rejection breakdown and an `m1_gates[...]` field in
the per-shape log. §15's telemetry inventory should absorb them, since `m1_gate_reject_cursor`
is precisely the C.0-readiness signal for §E.

## H. Evidence base and references

**§19's `Depends on: PR #129` evidence is unreproducible.**
`docs/handoff-fullscreen-novsync-phase-a-b.md` is now self-contradictory in the merged tree:
line 70 records that *"The direct path was exercised with the opt-in NVIDIA hardware-cursor
validation lever"* — a lever `ff31f14c` deleted — while line 129 states *"NVIDIA retains its
existing software-cursor policy."* Both cannot describe the same run. Line 77's *"Present
supersession absorbed the request flood"* describes the Phase A park/supersede mechanism the
merge then replaced with the backend successor slot (§A).

The spec cannot keep citing *"Phase B direct-scanout implementation and validation in PR
#129"* as supporting evidence without either (a) revalidation on a device that reaches the
direct path under current policy, or (b) an explicit statement that the dependency is on the
**code** in #129 and not on its measurements. Fixing the handoff's two stale claims is a
separate PR-level task, noted only because §19 rests on them.

**§19's C.2 pointer is still broken.** `docs/superpowers/specs/2026-08-27-phase-c2-async-cursor-motion-design.md`
does not exist. §4 and §12 both defer real design decisions to C.2 (the above-vblank cursor
fast path), so the pointer is load-bearing, not decorative.

**Metadata.** `Date: 2026-08-26` predates the merged content. `Depends on: PR #129` should
name `fc76b743`, not the PR — the PR's content changed materially twice after the spec was
written. `Status: Draft — adversarial composition blockers under revision` remains accurate:
§§A, B, D are new blockers of the same class.

## I. §10.4 does not state the idle/complete split that M4 was filed about

M4 is the most performance-load-bearing directive in the set, and it is specifically about
*release timing*: *"another path that **immediately releases superseded async buffers**
while retaining only the newest successor"*, because *"the client exhausts its buffers and
becomes vblank-blocked"* — 200 fps → 60 fps in Warframe.

The merged code implements exactly the required split, in `defer_direct_successor_skip`
(`backend.rs:1845`): the superseded buffer is pushed to `scanout_m2.idled` **immediately**,
with `emit_idle = false` stamped on the event so the later completion cannot idle it twice,
while the `Skip` CompleteNotify is parked in `deferred_successor_skips` and published only
at the predecessor's retirement (`backend.rs:2276`). Its regression test asserts both halves
(*"the coalesced buffer idles immediately and exactly once"* / *"the predecessor completes
before the coalesced successor Skip"*).

The spec never states this pairing. §10.4 says the opposite-sounding thing in general terms:

> *"Present completion does not imply buffer idleness. Pixmap idle notification, release
> syncobj/timeline advancement, dma-buf release, and pin release wait until
> `PriorBufferReleased` or the section 10 teardown barrier proves the buffer unreachable"*

That rule is correct for an **accepted** commit whose buffer the kernel may still be
scanning. It must not be read to cover a **never-submitted** superseded intent, whose buffer
the kernel never saw — and the spec's own first bullet (*"a never-submitted queued victim …
releases pins/wakes immediately"*) is the intended carve-out. But §9.1 says the victim
*"releases its pins/wakes immediately"* while §10.4 is silent on the CompleteNotify
ordering, so a literal implementation could satisfy both by releasing the buffer **and**
firing the Skip immediately — breaking M3's *"preserving Present completion/Idle/Skip
ordering"* — or by deferring both — reintroducing exactly the M4 collapse.

**Required:** §10.4 must state the split explicitly as a normative rule, keyed the way the
section's own ledger already permits (*"keys protocol completion, idle/release completion,
and resource quarantine separately"*):

> A never-submitted primary-plane intent superseded by a newer intent for the same plane
> releases its buffer, pins and wakes at supersession time and emits `IdleNotify` once;
> its `Skip` CompleteNotify is published only after the in-flight predecessor's completion,
> and the idle must not be re-emitted with it.

And §16.2/§16.3 need the matching evidence item, because it is a measured client-visible
regression, not a bookkeeping detail.

## J. §18's single merge boundary conflicts with the maintainer's stated PR policy

§18 declares:

> *"C.0 is its own PR stacked on #129 … Cursor conversion, gamma conversion, and
> device-local commit ownership form one merge boundary"*

and §§6.3/10.2 add to that boundary a process-isolated `KmsIoExecutor` with IPC, fd-passing,
watchdogs, reap supervision, and an `ExecutorStalled`/`ShutdownExecutorStalled` lifecycle.
Against M6 (*"your PRs are huge and not properly stacked"*), that is a merge boundary the
maintainer is unlikely to accept in one piece — and #129, a far smaller change, took four
review rounds and three of his own fix commits over four days.

§18's technical argument is sound as far as it goes (*"switching call sites to atomic
without bounded progress recreates the old cursor failure"*), but it justifies coupling
**cursor conversion to bounded progress**, not coupling either to the executor. The spec
should either (a) split the executor into its own stacked PR ahead of the conversions, with
the conversions landing on legacy-equivalent ordering first, or (b) state explicitly why
the executor cannot be staged separately. As written it asserts a single boundary without
addressing the executor at all, and it does so against a known maintainer preference.

## K. C.0 must record "no gating env vars" as a hard constraint

M1 is a standing policy, not a comment on one PR: *"please remove all these gating env vars,
I know the agents like to do that, but it's just bad practice."* The spec is currently
compliant — a grep for `env`/`YSERVER_`/`opt-in`/`lever` over the spec finds only an
unrelated use of "DPMS toggles" at line 1171 — but compliance by accident is not the same as
a constraint an implementing agent will honour, and C.0 is a large enough surface to invite
exactly the reflex the maintainer named.

Add it to §4 (non-goals) or §3 (goals) verbatim: **no runtime environment gate, opt-in flag,
or rollout lever may be introduced for any C.0 behavior.** Then reconcile it with the places
§16.3 currently implies switchable behavior — *"capability injection for
`DRM_CAP_CRTC_IN_VBLANK_EVENT`"*, *"injected atomic accept/reject"*, *"injected producer,
out-fence, and page-event stalls"*, *"a host-call helper that ignores termination"* — by
stating that every injection point is a `#[cfg(test)]` seam or a test-only backend
implementation, never a production-reachable configuration surface. Phase B's own history
is the precedent: `YSERVER_HW_CURSOR`, `YSERVER_HW_CURSOR_NVIDIA` and
`YSERVER_PHASE_B_FLIP_VISIBILITY` were all introduced as diagnostics and all had to be
deleted before merge (§G).

**The sanctioned third pattern — name it, because C.0 needs it.** Some hardware validation
cannot be a `cfg(test)` seam: it has to run in a real session against a real driver, which is
exactly §E's legacy-HW arm and the 2026-08-29 direct-scanout captures. Both were obtained by
editing a single source gate (`platform.rs:2555`) in a working tree that was never merged.
That is M1-compliant precisely because it does not survive into the shipped binary, and it is
the pattern §16.3 should mandate for every hardware arm that needs behavior the release build
does not expose:

> A hardware-validation arm that cannot be expressed as a `#[cfg(test)]` seam is obtained by
> a documented single-site source edit in the development tree, recorded in the PR evidence
> with the exact diff. It must never be introduced as an environment variable, command-line
> flag, config key, or any other lever reachable from a release build.

Without this sentence the "no env vars" rule and the §16.3 injection matrix read as
contradictory, and the next implementing agent resolves the contradiction the way M1 says
agents always do.

---

# NEW SCOPE — a phase the roadmap does not have

## L. Deadline-scheduled submission is absent from C.0, C.1 and C.2 alike — C.0 must own the seam, a new C.3 must own the policy

**The finding.** The merged direct path dispatches its next flip at the earliest possible
instant of the refresh interval, which is also the instant of maximum display latency. The
trace is unambiguous:

```
vblank N  → kernel page-flip event
          → drain_page_flip_events            (backend.rs:14932)
          → retire_direct_output              (backend.rs:14940)
          → awaiting_outputs empties
          → submit_queued_direct_successor    (backend.rs:2296)
          → submit_direct_scanout
          → atomic_commit(PAGE_FLIP_EVENT | NONBLOCK)   (modeset.rs:1635)
vblank N+1 → that commit latches and scanout begins
```

The commit is issued inside the handler for vblank `N`, so the earliest vblank it can latch
on is `N+1`: **one full refresh period elapses between the frame being chosen and the frame
being seen**, and the choice is made with the maximum possible margin rather than the
minimum necessary one. For a Present arriving at `t` in `(vblank N-1, vblank N]` the age of
the frame when scanout starts is `vblank(N+1) - t`, i.e. in `(T, 2T]` for refresh period
`T`. Latest-wins makes the survivor the newest arrival in that window, so in practice the
age is `T` plus at most one client frame interval:

| Client rate | Refresh | Age at scanout start |
| --- | --- | --- |
| 200 fps | 60 Hz | ~16.7 – 21.7 ms |
| 60 fps | 60 Hz | ~16.7 – 33.3 ms |

**Why §A's latest-wins slot does not solve this, and was never meant to.** M4's failure mode
was *throughput*: unbounded accumulation in core, pins not released, the client starved of
buffers and collapsing from ~200 fps to exactly 60 with `present_skips` at zero. The slot
fixed that by releasing the displaced pin immediately (`defer_direct_successor_skip`,
`backend.rs:1845`). It delivers *the freshest frame available in the window*; it does not
and cannot deliver *the lowest latency*, because the window closes one full refresh before
display. Freshest-available and minimum-latency differ by exactly `T`, and no change to the
slot's replacement policy can recover that term — only moving the dispatch instant can.

The current instant is deliberate, not accidental (`backend.rs:2293-2295`):

> *"Like Xorg's `present_flip_try_ready` and wlroots' `frame_pending` gate, submit at most
> one successor only after the kernel has retired the preceding transaction."*

That is the safe end of a two-sided tradeoff — maximum margin, maximum reliability, maximum
latency. Compositors that are perceived as low-latency under vsync sit at the other end:
Mutter's dynamic max render time and the KWin latency policy both exist to move the
dispatch toward the deadline. Nothing in this review disputes the gate itself (one flip in
flight, resubmit driven by retirement); the finding is only that the *instant* within the
interval is currently fixed at its worst value and no C phase ever revisits it.

**Neither C.1 nor C.2 covers it.**

- **C.1** is `PAGE_FLIP_ASYNC` and visible tearing. Deadline submission applies precisely
  when a flip is *not* async; the two are mutually exclusive per flip. Folding it into C.1
  would make low-latency vsync unreachable for every client that does not set
  `Async`/`AsyncMayTear`, and would couple a change with universal impact to one with narrow
  impact. §E5 additionally warns that if the three-arm result keeps nvidia-drm on software
  cursor then *"the C.0→C.1 chain is unreachable on the maintainer's primary test
  hardware"* — deadline submission is demonstrable on every device that reaches C.0
  readiness, and would be the only primary-plane latency work in the C series that NVIDIA
  users can actually receive.
- **C.2** is above-vblank hardware-cursor motion. Same *shape* of problem — vblank cadence
  imposing latency on a user-visible response — but a different plane, a different commit
  class, and a different fix. It does not generalise to the primary plane.

The roadmap therefore has a hole rather than a misplacement: **no C phase addresses
primary-plane latency for synchronized clients**, which is every client that has not opted
into tearing.

**This is also the resource §B is missing.** §B establishes that under C.0's
single-device-slot rule the retirement-time resubmit is *"a new primary commit taking the
sole device slot, dispatched from the completion path of the commit that just released
it"*, that a fullscreen stream keeps `queued_successor` permanently occupied, and that the
steady state is **unbounded maintenance starvation** for aged cursor and gamma tickets. §B
then notes the obvious fix is unavailable because M3 mandates the submission point, and is
left choosing between absorption (option i) and a stated frame-drop budget (option ii).

Deadline retention changes that arithmetic. If the retained primary intent is dispatched at
a measured margin before the target vblank rather than at the retirement of its predecessor,
the device slot is **free for most of the refresh interval** — which is exactly the window an
aged cursor or gamma ticket needs, and exactly what §16.3's *"input-to-submit p99 must not
exceed one output period plus 2 ms"* gate is asking for. The same mechanism that removes the
latency term also supplies the yield §9.2.1's starvation bound currently has to invent.

This does not make §B's absorption rule unnecessary — a cursor that must move *between*
primary frames still needs either its own slot or a seat in the successor commit. And it
introduces its own tension, which C.3 must own: a maintenance commit still in flight when
the deadline arrives will push the primary past it, converting a latency win into a dropped
frame. Naming that contention is part of C.3's charter, but the seam that makes it
expressible has to exist in C.0.

### Required C.0 changes

1. **§9.1 / §9.2.1 — make the dispatch trigger structural, not the completion callsite.**
   The queued primary-plane intent must be dispatchable from *either* the predecessor's
   completion *or* a timer. C.0 ships the trigger point with an immediate default that is
   behaviourally identical to the merged base; it does not ship a delay. Without this the
   owner's dispatch path, watchdog timing, and terminal-state accounting all bake in
   "dispatch on completion", and C.3 has to reopen precisely the machinery §6.3 and §10 are
   most careful about.
2. **§4 non-goals — reword the scheduling bullet.** *"Present scheduling, async-option
   parsing, or Present capability changes"* is about the core Present layer
   (`present_scheduler.rs`, effective MSC, option bits) and does not on its face exclude
   KMS commit dispatch timing, which is a layer below. But a timer inside the commit owner
   will read as a non-goal violation to any reviewer. State it explicitly: C.0 provides the
   dispatch trigger point and retains the immediate default; **Phase C.3 owns dispatch
   timing policy**; the Present layer is untouched by both.
3. **§6.3 / goal 10 — record the timing substrate.** C.0 already gives every commit an
   unambiguous completion identity and terminal state. Record dispatch timestamp and
   completion timestamp per commit class alongside it. This is near-free given the identity
   model and is the exact input a per-device commit-latency estimator needs; omitting it
   means C.3 opens with a measurement campaign it could have inherited.
4. **§9.2.1 / §10 — define retention-window arbitration now.** A retained intent widens the
   window in which `unflip_requested` can arrive, which today wins over the queued Present
   (`backend.rs:1808-1811`). The owner must define who wins and how ordering is preserved
   when the retention window is non-zero, even while C.0 keeps it at zero. The same applies
   to a topology barrier or recovery tier landing mid-window.
5. **§17 — add a pacing-neutrality acceptance.** C.0's dispatch instant must be unchanged
   from the merged base, asserted as an acceptance criterion. This is what keeps any later
   pacing delta attributable to C.3 (see the risk note below).
6. **§K — pre-empt the obvious M1 violation.** The margin is derived from measurement, never
   configured. No environment variable, flag, or config key for the deadline, the margin, or
   an enable switch, in C.0 or in C.3. This is exactly the reflex M1 names, and a tunable
   millisecond value is the most inviting form of it.

### Phase C.3 charter

**Phase C.3 — deadline-scheduled primary-plane submission (low-latency vsync).**

- **Objective.** Remove the dispatch-to-display refresh period from the synchronized direct
  path by dispatching the retained primary-plane intent at a measured margin before its
  target vblank, instead of at predecessor retirement. Target: reduce Present-to-scanout age
  from `T + client interval` to `margin + client interval`.
- **Depends on:** C.0 — the commit owner, the dispatch seam (change 1), and the timing
  telemetry (change 3).
- **Independent of C.1 and C.2.** May ship before either. The numbering is dependency order
  from C.0, not delivery order.
- **In scope.** The retention timer in the commit owner; a per-device commit-latency
  estimator built on C.0's timing records; margin derivation; missed-deadline detection and
  its fallback; the maintenance-contention rule at the deadline instant (see §B); hardware
  validation including a judder metric.
- **Non-goals.** Tearing (C.1). Cursor latency (C.2). VRR. Any change to the Present layer's
  scheduling, option parsing, or capabilities. Any runtime configuration lever (§K, M1).
- **Acceptance shape.** A measured reduction in Present-to-scanout age at fixed refresh,
  with **no** increase in missed flips or dropped frames against the C.0 baseline, and an
  explicit judder metric rather than a latency metric alone.
- **Risk, and why it must not be folded into C.0.** The failure mode of a deadline is a
  missed deadline: a dropped frame and judder, which is perceptually worse than the latency
  it removes. It needs its own margin tuning per driver and its own validation campaign.
  Folding it into C.0 would make C.0 unshippable until that tuning converges — and C.0 is
  already `Status: Draft — adversarial composition blockers under revision`.

  The attribution argument is decisive and this project has already paid for it once. The
  post-#95 investigation required counterbalanced `post95-1 → pre95 → post95-2` runs on real
  hardware to attribute a *perceived* pacing change, and still concluded that perceived
  quality tracked run order rather than the selector. C.0 already changes the cursor path,
  the gamma path, and commit ordering. Adding the single most pacing-visible change
  available — when flips are handed to the kernel — makes any pacing movement
  unattributable. The handoff for PR #129 defends exactly this separation: keeping the
  follow-up apart means it *"can be reviewed or reverted without losing the safe Phase A/B
  infrastructure"*.

### Open question for the maintainer

M3 specifies the submission point as *"then submit it after retirement"*, and §B treats that
as binding enough that relocating the resubmit into a generic admission pass is ruled out.
Deadline submission still submits **after** retirement — it does not resubmit before the
predecessor retires, and it preserves one flip in flight — but it does not submit
**immediately at** retirement. Whether M3's wording constrains the ordering only, or the
instant as well, is a question for `joske/yserver#129` and not one to resolve by
interpretation. C.0's seam (change 1) is compatible with either answer, since its default is
immediate dispatch; only C.3's policy depends on the reading.

---

# Not affected

The spec's core is orthogonal to all of the above and survives intact: the §5 global
mutation invariant; §6.1 identity model and `ID-1..3`; §6.3's `COMMIT-1..7` and the
`KmsIoExecutor` process-isolation/watchdog/reap model (§J disputes only *when* the executor
lands, never the model itself — and §E depends on `COMMIT-5` being right); §6.4 and
`REC-1..6`; §7 cursor payload; §8 gamma payload and blob lifetime; §11 cursor lifecycle;
§13's cross-device transfer coordinator; the `TEST_ONLY` construction-ordering rule. Legacy `set_gamma`
(`backend.rs:14539`) and the legacy cursor family (`cursor_plane.rs:505/508/530/551`) are
still the only paths in the tree, so C.0's conversion scope for §§7-8 is unchanged by the
merge.
