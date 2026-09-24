# Stage 3b design — codex review, round 4

**Target:** `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
revision 4 (`59d9793e`), prior review round 3.

**Result:** 2 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24; copied-route
source/sink ownership and the rejected-commit release path unassessed).
Trend: r1 2B 3M, r2 2B, r3 1B 1M, r4 2B 1M.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):**

- **B-1 — CONFIRMED.** Revision 3 had position-only changes stage a new scene
  state. Revision 5 updates the existing state in place; nothing is retired.
- **B-2 — CONFIRMED.** An inactive-to-inactive CRTC has no out-fence (C.0 §6.3).
  Revision 5 adds the typed `DarkCrtcDisplacement` proof (a proven off plus an
  owner-recorded chain of inactive commits) and a fallback to the lighting
  commit or the device barrier when the chain is unproven; listed for the user.
- **M-1 — CONFIRMED.** `retire_owner_displaced` (`scene.rs:5062`) is
  index-addressed. Revision 5 moves the replaced scene state and the old pool
  into one detached retired-output bundle addressed by identity; the copied
  route's source-side resources travel in it (`OutputScanout::Copied`).
- **Unassessed rejected-commit release path:** a rejected commit's prepared set
  is released once (section 6); its old pool was never displaced.

## Verdict

**2 blocking, 1 major, 0 minor.** Coverage: **INCOMPLETE**. This is a design-review result; it does not establish that the design compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 3 B-1 — Legacy call delays gate expiry | **APPLIED** | Revision 4 services queued deadlines before the next admission and adds Legacy execution time `L` to the mixed-server bound ([3b:569](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:569)). The duration of Legacy’s blocking calls is outside this review’s scope. |
| Round 3 M-1 — disabled pool release proof | **PARTIAL** | The design now registers `KmsRelease` against a displacing commit ([3b:326](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:326)). It does not supply release evidence for a mode changed while the CRTC stays dark (B-2). |
| Round 2 B-1 — scene resource handoff | **PARTIAL** | The replaced scene is retained, but position-only replacement and retirement after output removal still lack safe ownership contracts (B-1, M-1). |
| Round 2 B-2 — validation at gate expiry | **APPLIED** | Expiry performs only stateless checks ([3b:549](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:549)). |
| Round 1 B-1 — atomic `EBUSY` retry | **APPLIED** | No retry; readiness closes ([3b:432](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:432)). |
| Round 1 B-2 — whole-request bound | **APPLIED** | Probe, prerequisite, unflip, commit and gate waits have stated deadlines, subject to the declared Legacy `L` allowance ([3b:530](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:530)). |
| Round 1 M-1 — position-only request | **TRADED** | It now avoids a KMS commit, but replacing its scene state has no corresponding resource handoff (B-1). |
| Round 1 M-2 — fallible promotion | **APPLIED** | Scene construction precedes acceptance; promotion moves staged data ([3b:278](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:278)). |
| Round 1 M-3 — requester-less publication | **APPLIED** | The event reaches the arbiter immediately; publication uses the gate ([3b:597](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:597)). |

## Findings

### Blocking

**B-1 — Position-only promotion replaces a live scene without displacing its buffers.** A position-only request stages a new `OutputSceneState` and promotes it without a KMS commit or pool change ([3b:190](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:190), [3b:278](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:278)). Promotion swaps in that state, but names retirement of the old state only for a *mode change or disable* ([3b:317](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:317)). If the old state holds an unsignalled descriptor fence or a current composed buffer, dropping it releases ownership early; retaining it under the mode-change rule cannot obtain a displacement completion because no commit occurred. Define position-only promotion as an update that preserves the existing scene’s ring, pending releases and buffer ownership, or specify an equivalent transfer. Test it with a pending fence and a current buffer.

**B-2 — A dark-to-dark mode change has no fence to discharge the old pool’s `KmsRelease`.** DPMS-off retains the primary framebuffer while setting `ACTIVE=0` ([3a:164](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:164)). A client can then install another mode and framebuffer while keeping `ACTIVE=0` ([3b:173](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:173)). C.0 excludes an inactive-to-inactive CRTC from `ExpectedCompletionCrtcs` and forbids adding an out-fence to it ([C.0:550](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:550), [C.0:588](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:588)). The proposed rule nevertheless discharges the old allocation’s KMS obligation at that commit’s `CompletionRetired` ([3b:326](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:326)). No completion fence proves the release in this sequence: immediate discharge can free too early, while waiting for a nonexistent fence retains pools indefinitely. C.0 keeps buffer release distinct from acceptance and hardware completion ([C.0:2121](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2121)). Name a resource-appropriate later proof and its owner, and test repeated mode changes under DPMS-off.

### Major

**M-1 — A retired scene loses its positional route to an output removed by disable.** Disable removes the target from `platform.outputs` while moving its old scene to a retirement list ([3b:306](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:306), [3b:359](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:359)). The existing retirement path takes an `output_idx` and calls `platform.leave_owner_buffer(output_idx, …)` ([scene.rs:5062](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:5062)). If removing A shifts B into A’s index before A’s pending resources retire, that route addresses B or no output. The design specifies `OutputKey` reassociation for *kept* scenes, but not a stable access path for the removed scene ([3b:377](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:377)). Give retired scenes a detached, identity-stable resource route and test disable with an unsignalled release followed by an index shift.

### Minor

None.

## Coverage and implementation checks

**24/24 bounded spec/source excerpts used; no builds or tests run.** Incorporation covered the two round 3 findings and their carried findings. Architecture checked the gate, lifecycle result boundary and scene integration. Safety checked clock gating, commit evidence, pool release and scene retirement. Compliance checked the six RANDR obligations against the umbrella and the relevant C.0 rules.

Coverage is incomplete because the excerpt limit left the copied route’s full source/sink ownership and the complete rejected-commit release path unassessed; neither is judged sound. A focused follow-up should establish who retains and releases source-side copied resources after a sink output changes or disappears. Compilation, real tests, `cargo +nightly fmt`, CI’s `cargo clippy --all-targets -- -D warnings`, and applicable portability checks belong to implementation.