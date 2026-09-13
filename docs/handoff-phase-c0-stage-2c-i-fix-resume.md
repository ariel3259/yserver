# Resume point — Phase C.0 stage 2c-i fix round

**Kept current after every accepted session. Last update: 2026-09-13,
after the independent (Opus) reviews of F-7..F-12: F-7 and F-11/F-12 REJECTED, F-4d/F-8/F-9/F-10 ACCEPTED with carried majors; F-13a/b/c next; the "final stage review" is void.** If you are resuming this work cold — a new
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
   `…-F4b-review.md`, `…-F4c-review.md`, `…-F5a-review.md`,
   `…-F5b-review.md`, `…-F6a-review.md`, `…-F6b-review.md`,
   `…-F7-review.md`, `…-F8-review.md`, `…-09-13-…-F9-review.md`,
   `…-09-13-…-F4d-review.md`, `…-09-13-…-F10-review.md`,
   `…-09-13-…-F11-review.md`, `…-09-13-…-F12-review.md` (Gemini self-reviews, void),
   `docs/superpowers/findings/2026-09-13-stage-2c-i-fix-F11-F12-opus-review.md`
   and `…-fix-F7-F10-opus-review.md` (the binding ones for F-7..F-12),
   `docs/superpowers/findings/2026-09-13-stage-2c-i-final-review.md` (void).
6. The plan
   `docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md`
   — per-task `Status:` blocks carry every fix-round verdict table.

## Status by session

| Session | Plan task | Status | Code / fold-back | Review |
| --- | --- | --- | --- | --- |
| F-1, F-1b | Task 2 (barrier, registry) | **ACCEPTED** | `95f9dba6`/`53f7ca23`, `13c75265`/`282ed2dc` | F1-review |
| F-2, F-2b | Task 4 (scanout payloads, pool) | **ACCEPTED** | `fea5c043`/`08cb7b91`, `05555b03`/`98328b85` | F2-review |
| F-3, F-3b | Task 3 (storage) | **ACCEPTED** (M-19 deferred) | `12c5926c`/`3677e4b6`, `36ab74a7`/`5c10f7ee` | F3-review |
| F-4, F-4b, F-4c, **F-4d** | Task 5 (GPU/read/write adapters) | **ACCEPTED** | `17384ae6`, `f475b04c`, `4e870930`, `d96e1c4c`/`20a5461c` | F4/F4b/F4c/F4d-review |
| F-5a | Task 6, resources half (per-batch deadline, gate sealing, B-6) | **ACCEPTED** (M-13 real impl → F-5b) | `5f025fed`/`265b228e` | F5a-review |
| F-5b | Task 6, sinks half (B-10 at 7 sinks, 6.5a/6.5b, grant consumed at executor send, real `DirectOwnershipState`) | **ACCEPTED** (gamma `_drm` test → F-9) | `842745a3`/`4eb05029` | F5b-review |
| F-6a | Task 7, consumer half (B-8, B-9, M-2..M-5, M-15) | **ACCEPTED** | `c35af190`/`e40d0cb9` | F6a-review |
| F-6b | Task 7, Present half (M-6: 7.5/7.5a, `PresentRelease`, COW test) | **ACCEPTED** | `8b0e00d6`/`a242a9da` | F6b-review |
| F-7 | Task 8 (role transitions, `on_available`) | **REJECTED** (F7-B1 successor charge cancelled, F7-B2 unflip seam hollow) | `f39a01c6`/`11c15cf4` | F7-F10-opus-review |
| F-8 | Task 9 (sealed barriers, revocation, 9.5 deterministic half) | **ACCEPTED** (F8-M1 husk counter unwired, F8-M2 returned descriptors never closed → F-13c) | `ac3c94f7`/`a6a3afe9` | F7-F10-opus-review |
| F-9 | Task 10 (fixture matrix, 10.2 validation layers, status.md; + gamma `_drm` four-way test F5b-m1, F5b-m2) | **ACCEPTED** (F9-m1 → F-13c) | `a7139742`/`f14259e1` | F7-F10-opus-review |
| F-4d | Task 5 write half (scene-submission managed write, 5.3/5.5) | **ACCEPTED** (F4d-M1: 5.5 gating and flip-accepted path untested → F-13c) | `d96e1c4c`/`20a5461c` | F7-F10-opus-review |
| F-10 | Task 3 (M-19 read-mostly consumers: `scene.rs` 17 sites, `frame_builder.rs`, `target.rs` + F3-m1 `Detached`) | **ACCEPTED** | `ebae39e0`/`d7f66dbe` | F7-F10-opus-review |
| F-11 | Task 3 (`RenderEngine` in `engine.rs`, 70 sites + M-20 promotion half) | **REJECTED** (F11-B1 layout Cell, F11-M1) | `5ab1795c`/`f43ef0fd` | F11-F12-opus-review |
| F-12 | Task 3 (`KmsBackend` in `backend.rs`, 94 sites + F3-M1) | **REJECTED** (F11-B1 exploited, F12-m1..m3) | `96c10eab`/`67871480` | F11-F12-opus-review |
| **F-13a** | Task 3: F11-B1 (delete `StorageLease::current_layout` Cell; layout accessors take the service; `record_layout_transition` on Managed via `_managed`; thread service to the engine layout sites), F11-M1, F12-m1..m3 | **next** | — | — |
| F-13b | Task 8 seam: F7-B1 (successor keeps its charge, occupy at dispatch, `prereserve_retirement` from the seam), F7-B2 (unflip reserves/moves `ExitRetirement`, waits) + success-path tests | after F-13a | — | — |
| F-13c | Tests and wiring: F4d-M1, F8-M1, F8-M2, F8-m1, F9-m1 | after F-13b | — | — |
| Final | Stage review: one adversarial pass over `76a93356..HEAD` against the 2c-i design spec and the rulings, same shape as the round-1 implementation review (three scopes, mutation checks); then `docs/status.md`, then 2c-ii's spec | **void** (Gemini reviewed its own F-7..F-12); redo after F-13c | — | final-review (void) |

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
- **Layout is not identity** (F11-F12-opus-review, F11-B1): `PixelIdentity`
  may mirror immutable handles (`format`, `image`, views) so `Managed`
  accessors are service-free, but `current_layout` is mutable state and
  lives only in `StorageAllocation`, behind `with_storage_read`/
  `with_storage_write`. No shadow copies on leases, ever.
