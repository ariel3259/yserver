## Verdict

3 blocking, 5 major, 0 minor

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `38aee673`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification:** B-2/B-3 are genuine baseline/spec mismatches: the current
target-split alias compiles but does not satisfy the explicit replacement rule;
purpose-tagged raw tokens are unique but not monotonically increasing from one.
B-1 identifies an overbroad qualification claim; revision 3 narrows the exported
fact to completion-mechanism evidence, leaving full structural/incarnation/ready
conjunction to 2c. The capability table distinguishes structural capability from
incarnation qualification, so not every structural input named in B-1 belongs to
the latter, but the reduced gate still must not claim either complete bit.
M-1 through M-5 verified against source. Revision 3 addresses all eight; the next
review must check the changed contract and the two baseline prerequisite repairs.

## Incorporation audit

| Prior finding | Status | Audit |
|---|---|---|
| B-1 — QUEUE_SEQUENCE bypasses executor | APPLIED | Task 3 adds protocol-v3 `SequenceQueue` request/reply variants, a queue lease, asynchronous dispatch, helper-only ioctl execution, and early-event staging (`plan:173-203`, `524-535`). |
| B-2 — probe rejection stub emits wrong family | APPLIED | `RejectProbeWith(i32)` is explicitly required to emit `ProbeRejected`; `RejectWith` remains a deliberate malformed-family test (`plan:493`). |
| B-3 — qualification possible without GET_SEQUENCE | APPLIED | Install/restore clock rows cover every expected-completion CRTC and require current successful GET references at dispatch and completion (`plan:240`, `369-373`). |
| B-4 — blocking qualification unrepresentable | APPLIED | `CompletionContext` now carries `host_class` and `allow_modeset`; lifecycle blocking classes, validation options, and `ALLOW_MODESET` request construction are specified (`plan:209-242`). |
| B-5 — `before_block` after timeout calculation | APPLIED | Task 7 explicitly moves `before_block` before timeout calculation and adds the shortened-deadline regression (`plan:601`). |
| B-6 — undefined `LegacyDrainPermit` | PARTIAL | Task 1 now produces `LegacyDrainPermit`, `LegacyDrained`, `new_legacy`, and `finish_legacy_transport` (`plan:375-385`, `458`). However, the checked proof issuer and the error used when the permit refuses C.0 APIs remain unspecified; see M-4. |
| M-1 — capability evidence lacks production wiring | PARTIAL | The revision names `CompletionCaps`, discovery, installation, and Task 7 construction wiring (`plan:371-373`, `601`). The declared source cannot provide “actual successful atomic enablement,” and the discovery signature is not implementable as written; see M-1. |
| M-2 — no raw-parser failure ingress | APPLIED | Task 4 produces `report_stream_failure`, and Task 7 preserves valid-prefix events before calling it (`plan:259`, `599`, `602`). |
| M-3 — sequence identity/consumer APIs undefined | APPLIED | Exact owner and backend signatures, consumer IDs, coverage meaning, and cancellation semantics are now specified (`plan:395-448`, `528-534`). |
| M-4 — missing requirement traceability | APPLIED | Every task heading carries requirement groups, and the plan states that they apply to every named test/case and must be copied into test documentation (`plan:450-452`, `454-610`). |
| M-5 — missing workspace locked test | APPLIED | `cargo test --all-targets --locked` is present in the final gate (`plan:615-624`). |
| M-6 — missing normalization boundaries | APPLIED | The maximum `tv_sec`/`tv_usec` case and full `fffe → ffff → 0 → 1` sequence are present, with client-visible projection required in Task 7 (`plan:462-477`, `603`). |
| M-7 — page-before-reply misclassified | APPLIED | Task 8 now distinguishes valid page-before-success staging from page-before-rejection contradiction (`plan:612`). |
| m-1 — false `ClockEpochId` derive claim | APPLIED | The revised plan correctly says it already derives `Ord` and `PartialOrd` (`plan:98`). |
| m-2 — incomplete `ProbeAccepted` field claim | APPLIED | The baseline table now includes sequence plus helper and round-trip durations (`plan:51`). |

## Findings

### Blocking

#### B-1 — `CompletionCaps` can mark an incarnation qualified without the spec’s required structural capability

The proposed qualification gate checks only atomic enablement, the two event capabilities, `OUT_FENCE_PTR` coverage, and GET readiness (`plan:369-373`). It contains no evidence for cursor-plane coverage, a structurally available coordinate transport, required primary properties, or homogeneous multi-CRTC membership.

