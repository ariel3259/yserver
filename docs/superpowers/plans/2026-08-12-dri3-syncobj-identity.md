# DRI3 syncobj identity: pin the release handle, register the XID

**Branch:** `fix/dri3-syncobj-identity` (follow-up to #122, already merged as `5b730f75`).

Two defects, one theme: a DRI3 syncobj's identity is tracked by **XID** where Xorg
tracks it by **reference**. Neither is reachable by a well-behaved client today, and
neither was reachable in any run we have taken — which is why both need tests rather
than a repro.

## Defect 1 — release identity is re-resolved by XID after accept

`PresentWake::PixmapSynced` carries `release_syncobj: u32`
(`backend/trait_def.rs:239`). The `Arc` is only resolved later, at completion:
`self.dri3_syncobj_handle(*release_syncobj)` at `kms/render/backend.rs:1198` and
`:19450`. Between **accept** and **completion** a Present can sit parked (deferred
acquire, or msc-parked), and in that window a client may `FreeSyncobj(X)` and
`ImportSyncobj(X)` a *different* object. We then signal the replacement: the real
waiter never gets its release (hang) and an unrelated object gets a spurious signal
(premature buffer reuse).

The acquire path already does the right thing — `dri3_syncobjs.get(..).map(|(_, arc)|
arc.clone())` at `backend.rs:13188-13196` pins the `Arc` at accept. Release is the
outlier, so this reads as an oversight rather than a design choice.

Xorg is immune two ways: `dri3_import_syncobj` ends in `AddResource(...)`
(`dri3/dri3_screen.c:294`), and Present takes a reference on both syncobjs when it
accepts the request.

## Defect 2 — the syncobj XID is not in the server's XID namespace

`ImportSyncobj` checks `resources.xid_in_use` (8 core tables) and then stores the
syncobj only in the backend map. Nothing registers the XID, so:

- `ImportSyncobj(X)` then `CreatePixmap(X)` succeeds; Xorg returns `BadIDChoice`.
- `ServerState::used_xids_in` (`server.rs:1695`, ten extension namespaces) omits
  syncobjs, so **XC-MISC `GetXIDRange` can hand out an XID a live syncobj owns**.

The PR's own test only covers pixmap-then-syncobj (`process_request.rs:37304`), the
direction that already works.

**The two defects are linked.** xcb allocates XIDs monotonically, so a client does not
recycle its own — measured: two mpv swapchains reused the same *offsets* only because
they were different clients with different bases. The realistic path to reuse within a
client is XC-MISC, which is exactly what defect 2 leaves blind. Fixing 2 makes 1
unreachable; fixing 1 makes it impossible.

## Design

1. **Carry the handle.** `PresentWake::PixmapSynced { release: Arc<dyn SyncobjHandle>,
   release_syncobj: u32, release_value: u64 }` — the XID retained for logging only.
   Drop `Copy` from `PresentWake`, keep `Clone`. `CompletedPresentEvent` is already
   `Debug, Clone` (`trait_def.rs:187`), so the ~91 sites touching it do not ripple;
   the blast radius is the ~12 direct `PresentWake` sites.
2. **Resolve once, at accept.** The `PresentPixmapSynced` handler resolves via
   `Backend::dri3_syncobj_handle` (`trait_def.rs:2079`, already exists) and stores the
   `Arc` on the pending Present, so `PendingPresentRequest::wake()` (`server.rs:1735`,
   which has no backend access) can build the wake from stored state.
3. **Delete the late lookups** at `backend.rs:1198` and `:19450`; use the carried
   handle. Invariant to state in code: *after accept, a Present never resolves its
   synchronization objects by XID again.* Supersession, copy-failure, teardown and
   shutdown all route through the wake, so they inherit it.
4. **Free may then be immediate.** `FreeSyncobj` drops the registry entry while any
   in-flight Present keeps the object alive through its `Arc` — matching Xorg.
5. **Register the XID core-side.** Mirror the backend map with a core-side
   `HashMap<u32, ClientId>` in `ServerState` (the eleventh such extension namespace),
   fed by import/free/disconnect, and consulted by `xid_in_use` and `used_xids_in`.

## Not doing

- Full Xorg resource-type machinery for syncobjs. #122's declared divergence stands;
  this only closes the client-visible part of it.
- The eventfd-fallback counter (the latch in `9f0d958d` covers the spam).

## Validation

- **Regression test for defect 1** (the point of the change, no KMS needed): accept a
  synced Present against release XID `X` while parked, `FreeSyncobj(X)`,
  `ImportSyncobj(X)` a second object, then drive the Present to completion and assert
  the **first** object's timeline advanced and the second is untouched. `RecordingBackend`
  already carries a dummy `SyncobjHandle` (`backend/recording.rs:280`).
- **Tests for defect 2**: syncobj-then-pixmap is `BadIDChoice` (the untested
  direction), and `used_xids_in` includes a live syncobj XID.
- Existing suite (2257) stays green; `cargo +nightly fmt`, `cargo clippy --all-targets`.
- One mpv `--gpu-api=vulkan` synced run to confirm no behaviour change: imports and
  frees still pair, no new warnings.

## Risks

- **Dropping `Copy`** is the only wide edit. Mechanical, and the compiler finds every
  site.
- **A retained `Arc` extends syncobj lifetime** past `FreeSyncobj` until the Present
  retires. That is the intended Xorg semantics, but it means a stuck Present pins a
  kernel syncobj handle; bounded by the existing pending-Present lifecycle, which the
  timeline work measured at ≤5 entries.
- Touching the Present completion path again so soon after #122 — but this is the
  path #122 introduced, and it is untested in the reuse direction.
