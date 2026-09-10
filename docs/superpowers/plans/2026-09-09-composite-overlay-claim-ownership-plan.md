# Composite overlay claim ownership — implementation plan

Implements `../specs/2026-09-09-composite-overlay-claim-ownership-design.md`.
Read that first; this plan does not restate its reasoning.

**Branch:** `fix/composite-overlay-claim-ownership`, off **master**.

Confirmed 2026-09-09 (jos, codex). This is *not* part of #121: it fixes a leak
and a cross-client protocol hole that exist on master today, so it lands
independently, and #121's all-or-nothing merge does not apply to it.

**How this meets the #121 stack.** jos: #121 will be squash-merged, not
fast-forwarded. So there is no intermediate rebase to sequence — the stack
lands as a squash whenever it lands, and this fix's overlap with it is resolved
once, at that point, rather than branch by branch on the way there.

The overlap to expect is in **`process_disconnect.rs`**: step 2 below adds
claim release to exactly the function reset's `force_destroy_all_clients`
drives. That is the design working, not friction — if resolving the two
produced *no* overlap there, it would mean the cleanup had not landed where
reset inherits it, and step 4's "delete the block and add nothing" would not
hold. Treat a clean merge in that file as a signal to check, not a relief.

Whichever order they land in, this fix is independent: it never needs reset,
and reset must not ship without it.

**Deliverable:** a compositor that takes the overlay and is `SIGKILL`ed leaves
a usable display, its claim gone, and a second compositor can take the overlay
afterwards. No reset involved in that sentence — that is the point.

## Ordering principle

**Protocol correctness, then lifetime, then failure states, then reset.**
Each step is provable on its own and none depends on reset existing.

Step 1 makes core the single authority — the whole bug class is two counters
that can disagree, so the transition must not pass through a state where both
exist. Step 2 is the actual leak fix and the one with standalone value. Step 3
adds the failure state, which cannot be designed before 1 and 2 define what
success looks like. Step 4 is reset integration and is the only part that
touches the other branch.

## Prerequisites

- `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` clean before each commit.
- **No xts A/B.** Nothing here changes how anything is drawn; the overlay's
  contents and the scanout paths are explicitly out of scope.
- Hardware only for step 5.

---

## Step 1 — core owns the claims; the backend counts nothing

- `ServerState` gains the claim list: one entry per `GetOverlayWindow`,
  recording the owning `ClientId`. Repeated Gets from one client create
  repeated entries, matching Xorg's N-records model.
- **Remove `core.cow_refcount` as an independently maintained counter**
  (`kms/render/backend.rs:20050` and friends). The backend must not end this
  step still counting alongside core — that is the defect in a new costume.
  Its API narrows to two edges: materialize on core 0 → 1, final teardown on
  core 1 → 0.
- `GetOverlayWindow` adds a claim. `ReleaseOverlayWindow` removes **one** claim
  belonging to the **calling** client, and returns `BadMatch` when the caller
  holds none (`ProcCompositeReleaseOverlayWindow` via `compFindOverlayClient`).
- Transactional ordering, durable state changing only on success:
  - first Get: add the claim, materialize, and **roll the claim back** plus
    return `BadAlloc` if materialization fails;
  - final Release: teardown **first**; keep the last claim unless it succeeds;
  - non-final Release: remove the caller's claim, do not call the backend.

**Proof.** Ownership and pairing, all against the recording backend: Get by A
then Release by B ⇒ `BadMatch` and A's claim survives; Release with no claim
anywhere ⇒ `BadMatch`; N Gets need N Releases; two claimants, first Release is
not final. Rollback: injected materialize failure leaves **no** claim recorded.
**Rename `get_overlay_window_is_idempotent_across_repeated_calls`**
(`process_request.rs:48685`) to `repeated_get_reuses_materialized_host_overlay`
or similar. The behaviour it checks survives — `host_xid` stability, and its
own comment already says "repeated GETs still bump the backend refcount", so
only *what* is counted moves to the core claim list. But the name is actively
misleading under this model: repeated `GetOverlayWindow` is deliberately **not**
protocol-idempotent, since each call creates another claim that needs its own
Release. What is stable is the identity of the already-materialized host
overlay, and the name should say that so the distinction cannot be lost. If the
test needs more than a rename plus a change of what it counts, stop and report.

## Step 2 — every departure releases its claims (the leak fix)

