# Resume point — Phase C.0 stage 2c-i fix round

**Kept current after every accepted session. Last update: 2026-09-12,
after F-4c (`8c552f43`).** If you are resuming this work cold — a new
Claude session, a local model, or a person — this file is the only
context you need to pick the next session; the documents it links hold
the detail.

## Why this file exists

The user's Claude subscription ends **2026-09-28**. Sonnet's 5-hour
session limit has cut long sessions mid-way twice. Every session must
leave the branch coherent, and this file must say exactly where things
stand so nothing is re-derived.

## The documents, in reading order for a cold start

1. `AGENTS.md` — repo rules (clippy `--all-targets -D warnings`,
   `cargo +nightly fmt`, feature branch, squash at the end after asking).
2. `docs/handoff-phase-c0-stage-2c-i.md` — the original handoff; its
   **Rulings R1–R12** are binding.
3. `docs/handoff-phase-c0-stage-2c-i-fix.md` — the fix round: what is
   kept, **rules F1–F8**, the nine sessions F-1..F-9 with their findings.
4. `docs/superpowers/findings/2026-09-11-stage-2c-i-implementation-review-round1.md`
   — the findings (B-n / M-n) every session closes.
5. The per-session reviews (each ends with what the next session must
   do):
   `docs/superpowers/findings/2026-09-11-stage-2c-i-fix-F1-review.md`,
   `…-09-12-…-F2-review.md`, `…-F3-review.md`, `…-F4-review.md`,
   `…-F4b-review.md`, `…-F4c-review.md`.
6. The plan
   `docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md`
   — per-task `Status:` blocks carry every fix-round verdict table.

## Status by session

| Session | Plan task | Status | Code / fold-back | Review |
| --- | --- | --- | --- | --- |
| F-1, F-1b | Task 2 (barrier, registry) | **ACCEPTED** | `95f9dba6`/`53f7ca23`, `13c75265`/`282ed2dc` | F1-review |
| F-2, F-2b | Task 4 (scanout payloads, pool) | **ACCEPTED** | `fea5c043`/`08cb7b91`, `05555b03`/`98328b85` | F2-review |
| F-3, F-3b | Task 3 (storage) | **ACCEPTED** (M-19 deferred) | `12c5926c`/`3677e4b6`, `36ab74a7`/`5c10f7ee` | F3-review |
| F-4, F-4b, F-4c | Task 5 (GPU/read adapters) | **ACCEPTED** (write half → F-4d) | `17384ae6`, `f475b04c`, `4e870930` (+fold-backs) | F4/F4b/F4c-review |
| **F-5** | Task 6 (transport gate at sinks, serviced deadline) | **NEXT** | — | — |
| F-6 | Task 7 (`Terminal` by cause, one keyed release path, 7.6) | pending | — | — |
| F-7 | Task 8 (role transitions, `on_available`) | pending | — | — |
| F-8 | Task 9 (sealed barriers, revocation, 9.5 deterministic half) | pending | — | — |
| F-9 | Task 10 (fixture matrix, 10.2 validation layers, status.md) | pending | — | — |
| F-4d | Task 5 write half (5.3/5.5: managed branch in `scene.rs` `submit_shared_scanout_frame`, `PendingAck` batch, `drain_pending_pool_releases`) | after F-9 | — | ruled in F4c-review |
| F-10..F-12 | M-19 (180 `Storage` Deref accessor sites → lease accessors; 3 sessions: read-mostly consumers → `engine.rs` → `backend.rs`) + M-20 promotion half + F3-M1/F3-m1 | after F-4d | — | ruled in F3-review |

Between sessions the coordinating reviewer (Opus) runs an adversarial
review with **mutation checks** on the decisive assertions (delete the
mechanism, the test must fail) and writes `…-Fn-review.md`; a session is
not accepted until that passes. Then the next session is dispatched.

## Rulings taken during the round (binding, not in the handoffs)

- **F5 amendment** (F2-review, F2-m2): `Option<Arc<VkContext>>` is
  permitted when the only `None` constructor is `#[cfg(test)]` and every
  non-test constructor takes `Arc<VkContext>` by value.
