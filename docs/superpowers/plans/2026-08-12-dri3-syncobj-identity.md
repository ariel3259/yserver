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
| `process_request.rs:9359` | `let wake = pending.request.wake()` before the copy-failure reroute — distinct from the XID signal at `:9394`, and must become `pending.wake.clone()` before `pending` is consumed |
| `backend.rs:1205` | direct-scanout late lookup |
| `backend.rs:19487` | copy-completion enqueue late lookup |

Audit gate — after the change, `rg 'dri3_signal_syncobj|dri3_syncobj_handle'` must show
XID use only in request-time validation/import/free, never in pending-Present handling.
Exceptions: trait definitions, backend implementations and unit tests legitimately keep
mentioning both.

Construction sites are only two in production — `PresentPixmap` at
`process_request.rs:9957` and `PresentPixmapSynced` at `:10266`; everything else is a
test fixture. The destructuring at `:9149` must also carry `wake` through, so direct
scanout and fallback completion both receive the stored value.

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
   purely mechanical: `process_request.rs:8741`, `:10416`, `:10724` and `:10744` match `event.wake`
   through a shared reference only because it is `Copy`, and need `match &event.wake`.
5. **Core-side XID registry.** `ServerState.dri3_syncobjs: HashMap<u32, ClientId>`,
   added to **`ServerState::xid_occupied()`** (`server.rs:1677`) and `used_xids_in()`,
   and its coverage test `xid_occupied_covers_every_namespace` (`server.rs:5622`)
   extended from 18 namespaces to 19.

   The authoritative predicate already exists, and its doc comment states the
   obligation outright: *"extend this (and the `xid_occupied_covers_every_namespace`
   test) when adding an XID-keyed map."* #122 added an XID-keyed map and did not. So
   this is a missed documented obligation, not a design gap.

   What does **not** exist is a single `legal_new_resource` helper combining occupancy,
   client range and error generation. Most creation handlers bypass `xid_occupied()`
   and use `ResourceTable::xid_in_use()` plus ad-hoc extension-map checks — which is
   why `ImportSyncobj` itself can still collide with extension-only maps today.
   Narrowing this to `CreatePixmap` + XC-MISC would leave syncobjs colliding with
   windows, GCs, regions, pictures, cursors, colormaps and GLX resources, so the claim
   "syncobjs participate in the XID namespace" would be false. Introduce
   `legal_new_resource` and migrate the handlers, or accept that the claim is partial
   and say so.
6. **Transactional rules for the mirror** (it is two maps; drift is the risk):
   import → backend first, core row only on success; free → ownership checked
   core-side, backend removed, core row removed only on backend success; disconnect →
   remove core rows and call backend cleanup exactly once. Failed import must not
   create a row; failed free must not delete one. Tests assert core/backend ownership
   agreement after each.
7. **Close-down semantics — answered.** Xorg **retains** DRI3 syncobjs:
   `dri3_syncobj_type = CreateNewResourceType(dri3_syncobj_free, "DRI3Syncobj")`
   (`dri3/dri3.c:106`) carries **no `RC_NEVERRETAIN`**, so they behave as ordinary
   resources — `RetainPermanent` keeps them until server reset, `RetainTemporary`
   until `KillClient(AllTemporary)` or an explicit `KillClient` naming one of that
   zombie's resources, `DestroyAll` destroys them on disconnect. A Present-held
   reference may outlive the resource row until that Present retires.

   Our `Backend::client_disconnected()` drops them unconditionally
   (`backend.rs:14424`), which is **wrong for retained clients**. Stop making that
   generic hook own syncobj destruction: free registered syncobj XIDs when the
   resources are actually destroyed, and ensure each XID goes through exactly one
   backend-removal mechanism (the current wording "disconnect → remove core rows and
   call backend cleanup exactly once" contradicts retention).

8. **`KillClient` must see syncobjs.** It resolves ownership only through
   `state.resources.resource_owner()` (`process_request.rs:20825`), so a retained
   syncobj cannot name its zombie client as it does in Xorg. Add syncobj ownership to
   that lookup and make `destroy_zombie_resources()` destroy both rows.

## Validation

The first draft's test **could not fail before the fix**: `RecordingBackend` stores
only `xid -> owner` and `dri3_syncobj_handle` mints a fresh stateless
`DummySyncobjHandle` per call (`recording.rs:612`), so A and B are indistinguishable;
and `RecordingBackend` doesn't implement `enqueue_present_completion`, so "drive to
completion" exercises neither backend lookup.

- **`RecordingBackend` needs more than identity-bearing handles.** Identity-bearing
  `HashMap<u32, (ClientId, Arc<RecordingSyncobj>)>` with per-object signal logs is
  necessary but **not sufficient**: `enqueue_present_completion()` is still the no-op
  trait default, `signal_present_wake()` only records a `present_id` without signalling
  a retained handle, and `try_present_direct()` discards the completion event on
  success. So it must additionally: retain the event/wake by `present_id` in
  `enqueue_present_completion`, expose it from the completion drain, signal-and-remove
  in `signal_present_wake`, and retain + expose test controls when `try_present_direct`
  returns true.
- **Which paths are writable, and when.** Immediately writable after the identity
  change: supersession, window-destroy purge, copy-failure reroute, shutdown drain
  (all signal pre-completion). Requiring the extra backend machinery above:
  deferred-acquire/msc-parked normal completion, and direct-scanout accept. Do not
  list the latter two as ready until that machinery exists — or assert pointer identity
  at acceptance instead and say plainly that it is a weaker test.
- **Core assertion**: after `FreeSyncobj(X)` + `ImportSyncobj(X)` while parked,
  `Arc::ptr_eq(&original, &pending.wake.release)`.
- **Retention tests**: `RetainPermanent` disconnect preserves both rows;
  `RetainTemporary` preserves until `KillClient(AllTemporary)`;
  `KillClient(syncobj_xid)` destroys the owning zombie's resources; `DestroyAll`
  removes both; a pinned Present wake stays usable after the row is destroyed.
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

- Dropping `Copy` touches ~12 sites, and **four** rely on it implicitly by matching
  `event.wake` through a shared reference — `process_request.rs:8741`, `:10416`,
  `:10724`, `:10744` — each needing `match &event.wake` or `if let ... = &event.wake`.
- A retained `Arc` outlives `FreeSyncobj` until the Present retires — intended Xorg
  semantics, but **unbounded**: the first draft called it "bounded by ≤5 observed
  entries", which is a measurement, not a bound. A client can accumulate parked
  Presents; there is no enforced queue limit.
- Touching the completion path again so soon after #122 — mitigated by this being the
  path #122 introduced, untested in the reuse direction.
