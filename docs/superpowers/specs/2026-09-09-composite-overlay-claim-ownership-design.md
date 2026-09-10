# Composite overlay claim ownership — design

## Status

Draft, unimplemented. Written 2026-09-09 against master `64d4b6e4`.

**A pre-existing COMPOSITE lifecycle bug, not part of #121** — a compositor
crash leaks the overlay claim on master today, with no reset anywhere in the
picture. It gets its own spec because it touches claim ownership, the normal
disconnect path, `KillClient`, retained clients, forced teardown and
direct-scanout unflip failure semantics.

**It is a hard prerequisite for enabling server reset** (#121 stage 3,
`2026-09-09-server-reset-design.md`). Until it lands, `-reset` is not a
complete session boundary: a crashed compositor's claim crosses into the next
session, and under XDMCP that is a different user. Once it lands, reset must
inherit the cleanup through `force_destroy_all_clients` with **no COW-specific
loop and no exceptional path** — an earlier bounded-decrement loop inside
`reset_generation` was written and rejected for exactly that reason.

## Goal

Make the overlay claim a per-client resource whose lifetime is bound to the
claiming client, so that every way a client can go away releases its claims,
and the overlay is torn down when the last claim disappears.

**Non-goals:**

- Changing when the COW is *created*, or its contents, or the scanout paths
  it participates in. Only claim ownership and teardown.
- The scene's `root_overlay` contribution. Different concept, confusingly
  similar name — see below.

## Current behaviour on master

**The claim is an anonymous integer.** `get_overlay_window` increments
`core.cow_refcount` (`kms/render/backend.rs:20050`), `release_overlay_window`
decrements it. Nothing records *which* client claimed, so:

1. **A disconnecting client never releases.** `release_overlay_window` is
   called only from the protocol handler (`process_request.rs:7232`).
   `process_disconnect` does call `backend.client_disconnected`, and that
   *does* clear a per-client overlay thing — `scene.root_overlay_on_disconnect`
   (`backend.rs:19621`), with a passing test named
   `client_disconnected_clears_overlay`. **That is the scene's root-overlay
   contribution, not the COW claim.** Anyone checking "does disconnect clean up
   the overlay?" finds that call, that comment and that test, and concludes
   wrongly. The COW refcount is untouched.
2. **Any client may release any claim.** The handler calls
   `release_overlay_window` with no ownership check, so a client that never
   called `GetOverlayWindow` can decrement a claim held by another client, and
   at refcount 1 can tear the overlay out from under it. Xorg answers
   `BadMatch` here.
3. **A backend failure is swallowed.** The handler logs a warning and treats
   `Err` as "not the final release" (`process_request.rs:7234-7242`), so the
   protocol reports success while the teardown did not happen.

## Reference: how Xorg does it

All references `../xserver`.

- **Every `GetOverlayWindow` creates a record.**
  `ProcCompositeGetOverlayWindow` (`composite/compext.c`) calls
  `compCreateOverlayClient` unconditionally, which mallocs a
  `CompOverlayClientRec`, gives it `FakeClientID(pClient->index)` and registers
  it via `AddResource(..., CompositeClientOverlayType, ...)`
  (`composite/compoverlay.c`). So **N Gets by one client create N records**,
  each a client-owned resource.
- **Release frees exactly one.** `ProcCompositeReleaseOverlayWindow` uses
  `compFindOverlayClient(pScreen, client)` and, if the *calling* client holds
  no record, returns **`BadMatch`**. Otherwise it frees one resource. Get and
  Release are therefore paired 1:1.
- **Disconnect is automatic.** The records are ordinary client resources, so
  the resource system frees all of a client's records when it goes away — no
  COMPOSITE-specific disconnect hook exists, and none is needed. This is the
  whole design.
- **The last claim destroys the window.** `compFreeOverlayClient` unlinks the
  record and then: `if (cs->pOverlayClients == NULL)
  compDestroyOverlayWindow(pScreen);`

## Design

### Claims become per-client core state

Move ownership out of the backend's anonymous counter and into core state, as
Xorg does:

- `ServerState` holds the claim list: one entry per `GetOverlayWindow`,
  recording the owning `ClientId`. Repeated Gets by one client create repeated
  entries — matching Xorg, and bounding nothing artificially, because the
  entries are freed with the client.