- **M-19 deferred to F-10..F-12** (F3-review): nothing in F-4..F-9
  depends on it; the three files are Jos's hottest merge surface; done
  last, before the squash.
- **`PermissiveDump`** (F4b-review): the live-scene fixture opens a real
  primary node **without master** (matched to the Vulkan device by
  `VK_EXT_physical_device_drm`; this box is multi-GPU), so no bo is ever
  `OnScreen`; the decisive read test selects with `PermissiveDump` and
  asserts the phase. No `#[cfg(test)]` phase override, ever.
- **Managed bos are read/written through the payload under a lease**
  (F4b-B1): after `register_managed_scanout_bo` the pool `ScanoutBo` is
  a husk; `with_scanout_read`/`with_scanout_write` are the accessors.
- **F-4d** (F4c-review): the managed *write* branch is its own session
  after F-9.
- Hardware tests: `_vulkan`/`_drm` suffix, `#[ignore = "..."]`, and a
  missing device is `panic!`, never `return` (R12 — F-3 got this wrong
  and redid it).

## Cold-start recipe for the next session

Dispatch the implementer (Sonnet, or whatever is available) with a
prompt of this shape — the F-4c/F-4b prompts in the coordinating
session are the models; the essentials are:

1. Worktree `/home/ariel_santangelo/Projects/yserver-phase-b`, branch
   `feat/phase-c0-atomic-kms-migration`, may commit, must not push /
   squash / rebase / stash, no subagents.
2. Read order: `AGENTS.md` → fix handoff (F1–F8; R1–R12 binding) → the
   previous session's review (its last section names residuals) → the
   findings items for this session → the plan's contracts (lines 15–90)
   and **this task only** → invoke `superpowers:executing-plans` and
   `superpowers:test-driven-development`.
3. Scope: the fix handoff's "F-n — Task m" section, verbatim, plus the
   residuals the previous review carried in.
4. Gate before every commit: `cargo +nightly fmt`; `cargo clippy
   --all-targets -- -D warnings`; `cargo test -p yserver --lib c0_2ci`;
   the 12-run flake loop; `cargo test -p yserver --lib c0_2ci --
   --ignored` (all hardware tests pass on this box); full `cargo test -p
   yserver --lib` once (R2's three executor flakes excepted). Tasks
   touching `drm/`, `drm_cleanup.rs` or `transport.rs` also check the
   gnu/musl/freebsd targets.
5. Two commits: code (`fix(kms): …` subject from the handoff) and
   fold-back (`docs(plan): fold back session F-n's fix-round findings
   into stage 2c-i`), `Co-Authored-By: Claude Sonnet 5
   <noreply@anthropic.com>`, no session URLs.
6. Fold-back shape: `git show 98328b85` / `git show 3677e4b6` — a
   `**Fix round k: <sha>**` heading under the task's `Status:` line, a
   per-finding verdict table (`RESOLVED (test: …)` / `NOT APPLICABLE` /
   `DEFERRED TO …` / F8 stop), gate output including the hardware run,
   steps ticked only with the test that proves each.
7. F8: if a step cannot be done honestly in the session, stop and report
   the exact split; never tick over missing code.

Then review: read the diff, run the gate, mutate the decisive assertion,
write `…-Fn-review.md`, commit it, update this file's table, dispatch
the next.

## Upstream notes for the maintainer (not ours to fix)

- `root_get_image_reads_scanout_pixels_not_root_storage` and
  `root_overlay_xor_pass_reaches_scanout` (`backend.rs`, `#[ignore]`d)
  were never live: the fixture always failed `PRIME_FD_TO_HANDLE` with
  `ENOTTY` on `Device::for_tests()`'s socket and hid it with
  `eprintln`+`return`. With the fixture now on a real node they fail
  honestly at `OnScreenOnly` selection (no master → no flip).
- Two merges of `origin/master` are on the branch (`3033fa8a` glyphs,
  `3183d8ca` v1.5.1); both clean, clippy and suite green.
