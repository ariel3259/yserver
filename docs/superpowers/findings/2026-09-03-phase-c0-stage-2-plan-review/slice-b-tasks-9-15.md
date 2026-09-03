# Stage 2 plan review — slice B, tasks 9-15

Raw output of `codex exec --sandbox read-only`, 2026-09-03. Scope: the clock
probe and sequence normalization, completion deadlines, the qualification gate,
bounded intents and admission tickets, the seven admission tiers, and Present
terminalization.

## Blocking

### B-1. Task 9 cannot legally dispatch a clock probe through the Stage 1 executor

Stage 1’s executor requires `SubmittingProof` for every dispatch. Task 6 says its production constructor is available only to `KmsDeviceOwner::submit`, immediately after installing a `CommitRecord` (plan lines 1513–1539, 1590). Task 9 instead says the probe “owns no commit resources and never touches the atomic slot” but still calls the same executor path (lines 2052–2054). It therefore has no legal way to construct the required proof.

This also fails to encode the host-call serialization required by spec lines 1737–1750. A clock probe must not race another owner-mediated KMS host call merely because it is not an atomic transaction.

Introduce an owner-held typed probe/host-call reservation, installed before IPC, that can produce the executor proof without fabricating a `CommitRecord`. Specify whether it blocks atomic admission, validation, coordinate transport, and other probes until the reply or helper reap.

### B-2. Task 12 qualifies the incarnation with the wrong commit

The spec requires:

> “the first required real install/restore commit whose `ExpectedCompletionCrtcs` is non-empty is the qualification commit” (spec lines 2041–2046).

Task 12 instead intentionally uses “the first converted primary commit after the existing `commit_modeset` path” until Stage 3 (plan lines 2443–2447). Its transition is generic: any completed record with a nonempty expected set opens readiness (line 2443). Task 6 makes this worse by admitting every commit class while `Unqualified`, because its ordinary-class branch includes `admits_qualification_commit()` (lines 1491–1496).

A cursor, ordinary primary, or other non-install commit could therefore open readiness after an unowned legacy modeset. That is explicitly not the qualification defined by §10.1. `CommitClass::BlockingQualification` also cannot represent the spec’s permitted nonblocking qualification commit.

Make “is this exact required install/restore operation?” an explicit, orthogonal record property. Only that record may be admitted while unqualified and only its complete canonical fence evidence may open readiness. Do not defer the real gate to Stage 3 while claiming Stage 2 readiness.

The interface statement that `ClockUnresolved` and `AdmissionClosed` are “the only refusals” (line 2379) also contradicts Task 6’s `SlotBusy` and `Construction` errors (lines 1346–1348).

### B-3. Task 14’s `AdmissionContext` cannot encode the seven normative tiers

`AdmissionContext` contains only a homogeneous CRTC set, an externally supplied `owed_crtc`, and:

```rust
fn(MaintenanceIdentity, u32) -> bool
```

(plan lines 2596–2602). That predicate cannot inspect or prove the final atomic closure, primary generation, maintenance generation, changed-versus-unchanged state, snapshot currency, synchronous class, canonical completion coverage, or request construction compatibility required by spec lines 1508–1524 and 1534–1559.

Tier-by-tier:

1. Tier 1 is represented correctly by a topology barrier and has a meaningful priority test (lines 2607–2614).
2. Tier 2 is not fully representable. `AdmissionChoice::Recovery` exists, but Task 13 exposes no recovery-intent storage or offer API, and Task 14 has no software-cursor recovery test.
3. Tier 3 implements the round-robin rule backwards. The plan requires the successor CRTC to be “not the one currently owed” (line 2741). The spec says it is eligible when its CRTC is permitted, and is blocked when a *different* CRTC is owed (spec lines 1496–1498, 1514–1517). The absorption predicate also cannot prove the “full persistent request” or snapshot compatibility.
4. Tier 4’s enum is unimplementable as written: line 2622 matches `AdmissionChoice::Maintenance(id)` as a tuple variant, while lines 2678–2680 match the same variant as a struct variant. The plan also has no primary age/order metadata with which to absorb “the oldest such primary” required by spec lines 1542–1547.
5. Tier 5 has no representation of canonical completion coverage or cross-generation request compatibility. The prose says “at least two” CRTCs are included (line 2743) but does not require every ready CRTC in the qualified group, as spec lines 1549–1557 do.
6. Tier 6 cannot choose the “oldest ready primary replacement” because Task 13 gives primary intents no admission timestamp or comparable age. Round-robin state is caller-supplied and never advanced by `select`.
7. Tier 7’s oldest-ticket selection is expressible, but its symmetric primary absorption has the same missing primary order and compatibility data as tier 4.