- **Core owns the claim list exclusively. The backend keeps no claim count of
  its own.** `core.cow_refcount` goes away as an independently
  incremented/decremented quantity; two counters that can disagree is the
  current defect in a new costume. The backend's API narrows to two edges:

  | Edge | Backend call |
  |---|---|
  | core claims 0 → 1 | materialize the overlay |
  | core claims 1 → 0 | final teardown |

  Both keep returning `io::Result<bool>` rather than `io::Result<()>`, which
  the "two edges" phrasing might suggest. The bool is load-bearing for backends
  that do not model a COW at all: the trait default `Ok(false)` is what stops
  core driving `materialize_cow_resource` for v1/ynest, whose `cow_host_xid()`
  is `None` and would hit an `.expect()`. Its meaning is now "this backend owns
  / tore down a COW", not "this was the final release" — the latter is core's
  question and core now answers it.

  Non-final Gets and Releases do not reach the backend at all.

  **Transactional ordering**, so a failure cannot desynchronise the two
  layers — in each case the durable state changes only on success:

  - **First Get:** create the core claim, then materialize. If materialization
    fails, **roll the claim back** and return `BadAlloc`. Never leave a claim
    recorded against an overlay that does not exist.
  - **Final Release:** attempt backend teardown *first*; **do not remove the
    last core claim unless teardown succeeds.** The claim is the thing keeping
    the overlay alive, so dropping it before the overlay is gone is precisely
    the leak.
  - **Non-final Release:** remove one claim belonging to the caller. The
    backend is not called.
- `GetOverlayWindow` adds an entry. `ReleaseOverlayWindow` removes **one entry
  belonging to the calling client**, and returns `BadMatch` when that client
  holds none — a protocol fix in its own right.

### One release helper, four callers

**Correction, from implementation:** an earlier draft of this section said a
single `release_client_overlay_claims` served all four callers. It cannot —
that helper releases *all* of a client's claims, and the protocol path must
release exactly **one**, per Xorg's 1:1 pairing. What is genuinely shared is
`teardown_overlay`, the 1 → 0 edge; the protocol path and the disconnect helper
are two separate callers of *that*.

So, the four departure routes and what each does:

1. **`ReleaseOverlayWindow`** — removes one claim belonging to the caller, and
   calls `teardown_overlay` only if it was the last. Does **not** use
   `release_client_overlay_claims`.
2. **Normal disconnect** (`process_disconnect`) — releases *all* of that
   client's claims.
3. **`KillClient`** — note this path calls `process_disconnect` inline
   (`process_request.rs:22458`) rather than going through
   `disconnect_with_pending_cleanup`, so the release must live in
   `process_disconnect` itself, not in the funnel above it. (That same bypass
   already leaks parked CRTC tokens; separate bug, recorded in the reset spec.)
4. **Forced teardown** (`force_destroy_all_clients`) — inherits it for free
   via 2, which is the point.

**Retained clients** (`RetainPermanent`/`RetainTemporary`) need an explicit
decision, and the answer is that a claim is **not** a retainable resource: the
overlay is a screen-wide facility, and a zombie holding it would block every
future compositor. Release claims when the client's connection goes, whatever
its close-down mode. Note this differs from Xorg only in that Xorg's
`FakeClientID` resources follow the ordinary retain rules; our reason for
diverging is that we have a real reset boundary to protect and Xorg's
retention semantics for a screen-wide singleton are not worth inheriting.
State it in the code, do not let it be inferred.

### Failure semantics of the final release

`release_overlay_window`'s `refcount == 1 && scanout_m2.active()` branch calls
`materialize_direct_shadow_for_unflip()?` (`kms/render/backend.rs:20169`) and
can fail; by design it then leaves the refcount, the COW and the direct pins
untouched so a compositor can retry.

That is right for the protocol path and **wrong at a session boundary**:

- **Protocol path** (`ReleaseOverlayWindow`): keep the caller's claim — the
  release did not happen — and return **`BadAlloc`**. That is the mapping Xorg
  already uses for allocation failure in this extension
  (`composite/compext.c:216,220,263,297`), and a shadow-buffer allocation
  failing is exactly that. Do not swallow it into `false` as today, which
  reports protocol success for a teardown that did not occur.
- **Disconnect path**: the client is gone and cannot retry, so there is no
  "retryable" state to be in. Enter an explicit **`CowTeardownFailed`** state —
  a distinct, sticky, session-fatal condition, *not* a lingering ordinary
  claim. While it holds, no new compositor may take the overlay
  (`GetOverlayWindow` ⇒ `BadAlloc`), because whatever it would receive is
  inherited state from a session that could not be torn down.
- **Reset boundary**: seeing `CowTeardownFailed`, **terminate**. Never continue
  into a new session with inherited COW state. A reset that half-succeeds is
  worse than one that refuses.

**Where it lives: `ServerState::cow_teardown_failed`.** Core state, alongside
the claim list — not a backend flag and not a reset-local variable. Core owns
claim lifetime, so it must own the state that says lifetime broke. Living in
`ServerState` also gives it the right lifetime under `-noreset`, the ordinary
case: it persists for as long as the server does, which is exactly how long the
orphaned overlay does.