The authoritative capability contract requires all of those inputs before structural capability or incarnation qualification can become true (`spec:431-435`). The multi-CRTC requirement is reiterated at `spec:3049-3051`. Nevertheless, the plan changes `Qualification` to `Qualified` solely when its reduced-capability candidate completes (`plan:369-371`).

The legacy permit prevents this state from becoming ordinary production readiness during 2b-ii, but it does not make the state truthful: fixtures following Task 6 will assert `Qualified` from evidence that the spec says is insufficient. Either this state must be explicitly completion-mechanism-only, or the missing structural evidence must be included before the `Qualified` transition.

#### B-2 — The plan explicitly preserves and uses an ioctl request alias the spec requires removed

The canonical fence wrapper is required to import and use the existing `platform::ioctl::{IoctlReq, iowr}` boundary (`plan:282`, `295-316`). That existing alias is:

- `libc::Ioctl` on Linux (`crates/yserver/src/platform/ioctl.rs:12-13`);
- `libc::c_ulong` elsewhere (`crates/yserver/src/platform/ioctl.rs:14-15`).

The spec identifies precisely that representation and says it must be removed or replaced; merely leaving it in place does not satisfy the requirement (`spec:1677-1685`). The acceptance criteria likewise require the baseline alias to be absent (`spec:3036-3045`, `3749-3752`).

An engineer following the shown `query_status` code necessarily leaves the forbidden alias alive and adds another consumer of it. The final structural searches also contain no check that it disappeared (`plan:638-649`).

#### B-3 — Preserving purpose-tagged token values violates the normative monotonic token namespace

The plan instructs Task 3 to retain the existing purpose tags while allocating commit-event and sequence-arm tokens from one allocator (`plan:145`). The existing allocator encodes the type in the top two bits and seeds its counter from the incarnation (`crates/yserver/src/kms/owner/identity.rs:164-180`, `192-200`, `238-259`).

Consequently:

- the first raw event token is not 1;
- alternating event and sequence allocations are not numerically increasing, because the top-bit purpose changes;
- a sequence token followed by an event token decreases as a raw `u64`.

The spec requires the owner to allocate tokens from the shared namespace “in increasing order starting at one” (`spec:1663-1672`). It requires type resolution through the owner record, not through non-monotonic raw-value partitions (`spec:392`, `1665-1669`). Preserving the current tagged representation therefore implements a directly contradictory identity contract.

### Major

#### M-1 — Production cap discovery still has no concrete source for `atomic_enabled`

The plan says discovery must use “actual successful atomic enablement” and introduces `PlatformBackend::discover_completion_caps(device_key) -> io::Result<CompletionCaps>` (`plan:373`). No receiver or device argument is specified, even though a device key alone cannot issue the capability queries.

More importantly, the current atomic-capability setup discards exactly the evidence the new field needs: `Device::enable_atomic_capabilities` logs failures and still returns `Ok(())` (`crates/yserver/src/drm/device.rs:88-106`, `109-131`). `Device` stores no atomic-enabled result (`device.rs:9-13`). Task 6 does not declare a new `Device` field/accessor or a changed open return contract.

Thus the claimed correction of prior M-1 is overstated. An implementer must invent both the discovery receiver/data path and the meaning of `atomic_enabled`, or will incorrectly treat successful device construction as successful atomic enablement.

#### M-2 — Immediate fence success is required to unregister an fd that was never registered

The state algorithm says newly adopted fds are queried immediately and only `Pending` descriptors enter the poll set (`plan:320-322`). It then says that on `Success`, the implementation must unregister before closing (`plan:322`).

Task 5 makes the contradiction observable: deterministic tests inject immediate `Success` while using a real `CompletionPoller`, and the exact-close test requires the transferred writer to close “only after successful status and unregister” (`plan:558-561`). A newly adopted immediately-successful fd was never registered, so the existing `CompletionPoller::unregister` performs `epoll_ctl DEL`/`EV_DELETE` on a missing registration and returns an error (`crates/yserver/src/kms/render/completion_poller.rs:95-121`). Under the plan, that error is itself a platform failure.

The contract must say “unregister iff `registered`,” and the immediate-success test must assert close without an unregister. The unregister-before-close assertion belongs to a descriptor that first returned `Pending` and was registered.

