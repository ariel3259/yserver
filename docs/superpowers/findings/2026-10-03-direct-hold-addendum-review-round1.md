# Direct-hold Owner addendum — design review round 1

**Result:** 0 blocking, 3 major, 0 minor; coverage INCOMPLETE (24/24 excerpts).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`;
model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Coordinator verification:** M-1 verified (`direct_scanout_topology_eligible`
requires one device and the grouped framebuffer is installed on every output;
`has_current_direct` is a global role boolean). M-2 verified (VT acquire
reinstall calls `stop_direct_after_scanout_replaced` without Ciii,
admission.rs ~7393). M-3 accepted as a verification-strategy gap.
Classification: M-1 and M-2 design; M-3 test design. Addressed in addendum
revision 2.

## Verdict

0 blocking, 3 major, 0 minor

Coverage: INCOMPLETE

This is a design-review result. It does not establish compilation, test success, or implementation approval.

## Incorporation audit

| Prior findings | Disposition |
|---|---|
| None; this is the first review. | Check 1 skipped as instructed. |

## Findings

### Blocking

None demonstrated within the inspected scope.

### Major

**M-1 — The protected CRTC set needs an authoritative definition**

[Plan lines 51–65](docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md#L51) impose a per-CRTC hold and invariant, but do not define how admission obtains the complete protected set.

This matters because the direct frame’s Present CRTC is its **pacing identity**, while the existing grouped path installs the framebuffer on every output of the eligible device. The distinction is explicit in [backend.rs lines 4754–4785](crates/yserver/src/kms/render/backend.rs#L4754). The unflip producer likewise constructs the complete device CRTC group in [unflip_owner.rs lines 24–65](crates/yserver/src/kms/render/unflip_owner.rs#L24). Admission’s existing `current_direct` query is merely a global resource-role boolean ([admission.rs lines 9841–9850](crates/yserver/src/kms/render/admission.rs#L9841)); it supplies neither pending-frame coverage nor CRTC membership.

Concrete failure: a grouped frame covers CRTCs A and B but is paced on A. A gate derived from the candidate’s CRTC protects A while allowing a composed primary on B, reproducing the resource/M2 divergence there.

The smallest correction is to define one device- and topology-qualified protected set covering every output referenced by pending/current direct ownership, and use it consistently for readiness, bundles, maintenance absorption, and invariant checks. For the existing grouped path, explicitly distinguish this set from the Present pacing CRTC.

Also state that direct ownership of only one CRTC within a multi-CRTC device is presently refused unless a separate bounded ownership representation is defined. The authoritative spec requires the same complete output coverage and forbids reusing the successor slot for different output ownership ([spec lines 1110–1115](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md#L1110)).

**M-2 — The unflip-only rule lacks lifecycle and quarantine exceptions**

[Plan lines 57–65](docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md#L57) say that, once requested, Ciii is the only replacement and that M2-current implies a matching `current_resources` entry except during an in-flight unflip. Those statements do not accommodate lifecycle supersession.

Concrete sequence: a window release requests an unflip; shadow materialization is still waiting; VT release supersedes it. The old incarnation can no longer perform the normal unflip. The lifecycle arbiter must terminalize/quarantine outstanding work and later perform a fresh install. The spec gives VT release precedence and requires this handoff ([spec lines 633–676](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md#L633)). Existing Owner VT acquire promotion retires device-current resources and clears M2 after reinstall, without Ciii retirement ([admission.rs lines 7393–7396](crates/yserver/src/kms/render/admission.rs#L7393)).

Consequently, the absolute rule conflicts with an existing required replacement path. Waiting for Ciii before allowing lifecycle progress would create a dependency on work that cannot run after seat loss.

Restrict “unflip is the only replacement” to ordinary primary replacement on a live, healthy incarnation. Explicitly preserve owner-ordered disable/reinstall and proven teardown or quarantine handoffs. Define how the invariant applies during those transitions, without treating logical withdrawal as proof that scanout stopped. These exceptions must retain the spec’s resource-proof requirements, not authorize early release.

**M-3 — The deterministic mutations lack a required ready-composed precondition**

The new damage test promises that removing rule 1 deterministically dispatches a composed commit; the unflip test similarly mutates admission order ([plan lines 80–89](docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md#L80)). Neither specifies how a genuinely ready composed generation reaches admission.

Damage alone does not establish that precondition. The production composite tick returns while direct is pending or held ([backend.rs lines 28610–28619](crates/yserver/src/kms/render/backend.rs#L28610)), and the spec says held outputs bypass painting composed buffers ([spec lines 2150–2156](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md#L2150)). Admission separately checks the composed producer’s readiness ([admission.rs lines 8313–8325](crates/yserver/src/kms/render/admission.rs#L8313)).

Concrete false-negative test: establish a held direct frame, damage its background, then drive retirement admission. No composed offer becomes ready because composition was suppressed upstream. Removing rule 1 still dispatches nothing, so the mutation survives.

Require the fixture to establish a retained, production-produced ready composed offer before the direct retirement wake, and verify its underlying readiness independently of the new hold gate. Then require the mutation to cross the actual dispatch boundary.

The verification contract should also explicitly exercise:

- pending direct ownership and a queued successor;
- the complete multi-CRTC protected set;
- a requested unflip whose shadow initially fails, followed by a production retry;
- maintenance and lifecycle progress while ordinary composition is blocked.

The unflip decider checks readiness before selecting its priority tier ([decide.rs lines 19–29](crates/yserver/src/kms/owner/admission/decide.rs#L19)); a successful ready-unflip test does not establish behavior while that barrier waits. Likewise, checking rule 4 only at test end ([plan lines 90–91](docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md#L90)) cannot establish its every-iteration claim. Use the existing driver’s iteration observation facilities, followed by the required end-state check. The spec requires evidence for normative contracts ([spec lines 2350–2354](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md#L2350)).

### Minor

None.

## Coverage and implementation checks

- **Check 1:** skipped; no prior review.
- **Check 2:** inspected readiness production, retirement routing, grouped direct coverage, unflip preparation/retry, scheduler priority, and VT reinstall integration.
- **Check 3:** inspected frame/resource retirement correlation, queued-successor cancellation, lifecycle supersession requirements, and the prohibition on release without replacement proof. No demonstrated memory or ABI defect.
- **Check 4:** assessed damage, scheduling, lifecycle and multi-output requirements against the proposed evidence. The existing unmap test uses production import/window-release entries and the core driver. New-test readiness setup and mutation reachability remain underspecified.

**Excerpts used: 24/24**, excluding the target-plan read and bounded locator searches. Investigation stopped at the limit.

Unassessed questions are the exact direct-resource disposition across DPMS off/on and quarantine, and whether every maintenance-absorption path excludes protected composed primaries while preserving maintenance progress. Their correctness is not established here.

No builds, tests, installs, or compilation experiments ran. Implementation retains the required `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`, applicable portability checks, A/B test discipline, suites and mutations under CPU load (F), and coordinator hardware runs (C).