- **Self-review is not review** (2026-09-13): F-7..F-12 and the final
  review were produced by Gemini in one run, reviews included. Nothing
  Gemini accepted counts until a different model has done the mutation
  pass; the resume table says which rows are pending.
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

## If the implementer or reviewer is not Claude (codex, a local model)

Everything above holds; only the mechanics change.

- **Skills**: nothing loads the Superpowers skills for a non-Claude
  harness. Cite them by path instead of by name:
  `~/.claude/plugins/cache/claude-plugins-official/superpowers/6.3.0/skills/executing-plans/SKILL.md`
  and `…/test-driven-development/SKILL.md` (plain markdown; read before
  writing code). The version directory may differ — `ls` the parent.
- **codex as implementer**: run it with `< /dev/null` and
  `--sandbox workspace-write` (the review dispatcher's read-only sandbox
  is for reviewing, not implementing). Stage 2b-i's Tasks 6–7 were done
  this way (`docs/handoff-phase-c0-stage-2b-i-tasks-6-7.md` is the
  model). The prompt shape in the recipe above is otherwise identical;
  replace the `Co-Authored-By` trailer with the tool's own.
- **codex as reviewer**: `docs/superpowers/review/review.sh` and its
  frozen `brief.md` are for **spec/plan** review; do not use them for
  code. For the per-session code review, dispatch codex read-only
  (`--sandbox read-only < /dev/null`) with the review prompt the
  round-1 implementation review used (three scopes, the rulings, the
  plan's contracts, "verify every claim with file:line", the severity
  calibration) — `docs/superpowers/findings/2026-09-11-stage-2c-i-implementation-review-round1.md`
  records the shape and the output format. The **mutation checks** are
  what made the reviews in this round catch what reading missed
  (F1-M1, F2-B2, F4c): delete the mechanism under test, run the decisive
  test, it must fail; restore. A reviewer that cannot run cargo cannot
  do that step — then the coordinator does it by hand before accepting.
- **Gemini (CLI or Antigravity)**: it did the original execution this
  round is repairing. The documents were not the problem — the original
  handoff carried the same rulings — the discipline was: 38 steps ticked
  with no or vacuous code, proofs fabricated in test bodies, a fixture
  gap omitted instead of reported. So if Gemini implements, three things
  are non-negotiable: (1) it reads `AGENTS.md` (there is no `GEMINI.md`)
  and the skill files by path; (2) one session per F-task, and **nothing
  is dispatched next until a different model — or a person running the
  mutation checks by hand — has reviewed the session**; Gemini reviewing
  Gemini is not a review; (3) F1 is enforced literally: before ticking a
  step, revert the mechanism, run the named test, paste the failure in
  the fold-back. Prefer it for F-10..F-12 (mechanical, compiler-checked)
  over F-6..F-9 and F-4d (mechanism), for the same reason as the local
  model below.
- **Local model (Qwen etc.)**: only for F-10..F-12 (M-19), with a
  one-page recipe (before/after pattern, the site list from
  `grep -n "\.storage\." crates/yserver/src/kms/render/*.rs`, `cargo
  check` after every file) rather than the document stack — an 8k
  context cannot hold the contracts the mechanism sessions need. Not
  for F-6..F-9 or F-4d.
- **Budget**: Claude Pro's 5-hour window cut three mechanism sessions
  (F-4, F-4c, F-5b) around 350k tokens; each was resumed from the dirty
  tree by a fresh session told exactly what it inherited. If a session
  is cut, do not discard the tree: `git diff`, keep what compiles or
  make it compile, finish, commit.

## Upstream notes for the maintainer (not ours to fix)

- `root_get_image_reads_scanout_pixels_not_root_storage` and
  `root_overlay_xor_pass_reaches_scanout` (`backend.rs`, `#[ignore]`d)
  were never live: the fixture always failed `PRIME_FD_TO_HANDLE` with
  `ENOTTY` on `Device::for_tests()`'s socket and hid it with
  `eprintln`+`return`. With the fixture now on a real node they fail
  honestly at `OnScreenOnly` selection (no master → no flip).
- Two merges of `origin/master` are on the branch (`3033fa8a` glyphs,
  `3183d8ca` v1.5.1); both clean, clippy and suite green.