Replace the Boolean function pointer with a typed, generation-specific admission candidate produced from the final request builder, including closure, synchronous class, completion coverage, and exact consumed generations. The scheduler must own and update its primary round-robin state.

### B-4. The claimed primary round-robin guarantee is delegated to the test

The test manually sets:

```rust
ctx.owed_crtc = Some(...)
```

between selections (plan lines 2721–2726). It proves only that `select` obeys a correctly prepared mock context. Nothing in Task 14 updates `owed_crtc` after an admission, so production can repeatedly pass `None` or stale state and admit the same CRTC indefinitely.

This fails:

> “a continuously ready primary CRTC may not take two successive device slots while another CRTC has a ready primary intent” (spec lines 1582–1586).

Store the round-robin cursor inside `AdmissionState`/`KmsDeviceOwner`, update it atomically for singular and bundled admissions, and test repeated owner retirement/admission without manually editing scheduler state.

### B-5. Maintenance arriving behind an in-flight commit is not aged

The spec explicitly says:

> “A cursor or gamma intent is also aged when it arrives behind an already submitted commit. After that commit completes, no primary that cannot absorb the aged generation may take the device slot while its ticket exists.” (spec lines 1570–1574)

`offer_maintenance` always creates a non-aged intent (plan lines 2484–2489, 2566–2568). Task 14 ages maintenance only after `select` has already admitted a higher-priority item (line 2747). Thus maintenance arriving during a commit can lose to another ordinary primary after retirement, exceeding the stated bound.

Pass the owner’s in-flight state into `offer_maintenance`, or provide a distinct “offered behind submitted commit” operation that assigns the ticket already aged. Add a test that offers maintenance during an accepted commit, retires it, and verifies that a nonabsorbing primary cannot win next.

### B-6. Replacing or rejecting a direct successor loses the resources needed for terminalization

`offer_direct_successor` and `offer_barrier` return only:

```rust
Displaced {
    idle_now: Option<PresentSerial>,
    deferred_skip: Option<PresentSerial>,
}
```

(plan lines 2477–2479). The displaced `DirectIntent`, including its buffer, pins, wake, client/window identity, and other cleanup data, has already been removed. A caller cannot perform the release required by spec lines 1423–1428 and 2218–2222 from a serial alone.

A direct intent offered while a barrier is pending is even worse: it returns `None` and is not stored (lines 2544–2546, 2570), making rejection indistinguishable from successful insertion with no displacement. That incoming Present is neither released nor terminalized.

Return ownership of the complete displaced/rejected intent, or a linear cleanup token containing every resource and protocol identity. Use a result enum that distinguishes `Inserted`, `Replaced(old)`, and `RejectedByBarrier(incoming)`.

### B-7. The lifecycle deadline implementation contradicts the validation rule and can panic

The spec says lifecycle timing with missing evidence, an unrepresentable calculation, or a healthy observation above the 28-second margin leaves the cohort unvalidated (spec lines 2177–2183).

The plan instead saturates and clamps:

```rust
observed_max.saturating_add(Duration::from_secs(2))
```

and explicitly expects a 40-second observation to return a 30-second deadline (plan lines 2263–2268, 2338–2341). That silently validates a cohort the spec requires to remain unvalidated.

The fast and Present calculations use `period * 3` and `period * 2` (lines 2333–2346), which panic on `Duration` overflow. Deadline construction with `Instant + Duration` has the same unchecked edge unless `CommitDeadlines` uses `checked_add`.

Return `Result`/`Option` from lifecycle deadline calculation, reject observations above 28 seconds, and use checked multiplication and checked `Instant` addition. Specify the non-poisoning “cohort unvalidated” disposition separately from live completion timeout.

### B-8. Task 15 fabricates clock values forbidden by §10.4

The spec says an accepted Present without `Presented` completes as `Skip` using the last validated clock sample and “never fabricates a new MSC/UST” (spec lines 2223–2225).

The plan deliberately returns:

```rust
Skip { msc: 0, ust_us: 0 }
```

when no sample exists (plan lines 2830–2837, 2873). Zero is still an invented MSC/UST pair and can be a valid real clock value.

