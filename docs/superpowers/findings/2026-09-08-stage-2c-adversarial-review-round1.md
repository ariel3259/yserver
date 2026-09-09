## Verdict

**2 blocking, 2 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `13637318`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Reviewed source:** `d4c30877`. **Reported usage:** 77,154 tokens for this external pass;
not a total for the author session. Log: `/tmp/yserver-stage2c-review-round1.log`.
Input SHA-256: main design `fdbb97c2de50fc64637a753b278505b13062998e3239b64b55be166ff4668da6`;
resource design `7689e684e2dcdef7f6f2adcae185ab5329bc9717b2e7c115e02bb03b4aca7045`.
The findings below refer to those original document versions/line numbers.

This is a design-review result only. It does not establish compilation, test success, implementation readiness, or approval to activate the converted paths.

## Incorporation audit

| Prior finding | Status |
|---|---|
| None | No prior adversarial review exists; check 1 was skipped as directed. |

## Findings

### Blocking

#### B-1 — Quarantined lease ownership has no complete teardown handoff

The companion requires leases to outlive ordinary backend/store containers and says stage 3 must transfer them into “teardown supervision,” but it does not define the receiving owner, the atomic handoff boundary, or how deferred cleanup remains serviceable after the backend service boundary disappears ([resource design lines 71–101](docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L71), especially 87–101). These questions are explicitly left for another pass ([lines 214–220](docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L214)).

