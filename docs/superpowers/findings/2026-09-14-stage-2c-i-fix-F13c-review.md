# Stage 2c-i fix round — session F-13c (tests and wiring) review

## Verdict — ACCEPTED; two minors carried to 2c-ii, not to another fix session

Reviewed `cecec1f5..526b0085` (`c0918b22` code, `526b0085` fold-back).
Implementer: Sonnet, in two parts — a first session wrote the whole diff and
was cut by the account's **weekly** cap before running the gate or committing
anything; a second session inherited the dirty tree, verified it, answered the
two open questions, ran the gate and committed. Reviewer: Opus (this document).

All six residuals close:

- **F4d-M1** — four new scene-level tests drive the four `is_releasable` gates
  F-4d added, each against a real `ResourceService`/`AllocationKey` through a
  new `managed_pool_release_fixture()` (F3 kept: no `ResourceService` mock).
  A registered `Gpu` obligation stands in for not-yet-releasable and
  `service.cancel` for discharge.
- **F8-M1** — the husk counter is driven by the real sites:
  `register_managed_scanout_bo` (`platform.rs:5887`) registers once the display
  half's lease is committed; `detach_managed_entries` (`scanout.rs:780`, now
  taking `Option<&mut DrmCleanupRegistry>`) unregisters per display-pool
  `ScanoutBo` whose `take_managed()` returned `Some`.
- **F8-M2** — `HandoffRouter::service` closes returned descriptors once
  `helper_reaped()`; both tests were reworked to observe the router's own
  teardown instead of hand-calling `close_returned_descriptors`.
- **F8-m1** — new `UnknownCause::GrantRevokedInFlight`, used by
  `quarantine_live`; the existing test's `matches!` tightened to it.
- **F9-m1** — `VkContext::validation_layer_active()` (distinct from
  `debug_messenger.is_some()`) plus an environmental-skip guard, so the
  zero-validation-message assertions can no longer pass vacuously.
- **F13a-m1** — `Storage::set_current_layout` now proven at the accessor level:
  `Err(Busy)` under a live reader with the payload unchanged, and a
  `retain_storage` twin observing a write made through the drawable.

## Mutation checks (this reviewer, independent of the implementer's)

I did not re-run the implementer's mutations; I chose my own, one of them
deliberately more central than any of theirs.

1. **`ResourceService::is_releasable` → unconditional `true`** (one mutation at
   the service, not at the four call sites): **5 tests fail** — all four new
   `c0_2ci_scene_*_vulkan` plus the pre-existing
   `c0_2ci_scene_managed_shared_compose_vulkan`. This is the decisive result:
   F4d-M1's complaint was that this exact class of mutation left the whole
   suite green at 134/134. ✔
2. **`HandoffRouter::service`'s close gated off** (`if false && …`):
   `c0_2ci_handoff_complete_fd_family_barrier_deterministic` and
   `c0_2ci_adapter_unknown_detach_late_reply_reap` fail. ✔
3. **`unregister_pool_husk()` removed from the Shared arm** of
   `detach_managed_entries`:
   `c0_2ci_scanout_managed_pool_husk_blocks_family_barrier_until_detached_vulkan`
   fails (barrier stays refused after detach). ✔
4. **`register_pool_husk()` removed** from `register_managed_scanout_bo`: the
   same test fails the other way (barrier mints while the husk is live). ✔
5. **`validation_layer_active: false` forced**: `c0_2ci_live_lifetime_adapters_vulkan`
   fails with the environmental-skip panic instead of passing vacuously. ✔

Every mutation was reverted and the tree confirmed clean (`git status`) before
the gate below.

## The two coordinator-flagged open questions — both answered, both verified here

**The `Copied` pool's `sources` loop is correct as written.** Verified
independently, not taken from the implementer's report: `CopiedRenderSource`
(`scanout.rs:861`) and `CopiedRenderSourceBacking` hold no `drm` field at all —
`render_vk`/`sink_vk` are `Arc<VkContext>`, everything else is Vulkan handles —
so a source's managed lease was never counted as a husk and there is nothing to
unregister. `register_pool_husk` has exactly one non-test caller, for the
display half. Separately verified: `take_managed()` has **no** call sites outside
`detach_managed_entries`, so the invariant *registered husk ⟺ display bo holding
a managed lease* has no third path that could break it.