#### M-3 — The stated queue-reply byte order contradicts the existing probe layout it claims to preserve

The protocol text says the last 16 reply bytes are “sequence-or-errno/padding plus helper duration,” while also saying this matches the existing probe layouts (`plan:191-197`). The existing wire order is the reverse:

- probe accepted writes `helper_duration_ns` first, then `sequence` (`crates/yserver/src/kms/executor/protocol.rs:838-845`);
- probe rejected writes `helper_duration_ns` first, then `errno` and padding (`protocol.rs:846-853`);
- the decoder reads helper duration first (`protocol.rs:868-905`).

Because no exact queue-reply offsets or golden bytes are stated, an implementer can reasonably follow either the prose field order or “matching probe layouts.” Encoder/decoder round trips would not expose a consistently reversed implementation. The plan must prescribe exact offsets, preferably duration at payload offsets 60–67 and sequence/error at 68–75 to preserve the current layout.

#### M-4 — The legacy handover proof and refusal contract remain underspecified

Task 1 claims to produce `LegacyDrainPermit`, `LegacyDrained`, `new_legacy`, and `finish_legacy_transport`, while Task 7 claims to implement a checked platform proof issuer (`plan:377-379`, `458`, `602`). But no issuer signature is declared, no construction visibility is specified, and no interface explains how the owner module receives proof of platform-only facts such as empty scene/direct flips and an EAGAIN-complete drain.

Likewise, all C.0 begin/validation/probe calls must reject while the permit exists (`plan:377`), but neither the cross-task signature contract nor `DispatchError` names a legacy-mode refusal variant (`plan:395-428`, `520`, `528`). The current enum has no suitable variant (`crates/yserver/src/kms/owner/device.rs:60-81`).

This leaves Task 1 and Task 7 engineers to invent incompatible proof and error APIs at the safety boundary that is supposed to prevent legacy/C.0 coexistence.

#### M-5 — Consumer cancellation misses the shutdown removal path

Task 3 enumerates cancellation sites and tells implementers to cancel before removals in named execution, purge, supersession, drain, sweep, stale-event, and disconnect paths (`plan:533-534`). It does not include `shutdown_drain_present_pending_exec`, which removes the entire execution-stage store with `mem::take` (`crates/yserver-core/src/core_loop/process_request.rs:10514-10545`).

Those entries use `present_id` as their sequence consumer under the plan. Dropping the store without calling `cancel_present_sequence_consumer` leaves logical consumers attached to owner arms during shutdown/reap. The spec requires immediate consumer removal whenever parked Present work is removed and forbids consumerless arms from updating clocks (`spec:1795-1801`, `3085-3089`).

The broad instruction to “reconcile every pending-store removal with rg” does not repair the explicit cancellation inventory. This shutdown function must be named and tested.

### Minor

None.

## Notes on the rest

I completed all five requested checks:

1. **Incorporation audit:** Audited all 15 round-one findings against the revised task text. Eleven blocking/major findings and both minor findings are applied; B-6 and M-1 remain partial as detailed above.
2. **Existing-code verification:** Opened every explicit baseline anchor. The anchors at `backend.rs:1044`, `backend.rs:1514-1558`, `platform.rs:4150`, `platform.rs:4243`, and `run.rs:1251` are accurate. I also verified the current owner constructor, slot model, executor phase checks, protocol layouts, capability setup, event parser, completion poller, Present stores, and all six remaining DRM atomic call sites.
3. **Cross-task consistency:** Checked production and test-only types, visibility, consumer signatures, owner entry points, leases, capability installation, raw-error ingress, and cancellation paths. Apart from M-1, M-4, and M-5, the declared task handoffs are consistent. The public-hidden fixture strategy is sufficient for the external integration crate.
4. **Compilation/test review:** Read every shown code block as Rust. The clock arithmetic, UST maximum, deadline calculations, `SyncFileInfo` layout, request code, and probe example are type-correct. The immediate-success fence test is internally impossible as written (M-2); no other shown test block has an evident Rust type or borrow error.
5. **Spec compliance:** Checked the executor containment, identity, clock probing, sequence arms, event staging, canonical fences, milestones, qualification, deadlines, and portability invariants. The three direct violations are B-1 through B-3. The asynchronous GET/QUEUE model, full correlation, early-event staging, independent fence/page evidence, 64 commit tombstones, fail-closed FreeBSD runtime behavior, and final three-target compilation gates otherwise match the cited normative contract.