Concrete sequence: an accepted commit becomes unknown; both allocation sets and external-ownership obligations enter quarantine. Shutdown then destroys the ordinary store/platform containers. A later lease drop either calls unavailable backend cleanup, silently loses the cleanup obligation, or destroys an allocation without the GPU-idle/teardown proof. The authoritative spec requires the entire uncertain record and all external-ownership ledgers to remain quarantined ([authoritative spec lines 2127–2132](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md#L2127)).

Smallest correction: define the stage interface now—a persistent teardown supervisor that owns allocation contexts and cleanup obligations, the exact move-by-value handoff performed before container destruction, and the barrier/evidence required before it may release each resource. Stage 3 may implement the barrier, but 2c-i must define what it hands over and ensure no destructor bypass exists.

#### B-2 — Delayed direct-resource retirement lacks a bounded capacity invariant

The design promises bounded physical resources but only refers to “existing source-specific acquisition limits” and leaves their mapping unresolved ([resource design lines 161–183](docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L161), [219–220](docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L219)). The inspected baseline direct framebuffer cache is an uncapped `HashMap`, with removal/clear as its only demonstrated lifetime controls ([backend.rs lines 270–317](crates/yserver/src/kms/render/backend.rs#L270)). It therefore does not supply the claimed bound.

Concrete sequence: commit B replaces direct source A and frees the atomic slot at `CompletionRetired`, while A remains retained pending its independent release/FOREIGN dependency. B is then replaced by C, and so on. Without admission tied to the number of current, submitted, and delayed-release leases, physical imports and strong device contexts can accumulate even though only one successor and one atomic slot exist. This violates the authoritative requirement that late `PriorBufferReleased` state remain in an existing **bounded** resource-retirement ledger ([authoritative spec lines 2098–2114](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md#L2098)).

Smallest correction: specify a finite per-source/per-device lease capacity and make acquisition/admission fail or remain logically desired when all capacity is current, submitted, quarantined, or awaiting release. Also define how the non-supersedable unflip obtains a release-safe buffer without exceeding that bound.

### Major

#### M-1 — Deferred Skip metadata remains a structurally unbounded memory sink

The design correctly separates released framebuffers and pins from deferred protocol completion, but explicitly retains an unbounded `Vec` of distinct Skip records and declares total Present memory unbounded ([resource design lines 185–197](docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L185)); the baseline representation confirms that vector ([backend.rs lines 341–353](crates/yserver/src/kms/render/backend.rs#L341)).

Concrete sequence: while one predecessor awaits retirement, a client repeatedly replaces the sole successor. Each victim releases its physical resources but adds permanent metadata until the predecessor terminalizes. Processing requests returns ingress byte credits, so those credits do not bound the accumulated records. A sufficiently fast or stalled stream can exhaust server memory before the completion watchdog resolves it. Preserving every ordered Skip is normatively required ([authoritative spec lines 1423–1428](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md#L1423)); user acceptance of that behavior does not establish memory safety.

Smallest correction: treat this as an activation prerequisite and define an Xorg-compatible bounded-progress mechanism—such as request backpressure that preserves every notification and FIFO order—rather than claiming the bounded-intent model is operationally safe while this sink remains unrestricted.

#### M-2 — Exclusive production-route activation is an unresolved architectural contract

The target correctly states that `LegacyDrained` does not exclude future legacy writers, but defers the actual selection boundary and writer inventory to later planning ([target lines 195–208](docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md#L195), [241–242](docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md#L241)).

Concrete sequence: the converted primary route observes drained legacy events and starts an owner commit; an unchanged lifecycle or maintenance caller subsequently issues a legacy ioctl on the same device. Both mutate KMS state despite the nominal single owner. The authoritative staging boundary requires later transports to enter the already-established single admission/completion/quarantine model ([authoritative spec lines 3856–3880](docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md#L3856)).

Smallest correction: define one per-device route state owned by the platform, its one-way transition protocol, and the writer classes gated by it. Stage 3/4 may add producers, but the exclusion interface and the rule preventing re-entry after unknown must be fixed in 2c.

## Coverage and implementation checks

- **Incorporation:** skipped; no prior review.
- **Architecture/contracts:** reviewed block dependencies, event/resource consumers, grouped retirement, admission, activation, and deferred-stage interfaces.
- **Safety/ownership:** reviewed independent completion/release, quarantine, destructor context, capacity, supersession, teardown, and late-resource ownership.
- **Specification/verification:** checked §§9.1–9.2.1, 10.2–10.4, 12–12.1, and 18. Proposed damage milestone and current-master per-output tests are materially aligned; no separate damage finding was established.

Used **12/12 bounded excerpts** beyond the target: companion design 3, authoritative spec 8, baseline source 1. No further source adapters, lifecycle writers, or cleanup implementations were inspected; they are unverified, not deemed sound. Builds, formatting, clippy, portability checks, and executable test behavior remain deferred to implementation.

## Local verification and disposition — 2026-09-08

This section is the author's follow-up, not a replacement reviewer verdict.
The original counts and coverage above are preserved. No second external pass
was run. The reviewer used all 12 excerpts, with only one baseline code excerpt;
its declared complete coverage does not certify the uninspected implementations.

| ID | Local verification | Disposition |
| --- | --- | --- |
| B-1 | Confirmed a missing concrete receiver/handoff contract. The draft already forbade early destruction and production activation, so the reported unsafe sequence is a risk if the handoff remains unspecified, not a demonstrated runtime bug in the current tree. C.0 §10 requires retained supervision and separate reap/fd/shared-resource barriers. | Expanded 2c-i with a process-lifetime supervisor receiver, move-by-value bundle contents, consumer/late-reply routing, no old-backend callbacks and proof-gated cleanup. Locally addressed at contract level; adapter realization and external reassessment remain pending. |
| B-2 | Confirmed. `ScanoutM1ProbeCache.entries` is a `HashMap` with no count bound; `CompletionRetired` can release the atomic slot independently of old-resource release. The documents have not established a finite direct-import retirement invariant. This is distinct from notification credits. | OPEN. Define the physical direct-resource bound and release-safe unflip progress before the 2c-i plan; no arbitrary numeric budget has been introduced. |
| M-1 | Confirmed inherited metadata growth: each superseded frame appends a distinct event to `deferred_successor_skips`; source pins are released independently. Existing ingress byte credits do not bound completed dispatch obligations. The reviewer did not establish an Xorg-compatible replacement policy. | ACKNOWLEDGED, POLICY CHANGE NOT APPLIED. The user explicitly approved preserving Present semantics without new protocol credits. Keep the limitation visible; a new protocol cap/backpressure activation prerequisite requires a separate compatibility decision. No bounded-total-memory claim is made. |
| M-2 | Confirmed the activation interface was underspecified. Existing `try_finish_legacy_transport`/`finish_legacy_transport` provides drain handover, not a universal gate on future legacy writes. Current production remains legacy, so the finding describes a future activation hazard. | Expanded the main design with per-device Legacy/Quiescing/Owner/Closed transport authority, revocation-before-drain, gated writer classes, receiver requirements and no same-incarnation legacy re-entry after unknown. Locally addressed at contract level; full call-site inventory and external reassessment remain pending. |

The damage contracts received no separate finding in this pass. This does not
close the previously documented cross-output capture/ack validation question.

Remaining decisions are the direct physical retirement bound (B-2), the accepted
metadata-growth tradeoff versus a separately designed protocol policy (M-1), and
concrete adapter/call-site evidence for the two local contract corrections.
Do not report a clean review or implementation readiness from this follow-up.

## Blocking corrections — 2026-09-09

Following user authorization to resolve the blocking findings, the resource
design now specifies the following. This is a new local disposition; the
reviewer's original counts and the dated 2026-09-08 follow-up remain historical.

- **B-1, locally addressed in design:** the teardown bundle has a pre-established
  process-lifetime receiver and late-reply route. Allocation contexts no longer
  require an untracked `Rc<Device>` to survive: the fd registry owns closable
  DRM access. Normal cleanup consumes rights once; quarantine freezes them,
  and complete-family closure discharges file-owned handles without subsequent
  destructor ioctls. GPU/FOREIGN/shared cleanup remains independently gated.
  The existing unconditional probe-framebuffer destructor is explicitly named
  as an adapter conversion, not assumed safe unchanged.
- **B-2, locally addressed in design:** six named positions bound direct frame
  resources per supported ownership unit, including one preparing candidate
  and separate ordinary/exit retirements. Normal direct replacement reserves
  ordinary retirement before dispatch; delayed release stops further normal
  replacements. The exit position and retained composed return path permit
  unflip without waiting for that normal slot. Re-entry waits for both retires;
  failed cleanup closes transport instead of allocating overflow. Converted
  probe-cache indices own no imports outside the role table.

The design includes explicit failure/ordering/capacity test scenarios for both
corrections. No implementation tests or second adversarial review have run for
these changes. M-1's compatibility decision is unchanged; M-2 retains its prior
local transport-contract correction. Concrete adapter implementation and an
authorized scoped reassessment must not be represented as already complete.
