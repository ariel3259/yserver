# Stage 2c-iii, plan Ciii Task 6 — F8: direct framebuffer ownership is absent from production

**Baseline:** `13081c99`  
**Result:** **BLOCKING F8 STOP.** Task 6 cannot construct the P3-3 commit required
by the conversion spec. Plan Ciii is not accepted, and its hardware gate must
not run, until a new, focused spec defines and a reviewed plan implements the
missing direct-framebuffer ownership path.

## What Task 6 found

Task 6 requires a real sequence of composed → direct → same-source direct →
unflip. The second direct commit must carry the same framebuffer
`AllocationKey` as the first for the same `GroupMember`; only that shape can
prove that a retained allocation receives no `KmsRelease` obligation (P3-3).

The implemented production route cannot build that shape:

1. `direct_owner::resources` reaches
   `KmsBackend::take_direct_owner_resources`.
2. That function moves the source and fallback `StorageLease`s, but constructs
   `CommitResources` with an empty `allocations` vector
   ([backend.rs:2596](../../../crates/yserver/src/kms/render/backend.rs#L2596)).
3. The framebuffer imported by the M1 probe remains strongly owned by
   `ScanoutM1ProbeEntry::_framebuffer`; direct submission borrows its raw handle
   from that cache ([backend.rs:520](../../../crates/yserver/src/kms/render/backend.rs#L520),
   [backend.rs:2715](../../../crates/yserver/src/kms/render/backend.rs#L2715)).
4. `DirectScanoutProbeFramebuffer::into_managed` exists, but has no production
   caller ([modeset.rs:1425](../../../crates/yserver/src/drm/modeset.rs#L1425)).
5. `register_commit_dependencies` decides retained versus displaced ownership
   only from old and new `CommitResources::allocations`; it deliberately does
   not compare `source` or `fallback`
   ([commit.rs:677](../../../crates/yserver/src/kms/render/resources/commit.rs#L677)).

Both direct commits therefore have `allocations = []`. Reusing the same client
source can retain a Vulkan storage lease, but it cannot express the identity or
lifetime of the KMS framebuffer/GEM import. This is a code-shape impossibility,
independent of card1, its driver, or its current mode.

## The missed contract

This is missing infrastructure, not a Task 6 test-fixture problem. The Stage
2c-i resource design already required active and quarantined records to retain
the direct framebuffer independently of cache membership and said that
`DirectPresentFrame` could not remain only a numeric-pin owner
([resource design:38](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L38),
[resource design:46](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L46)).
It also required the converted cache to hold only weak indices outside the six
bounded direct roles
([resource design:425](../specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md#L425)).

The corresponding implementation-plan item remained explicitly open: the
conversion helper was tested in isolation, but production adoption, payload
alias registration and weak cache indexing were left to the later session that
wired the real producer
([resource plan:263](../plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md#L263)).
Plan Cii then converted the direct producer's Present pins and members without
carrying this open framebuffer allocation into `CommitResources`.

The Ciii reviews checked the downstream half of reachability: a real direct
dispatch calls `register_commit_dependencies`, which calls `register_kms`.
They did not check the upstream payload premise: whether the producer can place
the scanned framebuffer allocation into the old and new resource sets. Ten
adversarial rounds refined the retaining sequence and the registration call
graph, but none inspected the contents of those resource sets. The statement
that the registration path was reachable was true; the conclusion that P3-3
was reachable was not.

## Required spec before Task 6 resumes

Create a focused design spec for **production direct-framebuffer adoption and
retention**. It must reconcile the current producer/conductor code with the
existing Stage 2c-i ownership contract and settle all of the following before
implementation:

1. **Authoritative ownership boundary.** Define the exact transition that
   consumes an accepted M1 `DirectScanoutProbeFramebuffer` into the device
   incarnation's `DrmCleanupRegistry` and `ResourceService`. On the `Owner`
   route the probe cache becomes an index; it cannot remain an independent
   strong owner whose eviction may destroy an active import.
2. **Stable physical identity.** Define the identity used to reuse a managed
   framebuffer for the same device incarnation, source allocation generation
   and valid topology. A repeated same-source direct Present must acquire the
   same service `AllocationKey`; a new source generation, invalidated import,
   topology change or incarnation must not alias it.
3. **Producer-to-record handoff.** Define how the direct frame carries the
   managed framebuffer allocation through `Preparing`, `Successor` and
   `Submitted`, and how the conductor moves it by value into
   `CommitResources::allocations` for the captured members. The scanned
   framebuffer allocation and the `source`/`fallback` storage leases are
   different rights and must not be conflated.
4. **Every rollback and terminal path.** Give an ownership table for probe
   failure, successor replacement, capacity refusal, `begin` refusal, dispatch
   rejection, accepted completion, unknown completion/quarantine, ordinary
   replacement, unflip/exit retirement, cache eviction, topology invalidation,
   device loss and teardown handoff. Each row must name the sole owner before
   and after the transition and the proof that permits FB/GEM cleanup.
5. **Registry and service access.** Choose the production seam that gives the
   M1/direct producer scoped access to the matching incarnation's resource
   service and cleanup registry. It must preserve device qualification and the
   existing rule against long-lived backend borrows; a test-only registry is
   not an implementation.
6. **Bounded retention.** Show that managed framebuffer leases occupy the same
   six direct capacity roles already specified, including the transient
   Preparing/Successor overlap and both retirement roles. Reuse cannot create
   an uncounted cache owner or an extra release duty.
7. **Executable evidence and mutations.** Before the card1 gate, add
   hardware-free tests through the production producer proving: non-empty
   direct allocations; identical keys for a valid same-source successor;
   distinct keys after generation/topology invalidation; exact restoration on
   every pre-submit refusal; cache eviction cannot destroy current/submitted
   ownership; and FB/GEM cleanup happens exactly once. Mutations must at least
   catch dropping the allocation handoff, minting a fresh key on reuse, keeping
   a strong cache owner, and losing the allocation on rollback.

The spec must also state explicitly that changing
`register_commit_dependencies` to compare `source` is not a repair: it would
attach KMS release semantics to the Vulkan/store allocation while leaving the
framebuffer/GEM right outside the commit record.

## Re-entry gate

Task 6 may resume only after the new spec and its implementation plan pass the
repository's adversarial review, the production adoption is implemented, and
the software evidence above passes. The original hardware test then remains the
acceptance gate: composed → direct → same-source direct → unflip on card1, with
T32 proving displaced registration and T33 proving retained non-registration.
No hand-built `CommitResources`, synthetic allocation, or direct call to the
registration helper can substitute for that route.

Tasks 1–5 are not invalidated by this finding, but plan Ciii and stage 2c-iii
remain open. The copied-route plan is still owed after Ciii.

## Hardware safety

No DRM-master acquisition, `_drm` test, modeset, GPU mutation or hardware gate
was run for this finding. Static inspection was sufficient to trigger the
plan's F8 rule, and `c0_hw_ciii_owner_route_on_card1_drm` was not created.