Define the no-sample protocol disposition explicitly—normally suppressing the notification while still unblocking the per-client FIFO, or carrying an explicit `NoValidatedClock` result for the caller. Do not serialize fabricated zero timestamps.

### B-9. The terminalization ledger is keyed by identities that are not unique

The spec requires protocol completion, idle/release completion, and quarantine to be keyed separately by “Present serial/commit id” and forbids a rebuilt device from signalling an old generation (spec lines 2237–2239).

The plan keys protocol state only by `PresentSerial` and release only by `BufferRef` (lines 2869–2873). Client-supplied serials are not globally unique. The actual completion carrier already contains `client_id`, `present_id`, `window_generation`, `crtc_id`, and `crtc_epoch` (`crates/yserver-core/src/backend/trait_def.rs` lines 216–238). A buffer can also be reused by multiple Presents or generations.

A single global `generation` counter does not fix lookup by reused `BufferRef`; a late release can match a new entry with the same handle. Key protocol work by the existing monotonic `present_id` plus client/window lifetime as needed, and release/quarantine by commit/generation-specific ownership records. Make release proofs linear and generation-bound.

## Major

### M-1. The required `DRM_CRTC_SEQUENCE` conversion is absent

Spec lines 1772–1808 require every queue-sequence request to use a fresh owner token and typed `SequenceArm`, explicitly covering both relative idle and absolute Present-target producers. No task in the plan defines or converts `SequenceArm`, `IdleClockWake`, or `PresentTargetWake`.

The actual tree still has the production ioctl encoding raw identity in `user_data` (`crates/yserver/src/drm/page_flip.rs` lines 70–105) and both arm producers in `crates/yserver/src/kms/render/backend.rs` lines 1002–1033 and 9259–9319. Task 9 modifies only owner files; it cannot remove or convert those paths.

Add explicit conversion steps for both producers, arm cancellation/consumer removal, wrong-event-type poison, token tombstoning, and the rule that a sequence sample may advance clocks but never establish a commit milestone.

### M-2. Task 9 does not remove the actual separate sequence-support cache

The plan says “there is no separate device-keyed unsupported cache, which stage 1 already removed” (line 2050). The actual tree has:

```rust
HashMap<(DrmDeviceKey, ClockEpochId), SequenceSupport>
```

at `crates/yserver/src/kms/render/backend.rs` lines 1037–1042, with production reads/writes at lines 9246, 9297, and 9381–9405.

Although it is no longer the old process-lifetime `HashSet`, it remains separate from the required per-incarnation/per-hardware-CRTC clock record and omits hardware CRTC from its key. Spec lines 1755–1763 require the decision to live directly in that record.

Task 9 must inventory and remove/migrate these sites rather than assuming Stage 1 already did so.

### M-3. Stale clock-probe handling can poison the winning epoch

The spec says a stale result is discarded and the winning lifecycle transition owns any replacement probe (spec lines 1745–1750). The test instead maps a stale reply to `QualificationFailed(0)` (plan lines 2023–2029), while the implementation says an attempted unresolved record never retries in that epoch (line 2050). Errno zero is not a qualification failure, and the plan does not state how it avoids marking the new winning epoch attempted.

Also, `CrtcClockRecord` is declared without the `probe_attempted` flag used by its implementation (lines 1963–1965 versus 2050). Define a pending probe record carrying all correlation identities and make stale completion a neutral discard that cannot mutate the replacement epoch.

### M-4. Page-event normalization is not actually integrated into event handling

Task 8 says it hands normalized MSC/UST from future Task 10 to Present (plan lines 1932–1934), but Task 8 is committed before `clock.rs` exists. Task 10 later modifies only `clock.rs` (lines 2073–2075), not `events.rs` or `device_owner.rs`. Therefore no step wires `normalize_page_event` into the earlier event path.

The plan also stores only the extension reference. It does not store enough per-CRTC clock state to enforce:

> “A valid late sample … cannot move either CRTC clock backwards.” (spec lines 1824–1827)

Add an explicit integration step modifying `events.rs`/`device_owner.rs`, store the last validated general and completion samples, and test that late MSC and UST samples cannot regress either clock.

### M-5. Topology remapping and coordinate-lane precedence are stated by the spec but unenforced

No Task 13–14 operation preserves relative ticket age across a topology transition or drops only unremappable tickets, as required by spec lines 1469–1474 and 1578–1580.

