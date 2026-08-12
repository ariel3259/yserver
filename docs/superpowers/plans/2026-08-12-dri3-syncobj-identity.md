# DRI3 syncobj identity: pin the release handle, register the XID

**Branch:** `fix/dri3-syncobj-identity` (follow-up to #122, merged as `5b730f75`).
**Status:** revised after codex review — the first draft was incomplete in four ways,
recorded below so the corrections are not silently lost.

Two defects, one theme: a DRI3 syncobj's identity is tracked by **XID** where Xorg
tracks it by **reference**. Neither is reachable by a well-behaved client (xcb
allocates XIDs monotonically), so both need tests rather than a repro.

## Defect 1 — release identity is resolved by XID after accept

`PresentWake::PixmapSynced` carries `release_syncobj: u32`
(`backend/trait_def.rs:239`). Between **accept** and **release** a Present can sit
parked (deferred acquire, msc-parked); in that window a client may `FreeSyncobj(X)`
and `ImportSyncobj(X)` a *different* object. We then signal the replacement: the real
waiter hangs and an unrelated object gets a spurious signal.

The acquire path already pins correctly (`backend.rs:13188`), so release is the
outlier. Xorg is immune twice: `AddResource` (`dri3/dri3_screen.c:294`) and Present
referencing both syncobjs on accept.

### Every site that must change (verified, not assumed)

The first draft claimed supersession/teardown/shutdown "inherit" the fix. **False.**
Each signals by XID directly:

| site | path |
|---|---|
| `process_request.rs:1414` | window-destroy purge |
| `process_request.rs:8882` | supersession |
| `process_request.rs:9394` | copy-failure reroute |
| `process_request.rs:9597` | shutdown drain |
| `process_request.rs:8714` | `completed_event_for_pending()` rebuilds a fresh wake |
| `process_request.rs:~9184` | `execute_present_pixmap_copy()` rebuilds from the wire request |
| `backend.rs:1205` | direct-scanout late lookup |
| `backend.rs:19487` | copy-completion enqueue late lookup |

Audit gate — after the change, `rg 'dri3_signal_syncobj|dri3_syncobj_handle'` must show
XID use only in request-time validation/import/free, never in pending-Present handling.

## Defect 2 — the syncobj XID is not in the server's XID namespace

`ImportSyncobj` checks `resources.xid_in_use` (8 core tables) and stores the syncobj
only in the backend map, so nothing registers the XID:
`ImportSyncobj(X)` then `CreatePixmap(X)` succeeds (Xorg: `BadIDChoice`), and
`used_xids_in` (`server.rs:1695`) omits syncobjs so XC-MISC `GetXIDRange` can hand out
a live syncobj's XID. #122's own test only covers pixmap-then-syncobj
(`process_request.rs:37304`) — the direction that already works.

The defects are linked: XC-MISC is the realistic route to XID reuse within a client,
and defect 2 is what leaves it blind. Fixing 2 makes 1 unreachable; fixing 1 makes it
impossible.

## Design

1. **Store the complete wake on the pending Present.** `PendingPresentPixmap` gains
   `wake: PresentWake`, built once at accept:
   ```rust
   PresentWake::PixmapSynced {
       release: backend.dri3_syncobj_handle(req.release_syncobj).ok_or(..)?,
       release_syncobj: req.release_syncobj,   // IdleNotify + diagnostics only
       release_value: req.release_value,
   }
   ```
2. **Delete `PendingPresentRequest::wake()`** (`server.rs:1735`). Leaving a
   request-derived constructor available is how an unpinned wake gets reintroduced
   later. Every consumer uses `pending.wake.clone()`.
3. **Replace all four direct `dri3_signal_syncobj(xid, ..)` calls** with handle-based
   signalling from the carried wake, plus the two rebuild sites and the two backend
   late lookups.
4. **Drop `Copy` from `PresentWake`**, keep `Clone`. `CompletedPresentEvent` is already
   only `Clone` (`trait_def.rs:187`) so its ~91 sites don't ripple — but this is *not*
   purely mechanical: `process_request.rs:10417` and `:10724` match `event.wake`
   through a shared reference only because it is `Copy`, and need `match &event.wake`.
5. **Core-side XID registry.** `ServerState.dri3_syncobjs: HashMap<u32, ClientId>`,
   included in the **occupancy test the creation paths actually use** — extending only
   `used_xids_in` does **not** make `CreatePixmap` reject a syncobj XID. Requires
   auditing every creation handler that accepts a client XID (windows, pixmaps, GCs,
   pictures, regions, GLX objects). This exposes a pre-existing problem: creation paths
   do not consistently use one authoritative occupancy helper.
6. **Transactional rules for the mirror** (it is two maps; drift is the risk):
   import → backend first, core row only on success; free → ownership checked
   core-side, backend removed, core row removed only on backend success; disconnect →
   remove core rows and call backend cleanup exactly once. Failed import must not
   create a row; failed free must not delete one. Tests assert core/backend ownership
   agreement after each.
7. **Decide close-down semantics.** `process_disconnect()` honours `RetainPermanent` /
   `RetainTemporary`, but `Backend::client_disconnected()` unconditionally drops the
   client's syncobjs (`backend.rs:14424`). State explicitly whether syncobjs are
   connection-tied (always dropped) or retained like Xorg resources, and make the core
   mirror follow the same rule. `KillClient` likewise.

## Validation

The first draft's test **could not fail before the fix**: `RecordingBackend` stores
only `xid -> owner` and `dri3_syncobj_handle` mints a fresh stateless
`DummySyncobjHandle` per call (`recording.rs:612`), so A and B are indistinguishable;
and `RecordingBackend` doesn't implement `enqueue_present_completion`, so "drive to
completion" exercises neither backend lookup.

- **Give `RecordingBackend` identity-bearing handles**: `HashMap<u32, (ClientId,
  Arc<RecordingSyncobj>)>`, each with its own signal log. Prerequisite for everything
  below.
- **Core assertion**: after `FreeSyncobj(X)` + `ImportSyncobj(X)` while parked,
  `Arc::ptr_eq(&original, &pending.wake.release)`.
- **Per-path tests**: deferred-acquire/msc-parked normal completion, supersession,
  window-destroy purge, copy-failure reroute, shutdown drain, direct-scanout accept.
  Each asserts A signalled, B untouched.
- **Defect 2 tests**: syncobj-then-pixmap is `BadIDChoice`; `used_xids_in` includes a
  live syncobj XID.
- Existing suite (2257) green; `cargo +nightly fmt`; `cargo clippy --all-targets`.
- One mpv `--gpu-api=vulkan` synced run: imports/frees still pair, no new warnings.

## Out of scope, stated explicitly

- Full Xorg resource-type machinery. A `ResourceTable` record plus an independent
  backend map is still a mirror; a genuinely authoritative design would have backend
  import return the handle to core and pass handles into acquire/release — a larger
  trait redesign. The scoped mirror is defensible, not ideal.
- **Pre-existing release-hang** at `process_request.rs:10314`: an accepted synced
  Present with an unresolved backend drawable or depth mismatch is logged and dropped
  **without signalling the release point**. Not introduced here; file separately unless
  it is cheap to fix in passing.
- Server reset: no live reset path exists today.

## Risks

- Dropping `Copy` touches ~12 sites and two of them rely on it implicitly (above).
- A retained `Arc` outlives `FreeSyncobj` until the Present retires — intended Xorg
  semantics, but **unbounded**: the first draft called it "bounded by ≤5 observed
  entries", which is a measurement, not a bound. A client can accumulate parked
  Presents; there is no enforced queue limit.
- Touching the completion path again so soon after #122 — mitigated by this being the
  path #122 introduced, untested in the reuse direction.