- One helper, `release_client_overlay_claims(state, backend, client)`.
- Call it from **`process_disconnect`**, not from `disconnect_with_pending_cleanup`.
  That placement is load-bearing: `KillClient` on another client's resource
  calls `process_disconnect` inline (`process_request.rs:22458`) and bypasses
  the funnel, so a helper in the funnel would miss it.
- **Claims are not retainable.** Release them whatever the close-down mode,
  unlike ordinary resources. Put the reason in the code: the overlay is a
  screen-wide singleton and a zombie holding it would block every future
  compositor.

**Proof.** The headline test, which must exist independently of reset: a client
takes the overlay, **disconnects without releasing**, and its claim is gone and
the overlay torn down. Plus `KillClient` on a claimant; a `RetainPermanent`
claimant's claims released anyway; two claimants where only the second
departure tears down.

## Step 3 — failure semantics and `CowTeardownFailed`

`release_overlay_window`'s `refcount == 1 && scanout_m2.active()` branch calls
`materialize_direct_shadow_for_unflip()?` (`kms/render/backend.rs:20169`) and
can fail. Three different right answers:

- **Protocol path:** keep the caller's claim and return **`BadAlloc`** — Xorg's
  mapping for allocation failure in this extension
  (`composite/compext.c:216,220,263,297`). Stop swallowing `Err` into `false`
  (`process_request.rs:7234`), which reports success for a teardown that did
  not happen.
- **Disconnect path:** remove the departed client's final claim (no claim
  outlives its owner) **and** set the state — sticky, owned by nobody,
  releasable by nothing. It owns the orphaned still-materialized overlay until
  the process ends.
- **Where it lives: `ServerState::cow_teardown_failed`.** Core state beside the
  claim list — not a backend flag, not an ad-hoc reset-local variable. Core
  owns claim lifetime, so it owns the record that lifetime broke, and
  `ServerState` gives it the right lifetime under `-noreset`: as long as the
  server, which is as long as the orphaned overlay.
- **`GetOverlayWindow` checks the state first**, before anything else, and
  returns `BadAlloc`.

**Proof.** Failure injection, all four outcomes: first Get fails ⇒ `BadAlloc`,
no claim; final Release fails ⇒ `BadAlloc`, caller keeps its claim; disconnect
final release fails ⇒ claims released *and* `CowTeardownFailed` set; a fresh
`GetOverlayWindow` under that state ⇒ `BadAlloc`.

## Step 4 — reset inherits it (in `feat/121-server-reset`)

Not on this branch. After rebasing reset onto the merged fix:

- Delete the "NOT handled here" block in `reset_generation`
  (`core_loop/reset.rs`) and confirm **no COW-specific code replaces it** —
  `force_destroy_all_clients` gets the cleanup through step 2's
  `process_disconnect`.
- The reset boundary refuses to proceed under `CowTeardownFailed`, terminating
  instead. Two things about that guard, both easy to get wrong:
  - It must test the **state**, not "is a claim held". After a failed
    disconnect teardown there is no claim left, so a claim-only check waves the
    reset through into a session inheriting a pinned overlay.
  - It must run **after `force_destroy_all_clients`** — which is what can set
    the flag — and **before `*state` is replaced**. The flag lives in
    `ServerState`, so a check placed after the swap reads a fresh state with it
    clear: the reset would destroy the evidence and then proceed on the
    strength of its absence.

**Proof.** The previously-leaking case is clean after a reset, with no
COW-specific path taken. A reset under `CowTeardownFailed` terminates rather
than installing a new generation.

## Step 5 — hardware

- `SIGKILL` a compositor mid-session: the display stays usable, and a second
  compositor can take the overlay afterwards. This is the whole bug, observed.
- Repeat under a `-reset` server once step 4 has landed.

## Hazards

- **Ending step 1 with two counters** is the failure that reproduces the
  original bug. The backend must stop counting in the same commit core starts.
- **`scene.root_overlay` is a different thing** with a confusingly similar
  name, already handled correctly by `client_disconnected`
  (`kms/render/backend.rs:19621`), with a passing test called
  `client_disconnected_clears_overlay`. Do not touch it, and do not mistake it
  for evidence the COW claim is handled.
- **The 1 → 0 edge touches live pinned buffers** via
  `materialize_direct_shadow_for_unflip`, and is the least-covered path in the
  change. The recording backend cannot exercise a real unflip, so step 5 is
  where that first runs for real.
- **`BadMatch` on Release is a behaviour change** a buggy compositor could
  notice: today an unpaired Release silently succeeds. It matches Xorg, so a
  client relying on the old behaviour is relying on our bug — but if something
  breaks, this is the change that did it.