The owner-mediated coordinate lane and its one post-`EBUSY` retry before atomic tiers are also absent despite spec lines 1475–1481. Deferring cursor payload construction does not remove the need for the admission representation if Task 14 claims to implement all seven-tier ordering.

Add typed topology remap/invalidation and coordinate-retry inputs, with tests for surviving ticket age and lifecycle-barrier precedence.

### M-6. Several tests prove helper behavior rather than the required invariant

- The starvation test manually marks every identity aged and checks only extraction order (lines 2655–2671). It does not model the already-submitted commit or assert the `N - 1` maximum per identity.
- The round-robin test manually supplies the correct owed CRTC (lines 2715–2727), so it does not prove production state advancement.
- `no_synthetic_transition_is_inserted_to_qualify` checks only initial construction (lines 2394–2398), not reopen, VT reacquire, or topology installation.
- The readiness assertion at lines 2427–2433 is tautological because `readiness_open()` is defined as `state == Ready`: `!readiness_open() || state == Ready` is always true.

Replace these with owner-level state-machine tests that exercise actual offer, retirement, selection, dispatch, and lifecycle transitions.

### M-7. Task 15 omits required FIFO, liveness, and teardown behavior

Spec lines 2226–2233 require notification suppression to follow client/drawable liveness while unblocking the per-client FIFO in either case, and permit release after either `PriorBufferReleased` or the teardown barrier.

The ledger exposes no client/drawable liveness input, FIFO-unpark result, or teardown-barrier method. `ReleasePoint::TeardownBarrier` is named at line 2778 but never implemented or tested. Add explicit outcomes for notification emission versus suppression, mandatory FIFO unpark, and generation-bound teardown release.

### M-8. Task 15’s own method signatures disagree

The interface says `record_displaced_successor` consumes `Displaced` (lines 2773–2777), the tests call it with two `PresentSerial` values (lines 2784–2792), and the implementation prose again describes a `displaced` argument (line 2871). This prevents implementing one API that matches the task’s tests and its integration with Task 13.

## Minor

### m-1. Admission ticket allocation has no overflow policy

`next_ticket: u64` is required to remain device-monotonic (plan line 2568; spec lines 1469–1474), but the plan does not require checked increment or define exhaustion behavior. Use checked allocation and an explicit invariant failure rather than wrapping and reversing oldest-ticket order.

### m-2. Task 10 exposes an unreachable `UstOverflow` distinction

`normalize_ust` takes `u32` seconds, so the maximum value is about `4.3 × 10^15` microseconds and cannot overflow `u64`. The checked arithmetic is appropriate, but `NormalizeError::UstOverflow` cannot arise from the declared input type. Either document it as defensive/future-proof or simplify the public error distinction.

## Notes on the rest

The `extend_sequence` arithmetic in Task 10 is sound for the specified `u64` domain. It examines the only possible neighboring congruent representatives, rejects the exact `2^31` tie, rejects lack of a nonnegative/in-range representative, treats raw zero as ordinary data, and follows the tested wrap:

```text
0x1_ffff_fffe
0x1_ffff_ffff
0x2_0000_0000
0x2_0000_0001
```

The additions inside its candidate construction do not overflow: `base` is `2^32`-aligned, `low <= 2^32 - 1`, and the upper block is included only when `base + 2^32` is representable. The defects are in clock-state integration, not this extension function.

The UST validity boundary (`tv_usec < 1_000_000`) and checked conversion match spec lines 1814–1815.

Task 11 correctly preserves the source-specific producer timer outside the post-dispatch record, arms Present-event timers from observed hardware completion, skips timers for events already received, and keeps one timer per required Present CRTC. The Stage 1 executor already enforces the 2-second/30-second host-call watchdog; the remaining problems are lifecycle-cohort validation and unchecked arithmetic.

The direct numeric anchors in the plan’s file inventory are current: `drm/modeset.rs:1562`, `:1635`, and `:1690`; `kms/render/platform.rs:5163`; `kms/render/backend.rs:1831`, `:1843`, and `:1892`; and `kms/backend.rs:844` all identify the stated live call sites. Tasks 9–15 themselves mostly name new files and contain no additional numeric anchors. The Task 15 baseline reference is also accurate: `ScanoutM2State` is currently at `kms/render/backend.rs:341`, with `deferred_successor_skips` and `idled` at lines 352–353.