That placement has one consequence a reset implementation must respect, and it
is easy to get backwards: **`reset_generation` replaces `ServerState`**, so the
check must happen *after* forced disconnect cleanup — which is what can set the
flag — and *before* the state is swapped. Checking after the swap reads a fresh
`ServerState` with the flag clear, so the reset would discard the evidence and
proceed into the session it exists to refuse.

**Why a distinct state rather than a stuck claim** (this is the resolution of
an outright contradiction in the first draft): invariant 2 says every departing
client releases all of its claims. A disconnect that leaves the claim recorded
would violate it, and worse, the leftover would be indistinguishable from a
live claim — the next compositor would see "someone holds the overlay" and
wait for a client that no longer exists. `CowTeardownFailed` is not a claim: no
client owns it, nothing can release it, and its only exits are process
termination or a reset that refuses to proceed.

## Invariants

1. Every claim has an owning client; there are no anonymous claims.
2. Every way a client can go away — orderly disconnect, `KillClient`, forced
   reset teardown, crash — releases all of its claims. **No claim outlives its
   owner.** If that release was the final one and the backend teardown failed,
   the claims are still released *and* the server additionally enters
   `CowTeardownFailed` (5). The two are not alternatives.
3. `ReleaseOverlayWindow` from a client holding no claim is `BadMatch` and
   changes nothing.
4. A client cannot release another client's claim.
5. The overlay is torn down exactly when the last claim disappears, and not
   before — **except in `CowTeardownFailed`**, which is precisely the state
   where that correspondence breaks and is why it exists.

   Normally a claim exists only while the overlay does, in both directions;
   the transactional ordering above is what guarantees it, and on the protocol
   path a failed final teardown keeps the caller's claim so the correspondence
   holds there too. On the **disconnect** path it cannot: invariant 2 forbids a
   claim outliving its owner, so the claim goes while the overlay is still
   materialized. **`CowTeardownFailed` owns that orphaned overlay** until the
   process terminates. Nothing else may claim it — `GetOverlayWindow` is
   `BadAlloc` while the state holds — and nothing can release it, because
   there is no longer a client to attribute it to.
6. Reset needs no COW-specific code: `force_destroy_all_clients` inherits the
   cleanup through the ordinary disconnect path.
7. A reset never completes while a claim remains held **or while
   `CowTeardownFailed` holds**. The second half is the one that bites: after a
   failed disconnect teardown there is no claim left to check, so a
   claim-only test would wave the reset through into a session inheriting a
   pinned overlay.

## Risks

- **Two authorities.** If the backend keeps an independent refcount alongside
  the core claim list, they can drift, which is today's bug reintroduced.
  Pick one authority explicitly.
- **The scene `root_overlay` name collision** will mislead the implementer at
  least once. They are unrelated; `client_disconnected` already handles the
  scene one correctly and must keep doing so.
- **Direct scanout is live on this path.** The `refcount == 1 &&
  scanout_m2.active()` branch is the one that touches real pinned buffers, and
  it is the least test-covered part of the change.

## Verification

- **The test that must exist regardless of reset**, and the one that proves
  the bug is fixed at its own level: a compositor client takes the overlay and
  **disconnects without releasing** — its claim is gone and the overlay is torn
  down, with no reset involved.
- Repeated `GetOverlayWindow` from one client, then that client disconnects:
  all its claims go, matching Xorg's N-records model.
- Get by client A, `ReleaseOverlayWindow` by client B ⇒ `BadMatch`, and A's
  claim survives.
- Two claimants, one disconnects ⇒ the overlay survives; the second
  disconnects ⇒ it is torn down.
- `KillClient` on a claimant releases its claim, via the inline
  `process_disconnect` path.
- A `RetainPermanent` claimant's claims are released when its connection goes.
- Failure injection on `materialize_direct_shadow_for_unflip`, all four
  outcomes: first Get fails ⇒ `BadAlloc` and **no claim recorded**; final
  Release fails ⇒ `BadAlloc` and the caller **keeps** its claim; disconnect
  final release fails ⇒ `CowTeardownFailed`, and a subsequent
  `GetOverlayWindow` from a fresh client is `BadAlloc`; reset under
  `CowTeardownFailed` ⇒ terminates rather than installing a new generation.
- Then, in the reset branch: `force_destroy_all_clients` needs no
  COW-specific code and the previously-leaking case is clean.
- Hardware: a compositor killed with `SIGKILL` mid-session leaves a usable
  display and a subsequent compositor can take the overlay.

## Adjacent gaps, not in scope

- `KillClient` bypassing `disconnect_with_pending_cleanup`
  (`process_request.rs:22458`) also leaks the killed client's parked CRTC
  token. Same bypass, different resource; recorded in the reset spec.