**The `_vulkan` `#[ignore]`d test shape is right, and my finding was wrong to
prescribe otherwise.** `SceneCompositor::new` takes `platform.vk().ok_or(NoVk)?`
and builds real Vulkan objects (`CompositorPipeline::new`,
`CompositePoolRing::new` → `create_descriptor_pool` against a live `ash::Device`);
no stub constructor exists for `SceneCompositor`, `OutputSceneState` or
`CompositePoolRing`, and all four gates take `&mut OutputSceneState`. A
deterministic version would have required new scene-level mock infrastructure
out of all proportion to the finding. **F4d-M1's prescription of a test shape I
had not verified was reachable is a defect in the finding, not in the session** —
see the ruling below.

## Minors carried to 2c-ii (not worth another fix session)

**F13c-m1 — `Option<&mut DrmCleanupRegistry>` makes skipping the accounting a
legal, silent call, and the production route already takes that arm.**
`drain_scanout_pool_at` (`platform.rs:6321`, reached from suspend/drain) calls
`detach_managed_entries(None)`. Today that is genuinely inert and the call site
says so honestly: registering a husk requires a registry, which only tests have
(R8), so no production bo is ever managed and `take_managed()` returns `None`
there anyway. But the moment 2c-ii/2c-iii makes the managed route
production-active, that same call drops managed leases with no decrement, the
counter never returns to zero, and **the fd-family barrier can never mint again**
— failing closed, silently, far from this code. The accounting should be
impossible to skip rather than optional: have `detach_managed_entries` return the
number of managed leases it dropped and make the caller account for them, or
require the registry outright. **2c-ii's spec must pick this up.**

**F13c-m2 — `unregister_pool_husk`'s `saturating_sub(1)` absorbs underflow
silently.** With a bare unkeyed counter there is no way to notice a
double-unregister or an unregister with nothing registered; combined with m1 the
counter has no self-check in either direction. A `debug_assert!` on underflow
costs nothing and would have surfaced m1's future failure at the source.

Neither is a defect in what F-13c was asked to do: the counter shape predates
this session (F-8 built it, F-13c wired it), and the `None` arm is documented
rather than hidden.

## Gate (this box, re-run by me after restoring from every mutation)

`cargo +nightly fmt --check` clean · `cargo clippy --all-targets -- -D warnings`
clean · `cargo test -p yserver --lib c0_2ci` **124 passed / 0 failed / 18
ignored**, twelve consecutive runs, **zero flakes** · `-- --ignored` **18 passed
/ 0 failed** · full `cargo test -p yserver --lib` **1662 passed / 0 failed / 90
ignored** (R2's three executor flakes did not fire in any run). The fold-back
additionally records `cargo check` clean on gnu/musl/freebsd.

## Ruling taken at this review — findings state invariants, not edits

Both open questions above trace back to the same authoring mistake in **my own**
F7-F10 review, and the round has now paid for it twice:

- F8-M1 named the two sites to wire ("`register_managed_scanout_bo` has the
  registry in hand; `detach_managed_entries` needs it"). The implementer wired
  exactly those two. **The enumeration became the scope boundary** — which is
  why the `sources` loop had to be raised as a separate question at review time
  instead of being covered by the finding, and why F13c-m1's `None` arm was
  never in scope at all.
- F4d-M1 prescribed a test shape I had not verified was reachable. The
  implementer hit the wall and **substituted silently** rather than reporting.

From here, in this round's remaining work and in 2c-ii's spec:

1. A finding states **the invariant that must hold and the mutation that must
   break a named test**. Site and shape are the implementer's call — they have
   the code in front of them.
2. Rule F8 (honesty) extends to test shapes: a suggested shape that turns out
   unreachable **must be reported**. The silent substitution is the defect, not
   the deviation.
3. Never name call sites unless verified in the file; when naming them, mark
   them explicitly as "at least these", never as a closed list.

## What comes next

The **final stage review** — one adversarial pass over `76a93356..HEAD` against
the 2c-i design spec and R1–R12, three scopes with mutation checks. The Gemini
one (`2026-09-13-stage-2c-i-final-review.md`) stays void. With no calendar
deadline in play any more, it is split **by scope, one session each**, per the
measured plan-size rule — not attempted in one pass. Then `docs/status.md`, then
2c-ii's spec, which must carry F13b-D1, F13c-m1 and F13c-m2.
