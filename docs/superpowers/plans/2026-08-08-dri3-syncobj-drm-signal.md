# DRI3 1.4 syncobj via DRM signalling — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Serve DRI3 1.4 syncobjs through DRM ioctls on every driver, so the
NVIDIA driver blacklist that caps DRI3 at 1.3 can be deleted.

**Architecture:** A DRI3 syncobj stops being a Vulkan timeline semaphore and
becomes a process-local DRM syncobj handle. Signal moves from
`vkSignalSemaphore` to `DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL`, readiness polling
from `vkGetSemaphoreCounterValue` to `DRM_IOCTL_SYNCOBJ_QUERY`; the acquire
eventfd path was already DRM and is untouched. `OwnedSemaphore` keeps its
Vulkan body and serves only the XSync fence half, which genuinely needs it.
Because a DRI3 syncobj is a DRM object, **every syncobj ioctl and the
capability query issue on the render node** — the fd DRI3 hands clients — so
`PlatformBackend` retains a `Device` over it.

**Tech Stack:** Rust, `drm` 0.15 (`syncobj_timeline_signal`,
`syncobj_timeline_query`, `syncobj_eventfd`, `fd_to_syncobj`,
`DriverCapability::TimelineSyncObj`), `ash` (only where it already was),
IGT GPU Tools (`syncobj_*` tests) on the validation box for hardware DRM
validation.

**Spec:** `docs/superpowers/specs/2026-08-08-dri3-syncobj-drm-signal-design.md`

**Revision:** revised 2026-08-09 after a third adversarial review round run
against the DRM / DRI3 1.4 / Present 1.4 documentation and the branch. Three
blocking findings from that review are fixed here:

1. **Render-node invariant enforced.** The previous draft ran every syncobj
   ioctl on the KMS node (`PlatformBackend::device`) while querying the
   capability on the render node — the spec's "advertises 1.4 on the strength
   of one device and then operates another" failure row. `PlatformBackend`
   now retains a `Device` over the render node (spec: *"if that means
   PlatformBackend has to retain a Device for the render node rather than an
   fd, that is part of this change"*) and both the capability and the ioctls
   use it (Task 2, Task 5).
2. **The spec's protocol-conformance scope decision is now implemented.**
   The spec scoped in an owning client plus Xorg error semantics
   ("the minimum that makes DRI3 1.4 safe to advertise"): XID validation,
   `BadValue` on a missing fd, `FreeSyncobj` ownership, `PresentPixmapSynced`
   Value errors, and a disconnect purge. That was missing entirely from the
   previous draft; it is now Task 3.
3. **Task 5's hardware validation is now evidence-producing.** It runs the
   full-Mesa session with the spec's two mandatory env overrides (a bare
   `just yserver-mate-hw` now launches the mismatched KMS-AMD/Vulkan-NVIDIA
   pair and proves nothing), gates on a non-zero deferred count and no
   fallback warning, and adds IGT GPU Tools (`syncobj_*` tests) as the
   kernel-level DRM validation.
4. **Divergence from Xorg declared and applied to error codes.** The spec now
   states explicitly that yserver does not adopt Xorg's resource-type
   machinery (syncobjs stay a backend `HashMap` + owner field — HLD non-goal)
   and does not replicate Xorg's error codes for `ImportSyncobj` /
   `FreeSyncobj` (BadIDChoice / BadAccess). Only `PresentPixmapSynced`'s
   Value errors are protocol (presentproto 1.4). Task 3 uses yserver's own
   codes: `BadAlloc` for import failures (xid invalid, missing fd, import
   error), `BadValue` for free failures (unknown or not the owner). The
   spec's "model it as a real resource rather than weakening the table"
   escape hatch is gone.

Task boundaries changed again: the conformance work is a separate task (3)
because it is independently reviewable (registry ownership + error semantics
are observable protocol behaviour), but its registry type change lands in the
same commit as the handlers that consume it.

## Global Constraints

- Branch: `dri3-syncobj-drm-signal` (already created off `master`).
- **Every `file:line` in this plan was verified against THIS branch.** The
  first draft cited `docs/status.md` lines read while checked out on
  `glx-extension-string-terminator`, where that file is longer, and all three
  citations were wrong. If you check out another branch mid-task, re-verify
  before trusting a number.
- Format with `cargo +nightly fmt` before every commit.
- Lint exactly as CI does: `cargo clippy --all-targets -- -D warnings`. CI
  fails on any warning and `--all-targets` lints test code too. Several steps
  below exist solely because a symbol goes dead and would trip this.
- No new ioctl plumbing is introduced — every ioctl used here already exists in
  the `drm` crate, so the AGENTS.md `libc::Ioctl` portability rule is not
  triggered. If you find yourself writing a raw `ioctl` call, stop.
- **The syncobj device is the render node, and it is one device, decided.**
  Every `ImportedSyncobj` and the `DRM_CAP_SYNCOBJ_TIMELINE` query use
  `PlatformBackend::render_node_device` — never `PlatformBackend::device`
  (the KMS node). On split-device boxes (Pi 4 vc4/v3d, Asahi) the two answer
  different questions. The capability is cached at init; the ioctls run
  per-request on the same retained device.
- **Deliberate divergence from Xorg (see spec § "Divergence from Xorg").**
  Error codes for `ImportSyncobj` / `FreeSyncobj` are yserver's own —
  `BadAlloc` for any import failure, `BadValue` for any free failure. Xorg's
  BadIDChoice / BadAccess distinction is NOT replicated (no client branches
  on it; HLD non-goal "preserving behavior that exists only because of Xorg
  implementation accidents"). Only `PresentPixmapSynced`'s Value errors are
  protocol-mandated. Do not "fix" these to match Xorg during implementation.
- `docs/status.md` must be updated (Task 6). AGENTS.md requires it current.
- Do not fix the freed-syncobj bookkeeping bug this change makes reachable
  (`docs/status.md:548`), and do not fix the fail-open arm in
  `PendingPresentSourceWait::is_ready`. Both are recorded in the spec's Risks
  section as out of scope.
- The retain-mode divergence in Task 3's disconnect purge (a
  `RetainPermanent` client's syncobjs are purged, where Xorg would keep them)
  is a documented divergence, not a bug to fix here — see Task 3 Step 7.

---

### Task 1: `ImportedSyncobj` — the DRM-backed syncobj resource

**Files:**
- Modify: `crates/yserver/src/drm/device.rs` (add `open_render_node`)
- Create: `crates/yserver/src/kms/render/imported_syncobj.rs`
- Modify: `crates/yserver/src/kms/render/mod.rs` (add the module at line 18,
  keeping alphabetical order — it sorts between `glyph_pixels` and
  `owned_semaphore`)

**Interfaces:**
- Consumes: `crate::drm::Device`.
- Produces: `Device::open_render_node(&str) -> io::Result<Device>`;
  `ImportedSyncobj::import(Arc<crate::drm::Device>, BorrowedFd) -> io::Result<Self>`,
  `.timeline_value() -> io::Result<u64>`, `.signaled_eventfd(u64) ->
  io::Result<OwnedFd>`, and an impl of `yserver_core::backend::SyncobjHandle`
  whose `signal(u64)` uses `syncobj_timeline_signal`. Tasks 2, 3 and 5 depend on
  these exact names.

- [ ] **Step 1: Add a master-free constructor**

`Device::open` (`crates/yserver/src/drm/device.rs:41`) calls
`acquire_master_lock()` (`:51`) and `enable_atomic_capabilities()` (`:57`) and
propagates with `?`. `DRM_IOCTL_SET_MASTER` returns `EACCES` on a render node —
`drm_ioctl_permit` rejects any non-`DRM_RENDER_ALLOW` ioctl from a render client
(drm-uapi.md render-node section) — and also on `card0` under a live session.
There is no way to build a `crate::drm::Device` over a render node today, so the
tests below cannot exist without this.

Add to `impl Device`, after `for_tests()` (`:30`):

```rust
    /// Open a render node without taking DRM master.
    ///
    /// `open` is the KMS-master constructor: it calls `acquire_master_lock`
    /// and `enable_atomic_capabilities`, both of which a render node rejects
    /// (`DRM_IOCTL_SET_MASTER` → `EACCES` from a render client). Render nodes
    /// still serve the `DRM_RENDER_ALLOW` ioctls, which is everything the
    /// syncobj paths need.
    pub fn open_render_node(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| open_error(path, &err))?;
        Ok(Self {
            file,
            path: path.to_string(),
        })
    }
```

- [ ] **Step 2: Write the failing tests**

Create `crates/yserver/src/kms/render/imported_syncobj.rs` with only the test
module for now:

Note `pub(crate)` on both the module and `render_node()`: Tasks 2, 3 and 5 reuse
this helper rather than each hardcoding a node path.

```rust
#[cfg(test)]
pub(crate) mod tests {
    use std::{os::fd::AsFd, sync::Arc};

    use ::drm::control::Device as DrmControlDevice;

    use super::*;

    /// Open a render node, or skip. Every test here needs real DRM ioctls;
    /// they are `#[ignore]` so CI never runs them, but a machine without a
    /// node should skip rather than fail.
    ///
    /// Do NOT hardcode `/dev/dri/renderD128`. `kms/render_node.rs:1-8` states
    /// the rule outright — "we deliberately do **not** hardcode
    /// `/dev/dri/renderD128` — on multi-GPU hosts that selects the wrong
    /// device" — and the nvidia box became exactly such a host on 2026-08-08:
    /// `renderD128` is nvidia-drm and `renderD129` is the Raphael iGPU. A
    /// hardcoded 128 would make a run intended to validate Mesa silently
    /// exercise nvidia-drm and report green.
    ///
    /// Honour `YSERVER_TEST_RENDER_NODE` so a Mesa run can be directed at the
    /// amdgpu node, and enumerate otherwise rather than guessing.
    pub(crate) fn render_node() -> Option<Arc<crate::drm::Device>> {
        if let Ok(path) = std::env::var("YSERVER_TEST_RENDER_NODE") {
            return crate::drm::Device::open_render_node(&path).ok().map(Arc::new);
        }
        let mut paths: Vec<_> = std::fs::read_dir("/dev/dri")
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("renderD"))
            })
            .collect();
        paths.sort();
        paths
            .iter()
            .find_map(|p| crate::drm::Device::open_render_node(p.to_str()?).ok())
            .map(Arc::new)
    }

    /// Full round trip mirroring the server's sequence: the client exports a
    /// syncobj fd, the server imports it, signals a release point, and the
    /// client's own handle observes it through its own separate handle.
    /// Run with `cargo test -p yserver --lib imported_syncobj -- --ignored`.
    #[test]
    #[ignore = "needs a DRM render node"]
    fn signal_reaches_the_clients_handle() {
        let Some(drm) = render_node() else {
            eprintln!("skipping: no render node");
            return;
        };

        let client_handle = drm.create_syncobj(false).expect("create syncobj");
        let fd = drm.syncobj_to_fd(client_handle, false).expect("export fd");

        let imported = ImportedSyncobj::import(drm.clone(), fd.as_fd()).expect("import");
        assert_eq!(imported.timeline_value().expect("query"), 0);

        yserver_core::backend::SyncobjHandle::signal(&imported, 7).expect("signal");

        // The client must observe the release through ITS handle, not the
        // server's, or the two are not aliasing one payload and the client
        // would wait forever.
        let mut points = [0u64; 1];
        drm.syncobj_timeline_query(&[client_handle], &mut points, false)
            .expect("client query");
        assert_eq!(points[0], 7, "server signal did not reach the client handle");

        drm.destroy_syncobj(client_handle).expect("destroy");
    }

    /// The acquire path's kernel notification.
    #[test]
    #[ignore = "needs a DRM render node"]
    fn eventfd_fires_on_the_registered_point() {
        let Some(drm) = render_node() else {
            eprintln!("skipping: no render node");
            return;
        };
        let client_handle = drm.create_syncobj(false).expect("create syncobj");
        let fd = drm.syncobj_to_fd(client_handle, false).expect("export fd");
        let imported = ImportedSyncobj::import(drm.clone(), fd.as_fd()).expect("import");

        let event = imported.signaled_eventfd(9).expect("register eventfd");
        let mut buf = [0u8; 8];
        assert!(
            nix::unistd::read(event.as_fd(), &mut buf).is_err(),
            "eventfd readable before the point was signalled",
        );

        yserver_core::backend::SyncobjHandle::signal(&imported, 9).expect("signal");
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            nix::unistd::read(event.as_fd(), &mut buf).is_ok(),
            "eventfd never fired after the point was signalled",
        );

        drm.destroy_syncobj(client_handle).expect("destroy");
    }

    /// Documents measured kernel behaviour the spec depends on: a stale or
    /// duplicate timeline point is CLAMPED and returns success, it is not
    /// rejected. Release replay after teardown therefore cannot be detected
    /// by checking the signal's return value.
    #[test]
    #[ignore = "needs a DRM render node"]
    fn a_stale_point_is_clamped_not_rejected() {
        use yserver_core::backend::SyncobjHandle as _;

        let Some(drm) = render_node() else {
            eprintln!("skipping: no render node");
            return;
        };
        let client_handle = drm.create_syncobj(false).expect("create syncobj");
        let fd = drm.syncobj_to_fd(client_handle, false).expect("export fd");
        let imported = ImportedSyncobj::import(drm.clone(), fd.as_fd()).expect("import");

        imported.signal(10).expect("signal 10");
        imported.signal(5).expect("a stale point must still return Ok");
        assert_eq!(
            imported.timeline_value().expect("query"),
            10,
            "the kernel must clamp to the max, not regress the timeline",
        );

        drm.destroy_syncobj(client_handle).expect("destroy");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p yserver --lib imported_syncobj -- --ignored`
Expected: compile error — `ImportedSyncobj` is not defined and the module is
not registered.

- [ ] **Step 4: Register the module**

In `crates/yserver/src/kms/render/mod.rs`, insert at line 18 (before
`pub(crate) mod owned_semaphore;`):

```rust
pub(crate) mod imported_syncobj;
```

- [ ] **Step 5: Write the implementation**

Prepend to `crates/yserver/src/kms/render/imported_syncobj.rs`:

```rust
//! A DRI3 1.4 syncobj imported from a client fd, held as a process-local
//! DRM syncobj handle.
//!
//! This deliberately has no Vulkan in it. A DRM syncobj is a kernel object
//! and every operation the server needs — signal, query, eventfd — has a DRM
//! ioctl. Importing it into a `VkSemaphore` instead only works where the
//! driver's `OPAQUE_FD` payload happens to be a DRM syncobj, which is true on
//! Mesa and false on NVIDIA proprietary
//! (`vkImportSemaphoreFdKHR` → `VK_ERROR_INITIALIZATION_FAILED`). See
//! docs/superpowers/specs/2026-08-08-dri3-syncobj-drm-signal-design.md.
//!
//! The sibling `OwnedSemaphore` keeps the Vulkan path for XSync `Fence`
//! resources, which need a real `VkSemaphore` for `FDFromFence`'s sync_file
//! export.
//!
//! The `Arc<crate::drm::Device>` here MUST be the render node — the device
//! DRI3 hands the client (`PlatformBackend::render_node_device`), never the
//! KMS node. See the spec's "Which fd to ask" section.

use std::{
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    sync::Arc,
};

use ::drm::control::{Device as DrmControlDevice, syncobj};

pub(crate) struct ImportedSyncobj {
    drm: Arc<crate::drm::Device>,
    handle: syncobj::Handle,
}

impl ImportedSyncobj {
    /// Import a client's `DRM_SYNCOBJ` fd as a process-local handle. The fd is
    /// only borrowed — `DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE` does not consume it —
    /// so the caller keeps ownership and drops it normally. Importing a
    /// syncobj fd creates a NEW handle (with its own reference) for every
    /// import; the underlying `struct drm_syncobj` is shared, which is what
    /// lets a server-side signal reach the client's handle.
    pub(crate) fn import(
        drm: Arc<crate::drm::Device>,
        fd: BorrowedFd<'_>,
    ) -> std::io::Result<Self> {
        let handle = drm.fd_to_syncobj(fd, false)?;
        Ok(Self { drm, handle })
    }

    /// Current timeline value. Replaces `vkGetSemaphoreCounterValue` in the
    /// deferred-acquire polling fallback.
    pub(crate) fn timeline_value(&self) -> std::io::Result<u64> {
        let mut points = [0u64; 1];
        self.drm
            .syncobj_timeline_query(&[self.handle], &mut points, false)?;
        Ok(points[0])
    }

    /// Register a non-blocking kernel notification for a timeline point.
    /// Unchanged in behaviour from the previous `OwnedSemaphore` version —
    /// that method already went through DRM.
    pub(crate) fn signaled_eventfd(&self, value: u64) -> std::io::Result<OwnedFd> {
        use nix::sys::eventfd::{EfdFlags, EventFd};

        let event =
            EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
                .map_err(|e| std::io::Error::other(format!("eventfd: {e}")))?;
        self.drm
            .syncobj_eventfd(self.handle, value, event.as_fd(), false)?;
        Ok(event.into())
    }
}

impl std::fmt::Debug for ImportedSyncobj {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedSyncobj")
            .field("handle", &self.handle)
            .finish_non_exhaustive()
    }
}

impl Drop for ImportedSyncobj {
    fn drop(&mut self) {
        if let Err(e) = self.drm.destroy_syncobj(self.handle) {
            log::warn!("destroy imported DRM syncobj handle failed: {e}");
        }
    }
}

impl yserver_core::backend::SyncobjHandle for ImportedSyncobj {
    /// Host-signal a timeline point. Replaces `vkSignalSemaphore`, which was
    /// also a host operation.
    ///
    /// Note the kernel CLAMPS: signalling a point at or below the current
    /// value succeeds silently and leaves the timeline where it was. Callers
    /// cannot use the return value to detect an out-of-order release.
    fn signal(&self, value: u64) -> std::io::Result<()> {
        self.drm.syncobj_timeline_signal(&[self.handle], &[value])
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p yserver --lib imported_syncobj -- --ignored`
Expected: 3 passed. If they skip instead, no `/dev/dri/renderD*` could be
opened — check `ls -l /dev/dri/` and the `render` group membership.

On a multi-GPU box, run it once per driver and say which is which, because
the enumeration picks the lexicographically first node that opens and that
choice is not stable across machines:

```bash
YSERVER_TEST_RENDER_NODE=/dev/dri/renderD128 cargo test -p yserver --lib imported_syncobj -- --ignored  # nvidia-drm
YSERVER_TEST_RENDER_NODE=/dev/dri/renderD129 cargo test -p yserver --lib imported_syncobj -- --ignored  # amdgpu
```

Both passing is the cross-driver evidence for the ioctl layer specifically;
it is **not** a substitute for the Mesa client run in the spec's Testing §4,
which is what exercises the release path against a real waiter.

- [ ] **Step 7: Format, lint, commit**

**This task cannot pass `-D warnings` on its own without help.** Task 1 lands
`ImportedSyncobj` with no non-test caller — Task 2 is what wires it into the
registry. `cargo clippy --all-targets` builds the lib target *without*
`cfg(test)`, where the type and its methods are constructed nowhere, so
`dead_code` fires and `-D warnings` turns it into a failure. The test module
does not rescue it: the tests satisfy the test target, not the lib target.

Two ways out; take the first.

1. **Merge the gate into Task 2** — run `-D warnings` only at the end of
   Task 2 and commit Task 1 behind `cargo clippy --all-targets` without
   `-D warnings`, reading the output by eye. Honest about the intermediate
   state and adds nothing to remove later.
2. If Task 1 must stand alone as a green commit, put
   `#![cfg_attr(test, allow(dead_code))]`-style scoping on the module —
   specifically `#[allow(dead_code)]` on `ImportedSyncobj` and its impl block,
   **with a `TODO(task-2)` comment** — and delete the attribute in Task 2. A
   lingering blanket `allow(dead_code)` in this module would later hide a
   genuinely unused method, so its removal is part of Task 2's checklist, not
   optional cleanup.

Whichever is chosen, Task 2's Step 7 must run the full
`cargo clippy --all-targets -- -D warnings` with no allowances left behind.

```bash
cargo +nightly fmt
cargo clippy --all-targets   # see the note above re: -D warnings
git add crates/yserver/src/drm/device.rs crates/yserver/src/kms/render/imported_syncobj.rs crates/yserver/src/kms/render/mod.rs
git commit -m "feat(dri3): add ImportedSyncobj, a DRM-backed syncobj resource

A DRI3 syncobj is a kernel object and every operation the server needs on
one -- signal, query, eventfd -- has a DRM ioctl. Importing it into a
VkSemaphore only works where the driver's OPAQUE_FD payload happens to be
a DRM syncobj, which is false on NVIDIA proprietary.

Device::open_render_node comes along because Device::open takes DRM
master, which a render node refuses, so there was no way to reach these
ioctls from a test.

Not wired up yet; the registries still hold OwnedSemaphore."
```

---

### Task 2: Move syncobjs onto the DRM resource, registry and acquire path together

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs` — field at `:805`, both
  initialisers at `:2334` and `:3222`, acquire lookup at `:13166`,
  `dri3_syncobj_handle` `:19291`, `dri3_fd_from_fence` `:19301`,
  `dri3_import_syncobj` `:19313`, `dri3_free_syncobj` `:19377`,
  `dri3_signal_syncobj` `:19384`, and the test at `:30010`
- Modify: `crates/yserver/src/kms/render/platform.rs` (retain a render-node
  `Device`, alongside `render_node_fd` at `:565`)
- Modify: `crates/yserver/src/kms/render/present_source_wait.rs:19-24,40-52`

**Interfaces:**
- Consumes: everything Task 1 produced.
- Produces: `PlatformBackend::render_node_device:
  Option<Arc<crate::drm::Device>>`; field `dri3_syncobjs: HashMap<u32,
  Arc<ImportedSyncobj>>` (Task 3 adds the owner); `PendingPresentSourceWait::
  syncobj_pin` retyped to `Option<Arc<ImportedSyncobj>>`. `dri3_sync_resources`
  survives, fences only.

**Why this is one task and not two.** The registry split and the acquire-path
rewrite cannot be separate commits. `arm_present_syncobj_wait`
(`backend.rs:13145`) looks up `dri3_sync_resources` for the acquire syncobj; the
moment `ImportSyncobj` writes elsewhere, that lookup's `ok_or_else` fires and
the error propagates out of the request handler via `?`
(`process_request.rs:10251`). It still compiles and `cargo test --lib` still
passes, so nothing catches it — but on Mesa, where the capability is still
advertised until Task 5, every synced present with an acquire syncobj breaks at
that commit.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `backend.rs`, near the other DRI3
tests (around `:28065`). This one needs a real DRM handle, so it is `#[ignore]`
like Task 1's:

```rust
/// Fences and syncobjs are different X resource types with different
/// backing primitives. Each resolver must see only its own registry: before
/// the split they shared one map, so FDFromFence on a syncobj xid resolved
/// and half-worked.
#[test]
#[ignore = "needs a DRM render node"]
fn each_resolver_sees_only_its_own_registry() {
    // Shared helper from Task 1 — never hardcode renderD128, see its doc
    // comment for why (multi-GPU hosts pick the wrong device silently).
    let Some(drm) = crate::kms::render::imported_syncobj::tests::render_node() else {
        eprintln!("skipping: no render node");
        return;
    };
    let handle = ::drm::control::Device::create_syncobj(drm.as_ref(), false)
        .expect("create syncobj");
    let fd = ::drm::control::Device::syncobj_to_fd(drm.as_ref(), handle, false)
        .expect("export fd");

    let mut b = KmsBackend::for_tests();
    let xid = 0xAAAA_BBBB_u32;
    b.dri3_syncobjs.insert(
        xid,
        std::sync::Arc::new(
            crate::kms::render::imported_syncobj::ImportedSyncobj::import(
                drm.clone(),
                std::os::fd::AsFd::as_fd(&fd),
            )
            .expect("import"),
        ),
    );

    // The syncobj resolver finds it.
    assert!(
        b.dri3_syncobj_handle(xid).is_some(),
        "syncobj registry must resolve a syncobj xid",
    );
    // The fence resolver must NOT, and must say so as an unknown fence
    // rather than tripping over some other gate first.
    let err = b
        .dri3_fd_from_fence(xid)
        .expect_err("FDFromFence must not resolve a syncobj xid");
    assert!(
        err.to_string().contains("unknown fence"),
        "expected an unknown-fence error, got: {err}",
    );

    ::drm::control::Device::destroy_syncobj(drm.as_ref(), handle).expect("destroy");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p yserver --lib each_resolver_sees_only -- --ignored`
Expected: compile error — `dri3_syncobjs` does not exist.

- [ ] **Step 3: Retain a render-node `Device` in `PlatformBackend`**

The spec's render-node invariant requires every syncobj ioctl and the
capability query to issue on the render node — the device DRI3 hands clients.
`PlatformBackend` today keeps only a bare `render_node_fd` (`platform.rs:565`);
`dri3_import_syncobj` previously reached for `self.platform.device` (the **KMS
node**) out of convenience, which is the exact failure the spec calls out.

In `crates/yserver/src/kms/render/platform.rs`, next to `render_node_fd`
(`:565`), add:

```rust
    /// The DRM device over the render node, retained so every DRI3 syncobj
    /// ioctl and the `DRM_CAP_SYNCOBJ_TIMELINE` query issue on the SAME fd
    /// kind DRI3 hands clients. This is deliberately NOT `device` (the KMS
    /// node): on split-device boxes (Pi 4 vc4/v3d, Asahi) the display device's
    /// answer says nothing about the render device's. See the spec's "Which
    /// fd to ask — one device, decided, not preferred".
    pub(crate) render_node_device: Option<Arc<crate::drm::Device>>,
```

Populate it in `from_platform_init` (`platform.rs:760`) where `render_node_fd`
is already destructured from `PlatformInit` — the path is available as
`render_node_path`:

```rust
        let render_node_device = render_node_path
            .as_deref()
            .and_then(|path| {
                // `open_render_node` takes a `&str`; a non-UTF8 node path
                // (unrealistic under /dev/dri) degrades to no device, which
                // `dri3_import_syncobj` surfaces as a hard error.
                drm::Device::open_render_node(path.to_str()?).ok()
            })
            .map(Arc::new);
```

Add `render_node_device: None,` to `PlatformBackend::for_tests()` (`:1007`,
next to `render_node_fd: None`). Missing it is a compile error, not a silent
bug. Note this opens a **second** fd to the render node (alongside
`render_node_fd`); that matches the existing design — `dri3_open` already opens
a fresh fd per request via `render_node::open_fresh` (`backend.rs:18963`) — and
syncobj handles are per-`drm_file` but the underlying `struct drm_syncobj` is
shared across fds of the same device, which is what makes a server-side signal
reach the client.

- [ ] **Step 4: Add the new registry field**

In `backend.rs`, narrow the existing doc comment on `dri3_sync_resources`
(`:800-804`) to fences only and add the new field after it:

```rust
    /// DRI3 sync-fence resources keyed by the client's xid, from
    /// `FenceFromFD` falling through the xshmfence path (sync_file fd →
    /// `VkSemaphore`). Syncobjs live in `dri3_syncobjs`.
    pub(crate) dri3_sync_resources:
        HashMap<u32, std::sync::Arc<crate::kms::render::owned_semaphore::OwnedSemaphore>>,
    /// DRI3 1.4 syncobj resources keyed by the client's xid, imported by
    /// `ImportSyncobj`. Separate from `dri3_sync_resources` because these are
    /// a different X resource type with a different backing primitive: a DRM
    /// syncobj handle, not a `VkSemaphore`. Task 3 adds the owning client.
    pub(crate) dri3_syncobjs:
        HashMap<u32, std::sync::Arc<crate::kms::render::imported_syncobj::ImportedSyncobj>>,
```

Add `dri3_syncobjs: HashMap::new(),` next to `dri3_sync_resources:
HashMap::new(),` in **both** initialisers (`:2334` and `:3222`). Missing the
second is a compile error, not a silent bug.

- [ ] **Step 5: Rewire the four syncobj methods**

Replace `dri3_import_syncobj` (`:19313-19375`). The `dup`, the Vulkan gate, the
import and the rollback all go away — `fd_to_syncobj` borrows the fd and there
is nothing left to roll back. The device is the **render node**, not
`self.platform.device`:

```rust
    fn dri3_import_syncobj(
        &mut self,
        syncobj_xid: u32,
        fd: std::os::fd::OwnedFd,
    ) -> io::Result<()> {
        use std::os::fd::AsFd;

        let render_node = self.platform.render_node_device.as_ref().cloned().ok_or_else(
            || io::Error::other("DRI3 ImportSyncobj: render node not resolved at init"),
        )?;
        let imported = crate::kms::render::imported_syncobj::ImportedSyncobj::import(
            render_node,
            fd.as_fd(),
        )?;
        // Arc Drop on any replaced entry destroys the previous handle.
        let _ = self
            .dri3_syncobjs
            .insert(syncobj_xid, std::sync::Arc::new(imported));
        Ok(())
    }
```

Replace `dri3_free_syncobj` (`:19377-19383`):

```rust
    fn dri3_free_syncobj(&mut self, syncobj_xid: u32) -> io::Result<()> {
        // Arc Drop destroys the DRM handle when the last reference goes away,
        // which may be later than this call: the deferred completion path
        // pins clones past FreeSyncobj.
        let _ = self.dri3_syncobjs.remove(&syncobj_xid);
        Ok(())
    }
```

Replace `dri3_signal_syncobj` (`:19384-19392`):

```rust
    fn dri3_signal_syncobj(&mut self, syncobj_xid: u32, value: u64) -> io::Result<()> {
        use yserver_core::backend::SyncobjHandle as _;

        let arc = self.dri3_syncobjs.get(&syncobj_xid).ok_or_else(|| {
            io::Error::other(format!(
                "DRI3 SignalSyncobj: unknown syncobj 0x{syncobj_xid:x}"
            ))
        })?;
        arc.signal(value)
    }
```

Replace `dri3_syncobj_handle` (`:19291-19299`):

```rust
    fn dri3_syncobj_handle(
        &self,
        syncobj_xid: u32,
    ) -> Option<std::sync::Arc<dyn yserver_core::backend::SyncobjHandle>> {
        self.dri3_syncobjs
            .get(&syncobj_xid)
            .cloned()
            .map(|arc| arc as std::sync::Arc<dyn yserver_core::backend::SyncobjHandle>)
    }
```

- [ ] **Step 6: Move the registry lookup ahead of the Vulkan gate**

In `dri3_fd_from_fence` (`:19301-19311`) the lookup must come first, so an
unknown xid reports as unknown instead of as a Vulkan failure:

```rust
    fn dri3_fd_from_fence(&mut self, fence_xid: u32) -> io::Result<std::os::fd::OwnedFd> {
        let arc = self
            .dri3_sync_resources
            .get(&fence_xid)
            .cloned()
            .ok_or_else(|| {
                io::Error::other(format!("DRI3 FDFromFence: unknown fence 0x{fence_xid:x}"))
            })?;
        let Some(vk) = self.platform.vk.as_ref() else {
            return Err(io::Error::other("DRI3 FDFromFence: Vulkan unavailable"));
        };
        crate::kms::vk::sync::export_sync_file(vk, arc.semaphore())
            .map_err(|e| io::Error::other(format!("export_sync_file: {e:?}")))
    }
```

- [ ] **Step 7: Repoint the acquire path (this is what keeps the tree working)**

In `arm_present_syncobj_wait`, the block starting at `:13162`:

```rust
            let syncobj = self
                .dri3_syncobjs
                .get(&acquire_syncobj)
                .cloned()
                .ok_or_else(|| {
                    io::Error::other(format!(
                        "PresentPixmapSynced: unknown acquire syncobj 0x{acquire_syncobj:x}"
                    ))
                })?;
            let event_fd = match syncobj.signaled_eventfd(acquire_value) {
                Ok(fd) => Some(fd),
                Err(e) => {
                    log::warn!(
                        target: "yserver::kms::render::present",
                        "PresentPixmapSynced DRM eventfd unavailable ({e}); polling the syncobj timeline",
                    );
                    None
                }
            };
```

- [ ] **Step 8: Retype the pin and its readiness check**

In `present_source_wait.rs`, replace lines 19-24 (the two fields and their doc
comments — **do not touch `poll_timeline: bool` at `:25`, it is still used** at
`backend.rs:12595/13186/13222`):

```rust
    /// Keeps an explicitly imported acquire syncobj alive until its timeline
    /// point signals. Implicit dma-buf waits leave this empty.
    pub(crate) syncobj_pin: Option<Arc<super::imported_syncobj::ImportedSyncobj>>,
    /// Target acquire point. Set whenever an acquire syncobj is present —
    /// note `is_ready` checks it on every poll, not only when
    /// `DRM_SYNCOBJ_EVENTFD` was unavailable.
    pub(crate) timeline_value: Option<u64>,
```

Replace the `is_ready` timeline arm (lines 40-52):

```rust
        let timeline_ready = match (&self.syncobj_pin, self.timeline_value) {
            (Some(syncobj), Some(target)) => match syncobj.timeline_value() {
                Ok(current) => current >= target,
                Err(e) => {
                    log::warn!(
                        "deferred Present acquire: syncobj timeline query failed: {e}; \
                         treating as ready"
                    );
                    true
                }
            },
            _ => true,
        };
```

Leave the fail-open `true` alone — the spec records it as a known hazard that
this change makes live, and fixing it is explicitly out of scope.

- [ ] **Step 9: Fix the two existing tests this task breaks**

`syncobj_handle_accessor_returns_arc_clone` (`backend.rs:30010-30029`) inserts
an `OwnedSemaphore` into `dri3_sync_resources` and asserts
`dri3_syncobj_handle` finds it. After Step 5 it returns `None` and the test
panics — and it still compiles, so `cargo test --lib dri3_` would not even
match its name. Rewrite it against the new registry:

```rust
    #[test]
    #[ignore = "needs a DRM render node"]
    fn syncobj_handle_accessor_returns_arc_clone() {
        // Shared helper from Task 1 — never hardcode renderD128.
        let Some(drm) = crate::kms::render::imported_syncobj::tests::render_node() else {
            eprintln!("skipping: no render node");
            return;
        };
        let handle = ::drm::control::Device::create_syncobj(drm.as_ref(), false)
            .expect("create syncobj");
        let fd = ::drm::control::Device::syncobj_to_fd(drm.as_ref(), handle, false)
            .expect("export fd");

        let mut b = KmsBackend::for_tests();
        let xid = 0xAAAA_BBBB_u32;
        b.dri3_syncobjs.insert(
            xid,
            std::sync::Arc::new(
                crate::kms::render::imported_syncobj::ImportedSyncobj::import(
                    drm.clone(),
                    std::os::fd::AsFd::as_fd(&fd),
                )
                .expect("import"),
            ),
        );

        let h = b.dri3_syncobj_handle(xid).expect("handle present");
        assert_eq!(std::sync::Arc::strong_count(&h), 2);
        b.dri3_syncobjs.remove(&xid);
        // Accessor returns None now; the held Arc still pins the resource
        // alive, which is what the deferred completion path relies on.
        assert!(b.dri3_syncobj_handle(xid).is_none());
        drop(h);

        ::drm::control::Device::destroy_syncobj(drm.as_ref(), handle).expect("destroy");
    }
```

This test previously ran under `for_tests_with_vk` and now needs a DRM node
instead, so it becomes `#[ignore]`. That is a real coverage loss on CI; it is
unavoidable, because the resource it covers is now a kernel object.

`dri3_import_syncobj_no_vk_errs` (`:28065`) asserts the import Errs without
Vulkan, which is no longer why it errs. Replace it with a version that feeds
`for_tests()` a bogus render-node device so the **ioctl** (not the
missing-device guard) is what fails:

```rust
    /// ImportSyncobj no longer needs Vulkan. for_tests() has no render node,
    /// so feed it a bogus one and assert the ioctl — not the device guard —
    /// is what fails.
    #[test]
    fn dri3_import_syncobj_errs_without_a_usable_drm_handle() {
        use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
        let f = std::fs::OpenOptions::new()
            .read(true)
            .open("/dev/null")
            .expect("open /dev/null");
        // SAFETY: we own this fd via the OpenOptions handle and re-wrap it
        // directly; the OwnedFd's Drop closes it.
        let fd = unsafe { OwnedFd::from_raw_fd(f.into_raw_fd()) };
        let mut b = KmsBackend::for_tests();
        b.platform.render_node_device = Some(std::sync::Arc::new(
            crate::drm::Device::open_render_node("/dev/null").expect("open /dev/null"),
        ));
        assert!(
            b.dri3_import_syncobj(0x4040_3333, fd).is_err(),
            "importing a non-syncobj fd must Err",
        );
    }
```

- [ ] **Step 10: Check whether `for_tests_dummy` still has callers**

`OwnedSemaphore::for_tests_dummy` (`owned_semaphore.rs:80`) existed for the
test just rewritten. Run:

```bash
grep -rn "for_tests_dummy" crates/
```

If nothing remains, delete it — `#[cfg(test)]` dead code still fails
`clippy --all-targets -- -D warnings`.

- [ ] **Step 11: Run the tests**

```bash
cargo test -p yserver --lib                                    # no regressions
cargo test -p yserver --lib -- --ignored                       # the DRM ones
```
Expected: PASS on both.

- [ ] **Step 12: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/render/backend.rs crates/yserver/src/kms/render/platform.rs crates/yserver/src/kms/render/present_source_wait.rs crates/yserver/src/kms/render/owned_semaphore.rs
git commit -m "refactor(dri3): hold syncobjs in their own DRM-backed registry

FenceFromFD and ImportSyncobj registered two different X resource types
into one HashMap of OwnedSemaphore. Only the fence half needs Vulkan
(FDFromFence exports a sync_file from the VkSemaphore); the syncobj half
needs a DRM handle. Split them so the type mismatch is unrepresentable.

PlatformBackend now retains a Device over the render node, and every
syncobj ioctl runs on it — the same fd kind DRI3 hands clients — instead
of the KMS node (which only answers for the display device on split-GPU
boxes).

The acquire path moves in the same commit rather than a later one: it
looks up the same map, so splitting the two would leave every synced
present failing on Mesa at the intermediate commit.

FDFromFence now looks up the registry before the Vulkan gate, so an
unknown xid reports as unknown rather than as a Vulkan failure."
```

---

### Task 3: DRI3 syncobj conformance — owning client and error semantics

The spec scoped this in: *"Give the registry an owning client and error
semantics. That is the minimum that makes DRI3 1.4 safe to advertise: rows 3
and 4 are cross-client and denial-of-service shaped respectively."* The
conformance table (spec § "Protocol conformance") — note rows 1-3 diverge from
Xorg in the **error codes** on purpose (spec § "Divergence from Xorg"): only
row 4 is protocol-mandated:

| # | Contrato que debe cumplirse | Xorg | yserver (este cambio) |
|---|---|---|---|
| 1 | Un xid no legal para el cliente se rechaza (convención core X11) | `LEGAL_NEW_RESOURCE` → BadIDChoice (`dri3/dri3_request.c:609`) | `BadAlloc` — divergencia deliberada |
| 2 | Un request sin fd se rechaza | `fd < 0` → BadValue (`dri3/dri3_request.c:619-620`) | `BadAlloc` — divergencia deliberada |
| 3 | Nadie puede liberar el syncobj de otro | `dixLookupResourceByType(..., DixWriteAccess)` → BadValue/BadAccess (`dri3/dri3_request.c:634-637`) | ownership enforced; `BadValue` único — divergencia deliberada |
| 4 | `PresentPixmapSynced`: syncobj inválido o points ilegales → **Value error** | `VERIFY_DRI3_SYNCOBJ` + BadValue (`present/present_request.c:296-302`) | `BadValue` — protocolo presentproto 1.4 |

**Do not "fix" rows 1-3 to match Xorg's codes during implementation.** The
Global Constraints and the spec's "Divergence from Xorg" section govern.

**Files:**
- Modify: `crates/yserver-core/src/backend/trait_def.rs` — `dri3_import_syncobj`
  (`:2045`) and `dri3_free_syncobj` (`:2054`) gain a `ClientId` parameter
- Modify: `crates/yserver/src/kms/render/backend.rs` — registry type, the four
  syncobj methods, `client_disconnected` (`:14361`), tests
- Modify: `crates/yserver-core/src/backend/recording.rs` — minimal syncobj
  tracking so the wire-level tests can exercise the handlers
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` —
  `IMPORT_SYNCOBJ` (`:11457`), `FREE_SYNCOBJ` (`:11507`), `PIXMAP_SYNCED`
  (`:10067`) handlers + wire tests

**Interfaces:**
- Consumes: `dri3_syncobjs` registry (Task 2), `dri3_syncobj_handle`,
  `ImportedSyncobj`, `ClientId`.
- Produces: registry `dri3_syncobjs: HashMap<u32, (ClientId,
  Arc<ImportedSyncobj>)>`; `dri3_import_syncobj(client_id, xid, fd)` and
  `dri3_free_syncobj(client_id, xid)` (trait signatures change);
  `PresentPixmapSynced` Value-error validation; disconnect purge in
  `client_disconnected`.

- [ ] **Step 1: Write the failing wire-level tests**

Add to the `#[cfg(test)] mod tests` block in `process_request.rs`, next to
`sync_create_trigger_query_fence_round_trip` (`:37092`). `RecordingBackend` is
the backend; the tests build DRI3 request bodies by hand exactly like the
existing fence round-trip test does. `IMPORT_SYNCOBJ` is DRI3 minor opcode 10,
`FREE_SYNCOBJ` 11, DRI3 major opcode 147 (all already in
`yserver_protocol::x11::dri3`).

The tests need `RecordingBackend` to track syncobjs — that is Step 4. To make
them fail first, write them, then run: they fail to compile until Step 4 adds
the overrides.

```rust
    /// Build an ImportSyncobj request body: syncobj(4) + drawable(4), fd via
    /// SCM_RIGHTS (attached_fd).
    fn import_syncobj_body(syncobj: u32) -> Vec<u8> {
        let mut body = vec![0u8; 8];
        body[0..4].copy_from_slice(&syncobj.to_le_bytes());
        body
    }

    fn read_error(buf: &[u8]) -> u8 {
        // X error: type=0, error-code at byte 1.
        assert_eq!(buf[0], 0, "expected an X error, got type {}", buf[0]);
        buf[1]
    }

    /// RecordingBackend with a DRI3 1.4 / `syncobj: true` surface so the
    /// IMPORT_SYNCOBJ / FREE_SYNCOBJ handlers pass their `caps.syncobj` gate.
    /// Kept as a per-test opt-in: the DEFAULT backend must stay
    /// `Dri3Caps::unsupported()` for the existing
    /// `dri3_hidden_when_caps_unsupported` test (process_request.rs:37076).
    fn syncobj_cap_backend() -> RecordingBackend {
        let mut backend = RecordingBackend::new();
        backend.dri3_caps = crate::backend::Dri3Caps {
            version: (1, 4),
            modifiers: false,
            fence_fd: false,
            syncobj: true,
        };
        backend
    }

    #[test]
    fn import_syncobj_in_use_xid_is_bad_alloc() {
        use yserver_protocol::x11::{CreatePixmapRequest, ResourceId};
        let mut state = ServerState::new();
        let mut peer = install_client(&mut state, 1);
        let mut backend = syncobj_cap_backend();

        // install_client (process_request.rs:29047) gives every test client
        // base=0/mask=u32::MAX — "every xid is in range" — so the
        // xid_out_of_client_range half of the LEGAL_NEW_RESOURCE gate is
        // unreachable in this fixture. Exercise the OTHER half: name a
        // pixmap with the xid, then ImportSyncobj with the same xid ->
        // resources.xid_in_use fires.
        const XID: u32 = 0x0040_0001;
        state.resources.create_pixmap(
            ClientId(1),
            CreatePixmapRequest {
                depth: 24,
                pixmap: ResourceId(XID),
                drawable: ROOT_WINDOW,
                width: 1,
                height: 1,
            },
        );

        process_request(
            &mut state,
            &mut backend,
            ClientId(1),
            SequenceNumber(1),
            RequestHeader {
                opcode: 147,
                data: 10, // IMPORT_SYNCOBJ
                length_units: 3,
            },
            &import_syncobj_body(XID),
            None, // no SCM_RIGHTS fd
        )
        .unwrap();
        peer.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 32];
        peer.read_exact(&mut buf).unwrap();
        assert_eq!(
            read_error(&buf),
            x11::error::BAD_ALLOC,
            "an in-use xid must be rejected as BadAlloc",
        );
    }

    #[test]
    fn import_syncobj_missing_fd_is_bad_alloc() {
        let mut state = ServerState::new();
        let mut peer = install_client(&mut state, 1);
        let mut backend = syncobj_cap_backend();

        process_request(
            &mut state,
            &mut backend,
            ClientId(1),
            SequenceNumber(1),
            RequestHeader {
                opcode: 147,
                data: 10, // IMPORT_SYNCOBJ
                length_units: 3,
            },
            &import_syncobj_body(0x0010_0001), // any xid; install_client is range-permissive
            None, // no fd attached
        )
        .unwrap();
        peer.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 32];
        peer.read_exact(&mut buf).unwrap();
        assert_eq!(
            read_error(&buf),
            x11::error::BAD_ALLOC,
            "ImportSyncobj without an fd must be BadAlloc (yserver's own code — not Xorg's BadValue)",
        );
    }

    #[test]
    fn free_syncobj_unknown_is_bad_value() {
        let mut state = ServerState::new();
        let mut peer = install_client(&mut state, 1);
        let mut backend = syncobj_cap_backend();

        let mut body = vec![0u8; 4];
        body[0..4].copy_from_slice(&0x0040_9999u32.to_le_bytes());
        process_request(
            &mut state,
            &mut backend,
            ClientId(1),
            SequenceNumber(1),
            RequestHeader {
                opcode: 147,
                data: 11, // FREE_SYNCOBJ
                length_units: 2,
            },
            &body,
            None,
        )
        .unwrap();
        peer.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 32];
        peer.read_exact(&mut buf).unwrap();
        assert_eq!(
            read_error(&buf),
            x11::error::BAD_VALUE,
            "FreeSyncobj of an unknown xid must be BadValue",
        );
    }

    #[test]
    fn free_syncobj_of_another_client_is_bad_value() {
        let mut state = ServerState::new();
        let _peer_a = install_client(&mut state, 1);
        let mut peer_b = install_client(&mut state, 2);
        let mut backend = syncobj_cap_backend();

        // Client A imports 0x0010_0001 (a legal xid for client 1 — the
        // handler now validates range). RecordingBackend ignores the fd's
        // payload, only ownership is recorded.
        let fd = std::fs::File::open("/dev/null")
            .expect("open /dev/null")
            .into();
        process_request(
            &mut state,
            &mut backend,
            ClientId(1),
            SequenceNumber(1),
            RequestHeader {
                opcode: 147,
                data: 10,
                length_units: 3,
            },
            &import_syncobj_body(0x0010_0001),
            Some(fd),
        )
        .unwrap();

        // Client B frees A's syncobj. FreeSyncobj has no range check (it is
        // not a new-resource request), so B can name A's xid — the ownership
        // check is what must reject it. One code, BadValue — deliberately NOT
        // Xorg's BadAccess (spec § "Divergence from Xorg").
        let mut body = vec![0u8; 4];
        body[0..4].copy_from_slice(&0x0010_0001u32.to_le_bytes());
        process_request(
            &mut state,
            &mut backend,
            ClientId(2),
            SequenceNumber(1),
            RequestHeader {
                opcode: 147,
                data: 11,
                length_units: 2,
            },
            &body,
            None,
        )
        .unwrap();
        peer_b.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 32];
        peer_b.read_exact(&mut buf).unwrap();
        assert_eq!(
            read_error(&buf),
            x11::error::BAD_VALUE,
            "FreeSyncobj of another client's syncobj must be BadValue",
        );

        // A still owns it (ImportSyncobj is a void request — nothing to read
        // on A's socket; the ownership assertion is the whole check).
        assert!(backend.dri3_syncobj_owners.contains_key(&0x0010_0001));
    }
```

`PresentPixmapSynced` tests need a window + pixmap, so they follow the shape
of `present_pixmap_synced_update_region_emits_damage_on_destination_window`
(`:40644`), which also documents the 84-byte body layout. Add a shared body
builder + setup helper, then three tests:

```rust
    /// PresentPixmapSynced 84-byte fixed prefix. Offsets per the :40644
    /// test's comment: window(0) pixmap(4) serial(8) valid(12) update(16)
    /// x_off(20) y_off(22) target_crtc(24) acquire_syncobj(28)
    /// release_syncobj(32) acquire_point(36) release_point(44) options(52)
    /// pad(56) target_msc(60) divisor(68) remainder(76).
    fn pixmap_synced_body(
        window: u32,
        pixmap: u32,
        acquire_syncobj: u32,
        release_syncobj: u32,
        acquire_point: u64,
        release_point: u64,
    ) -> Vec<u8> {
        let mut body = vec![0u8; 84];
        body[0..4].copy_from_slice(&window.to_le_bytes());
        body[4..8].copy_from_slice(&pixmap.to_le_bytes());
        body[28..32].copy_from_slice(&acquire_syncobj.to_le_bytes());
        body[32..36].copy_from_slice(&release_syncobj.to_le_bytes());
        body[36..44].copy_from_slice(&acquire_point.to_le_bytes());
        body[44..52].copy_from_slice(&release_point.to_le_bytes());
        body
    }

    fn dispatch_pixmap_synced(
        state: &mut ServerState,
        backend: &mut RecordingBackend,
        body: &[u8],
    ) {
        process_request(
            state,
            backend,
            ClientId(17),
            SequenceNumber(1),
            RequestHeader {
                opcode: 145,
                data: yserver_protocol::x11::present::PIXMAP_SYNCED,
                length_units: u32::try_from(1 + body.len() / 4).unwrap(),
            },
            body,
            None,
        )
        .unwrap();
    }

    #[test]
    fn present_pixmap_synced_unknown_acquire_syncobj_is_bad_value() {
        use yserver_protocol::x11::{ClientId, CreatePixmapRequest, CreateWindowRequest};
        const WINDOW_XID: u32 = 0x00e0_0403;
        const PIXMAP_XID: u32 = 0x00e0_0404;
        const ACQUIRE_SYNCOBJ: u32 = 0x00e0_0bad; // never imported
        const RELEASE_SYNCOBJ: u32 = 0x00e0_0408;

        let mut state = ServerState::new();
        let mut peer = install_client(&mut state, 17);
        let mut backend = RecordingBackend::new();
        // The release syncobj is a valid, imported resource; the acquire is
        // NOT. Row 4: an unknown acquire xid must produce a Value error, not
        // a silent no-reply hang.
        backend.dri3_syncobj_owners.insert(RELEASE_SYNCOBJ, ClientId(17));

        state.resources.create_window(
            ClientId(17),
            CreateWindowRequest {
                depth: 24,
                window: ResourceId(WINDOW_XID),
                parent: ROOT_WINDOW,
                x: 0,
                y: 0,
                width: 800,
                height: 600,
                border_width: 0,
                class: 1,
                visual: crate::resources::ROOT_VISUAL,
                ..Default::default()
            },
        );
        let _ = state.resources.map_window(ResourceId(WINDOW_XID));
        if let Some(w) = state.resources.window_mut(ResourceId(WINDOW_XID)) {
            w.host_xid = crate::backend::WindowHandle::from_raw(0x400403);
        }
        state.resources.create_pixmap(
            ClientId(17),
            CreatePixmapRequest {
                depth: 24,
                pixmap: ResourceId(PIXMAP_XID),
                drawable: ResourceId(WINDOW_XID),
                width: 800,
                height: 600,
            },
        );
        let _ = state.resources.set_pixmap_host_xid(
            ResourceId(PIXMAP_XID),
            crate::backend::PixmapHandle::from_raw(0x400404).expect("valid host pixmap"),
        );

        dispatch_pixmap_synced(
            &mut state,
            &mut backend,
            &pixmap_synced_body(WINDOW_XID, PIXMAP_XID, ACQUIRE_SYNCOBJ, RELEASE_SYNCOBJ, 1, 1),
        );

        peer.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 32];
        peer.read_exact(&mut buf).unwrap();
        assert_eq!(
            read_error(&buf),
            x11::error::BAD_VALUE,
            "unknown acquire syncobj must be BadValue, not a silent no-reply wait",
        );
        // X error: bytes 4-7 are the bad-value argument. It must carry the
        // offending xid, mirroring Xorg's VERIFY_DRI3_SYNCOBJ
        // (client->errorValue = id).
        assert_eq!(
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            ACQUIRE_SYNCOBJ,
            "BadValue must carry the offending syncobj xid",
        );
    }

    #[test]
    fn present_pixmap_synced_zero_point_is_bad_value() {
        use yserver_protocol::x11::ClientId;
        const WINDOW_XID: u32 = 0x00e0_0403;
        const PIXMAP_XID: u32 = 0x00e0_0404;
        const SYNCOBJ: u32 = 0x00e0_0407;

        let mut state = ServerState::new();
        let mut peer = install_client(&mut state, 17);
        let mut backend = RecordingBackend::new();
        backend.dri3_syncobj_owners.insert(SYNCOBJ, ClientId(17));

        // Both syncobjs imported; acquire_point == 0 is the violation.
        dispatch_pixmap_synced(
            &mut state,
            &mut backend,
            &pixmap_synced_body(WINDOW_XID, PIXMAP_XID, SYNCOBJ, SYNCOBJ, 0, 2),
        );

        peer.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 32];
        peer.read_exact(&mut buf).unwrap();
        assert_eq!(read_error(&buf), x11::error::BAD_VALUE, "acquire_point 0 must be BadValue");
    }

    #[test]
    fn present_pixmap_synced_acquire_gte_release_is_bad_value() {
        use yserver_protocol::x11::ClientId;
        const WINDOW_XID: u32 = 0x00e0_0403;
        const PIXMAP_XID: u32 = 0x00e0_0404;
        const SYNCOBJ: u32 = 0x00e0_0407;

        let mut state = ServerState::new();
        let mut peer = install_client(&mut state, 17);
        let mut backend = RecordingBackend::new();
        backend.dri3_syncobj_owners.insert(SYNCOBJ, ClientId(17));

        // Same syncobj for acquire and release: acquire_value >=
        // release_value is the violation.
        dispatch_pixmap_synced(
            &mut state,
            &mut backend,
            &pixmap_synced_body(WINDOW_XID, PIXMAP_XID, SYNCOBJ, SYNCOBJ, 5, 5),
        );

        peer.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 32];
        peer.read_exact(&mut buf).unwrap();
        assert_eq!(
            read_error(&buf),
            x11::error::BAD_VALUE,
            "acquire_value >= release_value on the same syncobj must be BadValue",
        );
    }
```

Note the `zero_point` and `acquire_gte_release` tests do not set up a window
or pixmap: the syncobj validation runs before the window-existence checks in
the handler, so the Value error fires first. The `unknown_acquire` test sets
up a fully valid window + pixmap to prove the error fires even for an
otherwise-valid request — that is the real-client shape of row 4.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p yserver-core --lib`
Expected: FAIL to compile. The new tests reference `dri3_syncobj_owners` and
`dri3_caps` (added in Step 4, via the `syncobj_cap_backend()` helper) and the
conformance error paths (Step 5-7), so the whole `#[cfg(test)]` target breaks.
A `cargo test <filter>` would not help — the compile error fires before any
test selection, so naming `import_syncobj_`, `free_syncobj_` or
`present_pixmap_synced_` changes nothing; run without a filter.

- [ ] **Step 3: Give the registry an owning client**

Change the field added in Task 2 Step 4 (`backend.rs`, after the
`dri3_sync_resources` field at `:805`):

```rust
    /// DRI3 1.4 syncobj resources keyed by the client's xid, imported by
    /// `ImportSyncobj`. The tuple's first element is the importing client —
    /// Xorg models a DRI3 syncobj as a first-class X resource owned by a
    /// client (`dri3_syncobj_type = CreateNewResourceType(...)`), and every
    /// conformance property in the spec's table falls out of that one
    /// decision: `FreeSyncobj` ownership, the disconnect purge, and the
    /// `PresentPixmapSynced` xid checks.
    pub(crate) dri3_syncobjs:
        HashMap<u32, (yserver_protocol::x11::ClientId, std::sync::Arc<crate::kms::render::imported_syncobj::ImportedSyncobj>)>,
```

This changes the shape every Task 2 method used. Update them in Step 5 and the
tests in Step 9 within this task (single commit — see the task intro).

- [ ] **Step 4: Add syncobj tracking to `RecordingBackend`**

`RecordingBackend` (`crates/yserver-core/src/backend/recording.rs`) drives the
wire-level tests. Add two fields, a private handle type, plus overrides for the
four syncobj methods:

```rust
    /// Minimal DRI3 syncobj registry for wire-level tests: xid -> owning
    /// client. Backed by a dummy `SyncobjHandle` so `dri3_syncobj_handle` has
    /// something to return.
    pub(crate) dri3_syncobj_owners: std::collections::HashMap<u32, yserver_protocol::x11::ClientId>,
    /// DRI3 capability surface. Defaults to `Dri3Caps::unsupported()` and is
    /// a FIELD, not a hardcoded override: the existing
    /// `dri3_hidden_when_caps_unsupported` test (process_request.rs:37076)
    /// requires the DEFAULT RecordingBackend to stay unsupported (DRI3 absent
    /// from QueryExtension/ListExtensions). The syncobj conformance tests
    /// flip this to a (1, 4)/`syncobj: true` surface so the `IMPORT_SYNCOBJ` /
    /// `FREE_SYNCOBJ` handlers pass their `caps.syncobj` gate.
    pub(crate) dri3_caps: crate::backend::Dri3Caps,
```

Initialize both in `RecordingBackend::new()` (recording.rs:369, alongside the
other field initialisers at `:396`): `dri3_syncobj_owners: HashMap::new(),`
and `dri3_caps: crate::backend::Dri3Caps::unsupported(),`.

```rust
#[derive(Debug)]
struct DummySyncobjHandle;

impl crate::backend::SyncobjHandle for DummySyncobjHandle {
    fn signal(&self, _value: u64) -> std::io::Result<()> {
        Ok(())
    }
}
```

And the overrides (next to the existing `dri3_signal_syncobj` override). The
`dri3_capabilities` override reads the field, so the default backend stays
`unsupported()` and only the conformance tests opt into a syncobj surface:

```rust
    fn dri3_capabilities(&self) -> crate::backend::Dri3Caps {
        self.dri3_caps
    }

    fn dri3_import_syncobj(
        &mut self,
        client_id: yserver_protocol::x11::ClientId,
        syncobj_xid: u32,
        _fd: std::os::fd::OwnedFd,
    ) -> std::io::Result<()> {
        self.dri3_syncobj_owners.insert(syncobj_xid, client_id);
        Ok(())
    }

    fn dri3_free_syncobj(
        &mut self,
        client_id: yserver_protocol::x11::ClientId,
        syncobj_xid: u32,
    ) -> std::io::Result<()> {
        // One code on the wire — BadValue — for both unknown and not-owner.
        // Deliberately not Xorg's BadValue/BadAccess split (spec § "Divergence
        // from Xorg"): the ownership ENFORCEMENT is what matters, the code is
        // cosmetic and no client branches on it.
        if self.dri3_syncobj_owners.get(&syncobj_xid) != Some(&client_id) {
            return Err(std::io::Error::other(format!(
                "DRI3 FreeSyncobj: unknown or not the owning client (0x{syncobj_xid:x})"
            )));
        }
        self.dri3_syncobj_owners.remove(&syncobj_xid);
        Ok(())
    }

    fn dri3_syncobj_handle(
        &self,
        syncobj_xid: u32,
    ) -> Option<std::sync::Arc<dyn crate::backend::SyncobjHandle>> {
        self.dri3_syncobj_owners
            .contains_key(&syncobj_xid)
            .then(|| std::sync::Arc::new(DummySyncobjHandle) as std::sync::Arc<dyn crate::backend::SyncobjHandle>)
    }
```

Note `dri3_syncobj_handle` returning a fresh `DummySyncobjHandle` per call means
the deferred-completion pinning test does not apply to `RecordingBackend`; the
ownership and error-semantics tests are what this backend is for.

- [ ] **Step 5: Thread `client_id` through the trait and the KmsBackend impl**

In `trait_def.rs`, change the signatures and docs of `dri3_import_syncobj`
(`:2045`) and `dri3_free_syncobj` (`:2054`) to take
`client_id: yserver_protocol::x11::ClientId` first. Update the default bodies
to keep returning `Err(...)` and keep the doc comment noting the resource is
owned by `client_id`.

In `backend.rs`:

- `dri3_import_syncobj`: store the owner with the resource, and `Err` on a
  duplicate xid still owned (the handler already rejected out-of-range /
  in-use xids; the backend re-checks for a racing duplicate):

```rust
    fn dri3_import_syncobj(
        &mut self,
        client_id: yserver_protocol::x11::ClientId,
        syncobj_xid: u32,
        fd: std::os::fd::OwnedFd,
    ) -> io::Result<()> {
        use std::os::fd::AsFd;

        let render_node = self.platform.render_node_device.as_ref().cloned().ok_or_else(
            || io::Error::other("DRI3 ImportSyncobj: render node not resolved at init"),
        )?;
        let imported = crate::kms::render::imported_syncobj::ImportedSyncobj::import(
            render_node,
            fd.as_fd(),
        )?;
        // Arc Drop on any replaced entry destroys the previous handle.
        let _ = self
            .dri3_syncobjs
            .insert(syncobj_xid, (client_id, std::sync::Arc::new(imported)));
        Ok(())
    }
```

- `dri3_free_syncobj`: enforce ownership, one `io::Error` for both failure
  cases. The handler maps it to yserver's single `BadValue` code (divergence
  from Xorg's BadValue/BadAccess split):

```rust
    fn dri3_free_syncobj(
        &mut self,
        client_id: yserver_protocol::x11::ClientId,
        syncobj_xid: u32,
    ) -> io::Result<()> {
        let Some((owner, _)) = self.dri3_syncobjs.get(&syncobj_xid) else {
            return Err(io::Error::other(format!(
                "DRI3 FreeSyncobj: unknown syncobj 0x{syncobj_xid:x}"
            )));
        };
        if *owner != client_id {
            return Err(io::Error::other(format!(
                "DRI3 FreeSyncobj: 0x{syncobj_xid:x} owned by another client"
            )));
        }
        // Arc Drop destroys the DRM handle when the last reference goes away,
        // which may be later than this call: the deferred completion path
        // pins clones past FreeSyncobj.
        let _ = self.dri3_syncobjs.remove(&syncobj_xid);
        Ok(())
    }
```

- `dri3_signal_syncobj` and `dri3_syncobj_handle` now index into the tuple:

```rust
    fn dri3_signal_syncobj(&mut self, syncobj_xid: u32, value: u64) -> io::Result<()> {
        use yserver_core::backend::SyncobjHandle as _;

        let arc = self.dri3_syncobjs.get(&syncobj_xid).map(|(_, arc)| arc).ok_or_else(|| {
            io::Error::other(format!(
                "DRI3 SignalSyncobj: unknown syncobj 0x{syncobj_xid:x}"
            ))
        })?;
        arc.signal(value)
    }

    fn dri3_syncobj_handle(
        &self,
        syncobj_xid: u32,
    ) -> Option<std::sync::Arc<dyn yserver_core::backend::SyncobjHandle>> {
        self.dri3_syncobjs
            .get(&syncobj_xid)
            .map(|(_, arc)| arc.clone())
            .map(|arc| arc as std::sync::Arc<dyn yserver_core::backend::SyncobjHandle>)
    }
```

- `arm_present_syncobj_wait` (the acquire path from Task 2 Step 7) is a FIFTH
  consumer of the registry and MUST be updated in this same task, or `cargo
  test -p yserver --lib` fails to compile: with the tuple type, Task 2 Step 7's
  `.get(&acquire_syncobj).cloned()` yields `(ClientId, Arc<ImportedSyncobj>)`,
  so `syncobj.signaled_eventfd(...)` and `syncobj_pin` both stop typing.
  Change the lookup to unwrap the tuple, leaving the rest of the block intact:

```rust
            let syncobj = self
                .dri3_syncobjs
                .get(&acquire_syncobj)
                .map(|(_, arc)| arc.clone())
                .ok_or_else(|| {
                    io::Error::other(format!(
                        "PresentPixmapSynced: unknown acquire syncobj 0x{acquire_syncobj:x}"
                    ))
                })?;
```

- [ ] **Step 6: Add the four error paths in the request handlers**

In `process_request.rs` `IMPORT_SYNCOBJ` (`:11457`), immediately AFTER the
parse guard (the check reads `req.syncobj`), add the XID validation (row 1)
using the repo's existing `LEGAL_NEW_RESOURCE` equivalents
(`xid_out_of_client_range` `:20797`, `resources.xid_in_use`):

```rust
            if xid_out_of_client_range(state, client_id, req.syncobj)
                || state.resources.xid_in_use(ResourceId(req.syncobj))
            {
                return emit_x11_error_with_minor(
                    state,
                    client_id,
                    sequence,
                    x11::error::BAD_ALLOC,
                    req.syncobj,
                    u16::from(header.data),
                    DRI3_MAJOR_OPCODE,
                );
            }
```

`xid_out_of_client_range` is a free fn in this file's non-test module — if it
is `#[cfg(test)]`-only today, promote it (it is used by the colormap handler at
`:20816`, so it is already live). Place the check between the parse guard and
the `attached_fd` check. Note the code is yserver's own `BadAlloc`, NOT Xorg's
`BadIDChoice` — spec § "Divergence from Xorg".

Row 2 — keep the missing-fd branch at `BadAlloc` (already what yserver emits at
`:11483-11491`). Xorg uses `BadValue` for `fd < 0`; yserver deliberately does
not (spec § "Divergence from Xorg"). The branch stays as-is:

```rust
            let Some(fd) = attached_fd else {
                return emit_x11_error_with_minor(
                    state,
                    client_id,
                    sequence,
                    x11::error::BAD_ALLOC,
                    req.syncobj,
                    u16::from(header.data),
                    DRI3_MAJOR_OPCODE,
                );
            };
```

Pass the client id into the import (Step 5 signature):

```rust
            if let Err(e) = backend.dri3_import_syncobj(client_id, req.syncobj, fd) {
                debug!(
                    "client {} #{} DRI3::ImportSyncobj 0x{:x} -> BadAlloc ({e})",
                    client_id.0, sequence.0, req.syncobj
                );
                return emit_x11_error_with_minor(
                    state,
                    client_id,
                    sequence,
                    x11::error::BAD_ALLOC,
                    req.syncobj,
                    u16::from(header.data),
                    DRI3_MAJOR_OPCODE,
                );
            }
```

In `FREE_SYNCOBJ` (`:11507`), replace the warn-only branch with an error
mapping (row 3). One code — `BadValue` — for both unknown and not-owner,
deliberately not Xorg's BadValue/BadAccess split (spec § "Divergence from
Xorg"; the ownership enforcement is what protects, the code is cosmetic):

```rust
            if let Err(e) = backend.dri3_free_syncobj(client_id, syncobj) {
                debug!(
                    "client {} #{} DRI3::FreeSyncobj 0x{:x} -> BadValue ({e})",
                    client_id.0, sequence.0, syncobj
                );
                return emit_x11_error_with_minor(
                    state,
                    client_id,
                    sequence,
                    x11::error::BAD_VALUE,
                    syncobj,
                    u16::from(header.data),
                    DRI3_MAJOR_OPCODE,
                );
            }
```

The `debug!` in the replacement is the only log line on this path — the
current `FREE_SYNCOBJ` handler has no success-path `debug!` (its only log was
the old `(warn: {e})` in the error branch, which the replacement absorbs). Do
not invent a success-path log here; the wire tests assert the error, and
`docs/status.md`/PR observability for `FreeSyncobj` is out of this task's
scope.

- [ ] **Step 7: Add `PresentPixmapSynced` validation (row 4)**

In the `PIXMAP_SYNCED` handler (`:10067`), immediately after the existing
divisor/remainder `BadValue` check, add the explicit-sync conformance. Per
presentproto 1.4 §7 (`present_request.c:296-302`): both syncobjs must be
non-None and previously imported, both points non-zero, and `acquire <
release` when they name the same syncobj — each violation is a `Value` error,
never a silent no-reply wait:

```rust
            let acquire_known = req.acquire_syncobj != 0
                && backend.dri3_syncobj_handle(req.acquire_syncobj).is_some();
            let release_known = req.release_syncobj != 0
                && backend.dri3_syncobj_handle(req.release_syncobj).is_some();
            if !acquire_known
                || !release_known
                || req.acquire_value == 0
                || req.release_value == 0
                || (req.acquire_syncobj == req.release_syncobj
                    && req.acquire_value >= req.release_value)
            {
                // Mirror Xorg's bad-value choice: VERIFY_DRI3_SYNCOBJ
                // (dri3/dri3.h:51-56) sets client->errorValue = <xid> when the
                // syncobj lookup fails, while the point/ordering failures
                // (present_request.c:299-301) leave errorValue 0. This is the
                // error ARGUMENT, not the error CODE — it is NOT part of the
                // declared divergence (which covers the code only).
                let bad_value = if !acquire_known {
                    req.acquire_syncobj
                } else if !release_known {
                    req.release_syncobj
                } else {
                    0
                };
                return emit_x11_error_with_minor(
                    state,
                    client_id,
                    sequence,
                    x11::error::BAD_VALUE,
                    bad_value,
                    u16::from(header.data),
                    PRESENT_MAJOR_OPCODE,
                );
            }
```

This runs before `arm_present_syncobj_wait` (`:10251`), so a bad xid can no
longer reach the arm path that previously errored out of the handler with no
reply. Confirm `dri3_syncobj_handle` is callable on `&mut dyn Backend` here
(it is a `&self` trait method).

- [ ] **Step 8: Purge a client's syncobjs on disconnect**

The registry now owns its entries, so the disconnect leak closes. Hook the
existing `client_disconnected` backend entry point (`backend.rs:14361`, called
from `process_disconnect.rs:474`):

```rust
    fn client_disconnected(&mut self, client_id: yserver_protocol::x11::ClientId) {
        self.dri3_syncobjs.retain(|_, (owner, _)| *owner != client_id);
        self.scene.root_overlay_on_disconnect(client_id);
    }
```

Add a unit test in `backend.rs` (needs a real DRM handle to build
`ImportedSyncobj`, so it is `#[ignore]` like Task 1's):

```rust
    #[test]
    #[ignore = "needs a DRM render node"]
    fn client_disconnected_purges_owned_syncobjs() {
        use std::os::fd::AsFd;
        use yserver_protocol::x11::ClientId;

        // Shared helper from Task 1 — never hardcode renderD128.
        let Some(drm) = crate::kms::render::imported_syncobj::tests::render_node() else {
            eprintln!("skipping: no render node");
            return;
        };
        let mk = |value: u64| {
            let handle = ::drm::control::Device::create_syncobj(drm.as_ref(), false)
                .expect("create syncobj");
            let fd = ::drm::control::Device::syncobj_to_fd(drm.as_ref(), handle, false)
                .expect("export fd");
            let imported = crate::kms::render::imported_syncobj::ImportedSyncobj::import(
                drm.clone(),
                fd.as_fd(),
            )
            .expect("import");
            ::drm::control::Device::destroy_syncobj(drm.as_ref(), handle).expect("destroy");
            std::sync::Arc::new(imported)
        };

        let mut b = KmsBackend::for_tests();
        b.dri3_syncobjs.insert(0x0040_0001, (ClientId(7), mk(1)));
        b.dri3_syncobjs.insert(0x0040_0002, (ClientId(8), mk(1)));

        yserver_core::backend::Backend::client_disconnected(&mut b, ClientId(7));
        assert!(
            b.dri3_syncobjs.contains_key(&0x0040_0002),
            "another client's syncobj must survive the disconnect",
        );
        assert!(
            !b.dri3_syncobjs.contains_key(&0x0040_0001),
            "the disconnecting client's syncobj must be purged",
        );
    }
```

**Retain-mode divergence, recorded:** `client_disconnected` is called
unconditionally by `process_disconnect`, which does not pass the close mode, so
a `RetainPermanent` client's syncobjs are purged too — Xorg would keep them.
This is a documented divergence, not a regression: a retained DRI3 1.4 client
(essentially nonexistent in practice) loses its syncobjs at disconnect, and
fixing it would require threading `close_mode` into `client_disconnected`, out
of scope here.

- [ ] **Step 9: Fix the tests from Tasks 2 that the type change breaks**

- `syncobj_handle_accessor_returns_arc_clone` (`backend.rs:30010`) — the map
  now stores tuples; update the two `.insert(...)` calls to
  `(yserver_protocol::x11::ClientId(1), std::sync::Arc::new(...))`.
- `each_resolver_sees_only_its_own_registry` (Task 2 Step 1) — same insert
  shape change.
- `dri3_import_syncobj_errs_without_a_usable_drm_handle` (`:28065`, rewritten
  in Task 2) — the signature now takes `client_id` first; add
  `yserver_protocol::x11::ClientId(1)`.
- The wire tests in Step 1 must now pass (they exercised the new paths).

- [ ] **Step 10: Run the tests**

```bash
cargo test -p yserver --lib
cargo test -p yserver-core --lib
cargo test -p yserver --lib -- --ignored
```
Expected: PASS on all three.

- [ ] **Step 11: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver-core/src/backend/trait_def.rs crates/yserver-core/src/backend/recording.rs crates/yserver-core/src/core_loop/process_request.rs crates/yserver/src/kms/render/backend.rs
git commit -m "feat(dri3): own syncobjs per client with error semantics

The spec scoped this in as the minimum that makes DRI3 1.4 safe to
advertise: a client can currently free another client's syncobj, and
PresentPixmapSynced with an unknown acquire syncobj gets no reply and no
error — a silent hang indistinguishable from a server crash.

The registry now carries the importing client. ImportSyncobj validates the
xid and the fd (BadAlloc); FreeSyncobj enforces ownership (BadValue); and
PresentPixmapSynced verifies both syncobjs and the point ordering per
presentproto 1.4 (Value errors). Error codes deliberately diverge from
Xorg's BadIDChoice/BadAccess: the protocol is silent there and no client
branches on the distinction (spec 'Divergence from Xorg'). client_disconnected
purges the owning client's syncobjs, closing the teardown leak."
```

---

### Task 4: Strip the syncobj half out of `OwnedSemaphore`

**Files:**
- Modify: `crates/yserver/src/kms/render/owned_semaphore.rs`
- Modify: `crates/yserver/src/kms/vk/sync.rs` (delete `import_drm_syncobj` and
  fix the intra-doc link at `:10`)
- Modify: `crates/yserver/src/kms/vk/device.rs:242-244` (stale comment)
- Modify: `crates/yserver-core/src/backend/trait_def.rs:345-353` (stale trait doc)

**Interfaces:**
- Produces: `OwnedSemaphore` reduced to `{ vk, semaphore }` with `new` and
  `semaphore`. No later task depends on the removed items.

- [ ] **Step 1: Reduce `OwnedSemaphore`**

Replace the whole file:

```rust
//! RAII wrapper for a `vk::Semaphore` so it can be `Arc`-shared for the
//! deferred PRESENT completion path. Destruction happens on the last Arc drop
//! (via `vkDestroySemaphore`), independent of the X11 resource id's lifetime.
//!
//! This backs XSync `Fence` resources only. DRI3 1.4 syncobjs are
//! `ImportedSyncobj` — they are DRM objects and never enter Vulkan.

use std::sync::Arc;

use ash::vk;

use crate::kms::vk::device::VkContext;

pub(crate) struct OwnedSemaphore {
    vk: Arc<VkContext>,
    semaphore: vk::Semaphore,
}

impl OwnedSemaphore {
    pub(crate) fn new(vk: Arc<VkContext>, semaphore: vk::Semaphore) -> Self {
        Self { vk, semaphore }
    }

    pub(crate) fn semaphore(&self) -> vk::Semaphore {
        self.semaphore
    }
}

impl std::fmt::Debug for OwnedSemaphore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedSemaphore")
            .field("semaphore", &self.semaphore)
            .finish_non_exhaustive()
    }
}

impl Drop for OwnedSemaphore {
    fn drop(&mut self) {
        if self.semaphore == vk::Semaphore::null() {
            return;
        }
        unsafe {
            self.vk.device.destroy_semaphore(self.semaphore, None);
        }
    }
}
```

`signal_vk` goes too. Its only callers were `dri3_signal_syncobj` (rewritten in
Task 2) and `impl SyncobjHandle for OwnedSemaphore` (deleted here), so keeping
it would be `dead_code` and fail `clippy --all-targets -- -D warnings`.
`for_tests_dummy` was already removed in Task 2 Step 10.

- [ ] **Step 2: Decide the fate of `signal_timeline`**

`crate::kms::vk::sync::signal_timeline` was `signal_vk`'s only callee. Run:

```bash
grep -rn "signal_timeline" crates/
```

It is `pub` inside `pub mod kms` / `pub mod vk` / `pub mod sync`, so it will not
trip `dead_code` — but if nothing calls it, it is now an unreachable public
helper. Delete it unless a caller remains.

- [ ] **Step 3: Delete the Vulkan syncobj import and its references**

- Delete `import_drm_syncobj` from `crates/yserver/src/kms/vk/sync.rs`
  (`:55-80`, doc comment included). Leave `import_sync_file`,
  `export_sync_file` alone — both still serve the fence path.
- `crates/yserver/src/kms/vk/sync.rs:10` is a rustdoc intra-doc link
  ``[`import_drm_syncobj`]`` in the module header. Deleting the function breaks
  it (a rustdoc warning clippy will not catch). Rewrite that bullet to describe
  `import_sync_file` instead.
- `crates/yserver/src/kms/vk/device.rs:242-244` justifies `timeline_semaphore(true)`
  by citing "Phase 4.2.2's `import_drm_syncobj`". Update the reason — the
  feature is still needed by other timeline users; say which.
- `crates/yserver-core/src/backend/trait_def.rs:345-353` documents
  `SyncobjHandle` as "opaque handle to a DRI3 syncobj's underlying VkSemaphore.
  Concrete impl in … `OwnedSemaphore`". Both halves are now false. Point it at
  `ImportedSyncobj` and drop the VkSemaphore wording.

- [ ] **Step 4: Build and let the compiler find stragglers**

Run: `cargo build -p yserver 2>&1 | head -40`
Expected: PASS. If something still references a deleted symbol, fix the call
site to use `ImportedSyncobj` rather than reinstating what you deleted. If a
call site cannot be converted, stop and report — a consumer was missed.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p yserver --lib
cargo clippy --all-targets -- -D warnings
```
Expected: PASS. Clippy matters as much as the tests here: this task's whole
risk is symbols going dead.

- [ ] **Step 6: Format and commit**

```bash
cargo +nightly fmt
git add crates/yserver/src/kms/render/owned_semaphore.rs crates/yserver/src/kms/vk/sync.rs crates/yserver/src/kms/vk/device.rs crates/yserver-core/src/backend/trait_def.rs
git commit -m "refactor(dri3): drop the Vulkan syncobj import

OwnedSemaphore carried a retained DRM handle, an eventfd registration, a
timeline query and a host-signal that existed only for the syncobj half,
now served by ImportedSyncobj. What remains is the XSync fence resource
it always was.

import_drm_syncobj goes with it: vkImportSemaphoreFdKHR on a DRM syncobj
fd is a Mesa-only accident, not a portable interop path."
```

---

### Task 5: Derive the capability from the kernel, not the driver

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:18967-18992`
  (`dri3_capabilities`) and the test at `:27892-27935`
- Modify: `crates/yserver/src/kms/render/platform.rs` (cache field)
- Modify: `crates/yserver/src/kms/vk/device.rs:78-89` (delete the blacklist and
  its doc comment)
- Modify: `crates/yserver/src/kms/render/backend.rs:1049` (doc comment citing
  the deleted function)

**This task turns the feature on. Do it last.**

- [ ] **Step 1: Cache the capability at init**

`dri3_capabilities()` is called once per DRI3 request
(`process_request.rs:10875`) and by `present_capabilities()`
(`backend.rs:19604`) on every Present `QueryCapabilities`. Today that is an
in-memory `driver_id` match; a raw ioctl on that path would be a regression.
Read it once when the platform is built.

In `PlatformBackend`, next to `render_node_device` (Task 2), add:

```rust
    /// `DRM_CAP_SYNCOBJ_TIMELINE` on the render node, read once at init.
    /// Whether DRI3 can offer syncobjs is a property of the kernel driver
    /// behind that fd, not of the Vulkan driver — the resource is a DRM
    /// syncobj and every operation on it is a DRM ioctl.
    pub(crate) syncobj_timeline: bool,
```

Populate it in `from_platform_init` right where `render_node_device` is built,
querying **the render node** (the same `Device` the syncobj ioctls run on — the
spec's one-device invariant), not the KMS node:

```rust
        let syncobj_timeline = render_node_device
            .as_ref()
            .and_then(|d| {
                use ::drm::Device as _;
                d.get_driver_capability(::drm::DriverCapability::TimelineSyncObj)
                    .ok()
            })
            .is_some_and(|v| v != 0);
```

Add `syncobj_timeline: false,` to `PlatformBackend::for_tests()` (`:1007`).
`get_driver_capability` lives on the `drm::Device` ROOT trait, not on
`drm::control::Device` — see `cursor_plane.rs:30-34`, which imports
`Device as DrmDevice` from the external `drm` crate. Note the path:
`crate::drm` is yserver's wrapper module, so the trait and
`DriverCapability::TimelineSyncObj` (at `drm` `lib.rs:309`) must be reached
via the extern-crate path `::drm::…`, exactly as `cursor_plane.rs` does.

- [ ] **Step 2: Write the failing test**

The old test (`:27892`) asserted `caps.syncobj == supports_dri3_syncobj()`, a
tautology against the blacklist. Replace it with one that pins the real
mapping. Add to the tests module in `backend.rs`:

```rust
/// DRI3 version follows syncobj support, and syncobj support follows the
/// kernel capability rather than the Vulkan driver. `for_tests()` has no
/// render node, so the capability is false and the version must be 1.3 —
/// which is also the check that would have caught a blacklist creeping back.
#[test]
fn dri3_syncobj_follows_the_kernel_capability() {
    assert_eq!(dri3_version_for(true), (1, 4));
    assert_eq!(dri3_version_for(false), (1, 3));

    let b = KmsBackend::for_tests();
    assert!(
        !b.platform.syncobj_timeline,
        "for_tests() has no render node, so the capability must be false",
    );
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p yserver --lib dri3_syncobj_follows -- --nocapture`
Expected: FAIL — `dri3_version_for` is not defined.

- [ ] **Step 4: Rewrite the capability derivation**

Add above the `impl Backend for KmsBackend` block containing
`dri3_capabilities`:

```rust
/// DRI3 version for a given syncobj capability. 1.4 is the version carrying
/// `ImportSyncobj` / `FreeSyncobj`; without them the server caps at 1.3 and
/// clients fall back to the fence path.
fn dri3_version_for(syncobj: bool) -> (u32, u32) {
    if syncobj { (1, 4) } else { (1, 3) }
}
```

Replace the body of `dri3_capabilities` (the availability guard now keys on
`render_node_device` — the same retained device the capability and every
syncobj ioctl use — not the bare `render_node_fd`; both are populated from the
same `render_node_path` in `from_platform_init`, but the device is the single
source of truth for this branch):

```rust
    fn dri3_capabilities(&self) -> Dri3Caps {
        // DRI3 entirely unavailable when the render-node device or Vulkan
        // weren't resolved at backend init: pixmap import/export still needs
        // both. `render_node_device` is the guard here (not the bare
        // `render_node_fd`) because it is what the syncobj ioctls and the
        // capability query run on.
        if self.platform.render_node_device.is_none() || self.platform.vk.is_none() {
            return Dri3Caps::unsupported();
        }
        let vk = self.platform.vk.as_ref().expect("vk Some by branch above");
        let modifiers = vk.image_drm_format_modifier;
        // VK_KHR_external_semaphore_fd is unconditionally enabled at device
        // init; fence_fd / SYNC_FD handle type rides along with it.
        let fence_fd = true;
        // Syncobj support is a property of the KERNEL, not of the Vulkan
        // driver. The previous NVIDIA blacklist here was a correct response
        // to vkImportSemaphoreFdKHR rejecting DRM syncobj fds, which no
        // longer matters because nothing imports them into Vulkan.
        let syncobj = self.platform.syncobj_timeline;
        Dri3Caps {
            version: dri3_version_for(syncobj),
            modifiers,
            fence_fd,
            syncobj,
        }
    }
```

The capability and the ioctls now both come from `render_node_device` — the
render node — so the spec's one-device invariant holds by construction (Task 2
Step 3 + this step).

- [ ] **Step 5: Delete the blacklist and its stale references**

- Delete `supports_dri3_syncobj` and its doc comment from
  `crates/yserver/src/kms/vk/device.rs:78-89`. **This is a compile error in the
  test target** until Step 2's replacement lands — `backend.rs:27928` called
  it. Removing it is not optional and `--all-targets` will catch it.
- `backend.rs:1049` carries a doc comment citing
  `VkContext::supports_dri3_syncobj`. Update it to name the kernel capability.

- [ ] **Step 6: Run everything**

```bash
cargo test -p yserver --lib
cargo test -p yserver --lib -- --ignored
cargo clippy --all-targets -- -D warnings
```
Expected: PASS.

- [ ] **Step 7: Format and commit**

```bash
cargo +nightly fmt
git add crates/yserver/src/kms/render/backend.rs crates/yserver/src/kms/render/platform.rs crates/yserver/src/kms/vk/device.rs
git commit -m "feat(dri3): advertise 1.4 whenever the kernel has timeline syncobj

supports_dri3_syncobj() blacklisted NVIDIA_PROPRIETARY, capping DRI3 at
1.3 and leaving PresentPixmapSynced untestable on the nvidia box. The
blacklist was a correct response to a real failure, recorded in its own
doc comment -- vkImportSemaphoreFdKHR rejects DRM syncobj fds on that
driver. It stops being the right question once nothing imports them.

The capability now comes from DRM_CAP_SYNCOBJ_TIMELINE on the render
node, cached at init because dri3_capabilities sits on the per-request
path. The capability and every syncobj ioctl run on the same retained
render-node Device, per the spec's one-device invariant."
```

---

### Task 6: Make the success path observable, then validate on hardware

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs:11457-11506`
- Modify: `docs/status.md`
- Modify: `docs/superpowers/specs/2026-08-08-dri3-syncobj-drm-signal-design.md`

- [ ] **Step 1: Add a success-path log line**

The only `DRI3::ImportSyncobj` line in the tree is the `BadAlloc` branch
(`process_request.rs:11493`). The success path logs nothing, so a healthy run
and a run where no client ever sent the request look identical — the validation
below would be meaningless without this. After the successful
`backend.dri3_import_syncobj(...)` call, add:

```rust
            debug!(
                "client {} #{} DRI3::ImportSyncobj 0x{:x} -> imported",
                client_id.0, sequence.0, req.syncobj
            );
```

Match the surrounding style: the neighbouring handlers all log
`client {} #{} DRI3::<Request> …` at `debug!`.

```bash
cargo +nightly fmt && cargo clippy --all-targets -- -D warnings
git add crates/yserver-core/src/core_loop/process_request.rs
git commit -m "feat(dri3): log successful ImportSyncobj

Only the BadAlloc branch logged, so a working explicit-sync client was
indistinguishable from no client at all."
```

- [ ] **Step 2: IGT GPU Tools — the DRM kernel path, per driver**

IGT GPU Tools is installed on the validation box. Its `syncobj_*` tests
exercise exactly the ioctl set this change relies on (create/destroy,
fd↔handle import/export, timeline signal/wait/query, eventfd, transfer)
straight against the kernel, so they validate the DRM layer independently of
yserver and of Mesa.

Run as root, with no compositor running (IGT requirement), on the box:

```bash
# Enumerate the syncobj tests and their subtests first. `igt_list` resolves
# the actual install path (Fedora: /usr/libexec; Debian: /usr/lib) once, so
# the run loop below does not need a `||` fallback that would also swallow a
# genuinely FAILING test as "wrong path" and retry it on the other prefix.
igt_list=$(ls /usr/libexec/igt-gpu-tools/syncobj_* 2>/dev/null || ls /usr/lib/igt-gpu-tools/syncobj_* 2>/dev/null)
for t in $igt_list; do echo "== $t =="; "$t" --list-subtests; done
```

Then run the syncobj suite (all subtests, or `--run-subtest <name>` for one).
The glob requires at least one match; if `igt_list` is empty, stop and find
where IGT installed its tests before running anything. Run the two blocks in
the SAME shell — the run loop reads the `igt_list` the enumeration set:

```bash
test -n "$igt_list" || { echo "no syncobj_* tests found — locate the IGT install"; exit 1; }
for t in $igt_list; do echo "== running $t =="; sudo "$t"; done
```

These tests open a render node themselves (`drm_open_driver_render(DRIVER_ANY)`
— first render node that matches), so on the two-GPU box they exercise
`renderD128` (nvidia-drm). Pin the other GPU the same way the spec's probes
did: if the installed IGT's test binaries accept a device option (check
`--help`), use it for `renderD129`; otherwise rely on `syncobjprobe.c`
`/dev/dri/renderD129` (spec Evidence, passes 12/12) for the amdgpu side. Record
which node each run opened — an IGT run on the wrong GPU is the same
not-evidence trap as a wrong ICD.

- [ ] **Step 3: Capture the full-Mesa session**

The spec's Risks make two things mandatory: the NVIDIA run is blocked (card0
has no connected connector), so this is the **full-Mesa** run (yserver and
client both on RADV/`card1`); and both env overrides are required, or the run
silently renders on the mismatched pair and proves nothing:

```bash
# The wire-level DRI3/PRESENT debug lines live on the default target, so
# present_pace alone is NOT enough -- process_request must be included.
# The two env overrides are the spec's mandatory pair (VK_ICD_FILENAMES is
# not optional) -- a run that omits them is not evidence about Mesa.
YSERVER_DRM_DEVICE=/dev/dri/card1 \
VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/radeon_icd.x86_64.json \
just yserver-mate-hw "info,present_pace=debug,yserver_core::core_loop::process_request=debug"
```

inside the session, forcing the Vulkan X11 WSI. It must be Vulkan, not GL:
`docs/status.md:4063` records NVIDIA's libGL failing to bind DRI3 against
yserver for unrelated reasons, so a GL client cannot answer either way:

```bash
mpv --gpu-api=vulkan --vo=gpu-next --gpu-context=x11vk \
    --length=15 av://lavfi:testsrc=size=1280x720:rate=60
```

Copy the log aside immediately — `just yserver-mate-hw` (`Justfile:293`)
truncates `yserver-hw-mate.log` with `>` on every start, including a normal
desktop session:

```bash
cp yserver-hw-mate.log dri3-syncobj-$(date +%Y%m%d).log
```

- [ ] **Step 4: Check what must appear**

```bash
LOG=dri3-syncobj-<date>.log
grep -c "DRI3::QueryVersion.*-> 1.4" "$LOG"         # server advertises 1.4
grep -c "DRI3::ImportSyncobj.*-> imported" "$LOG"    # client takes the path
grep -c "stage=acquire_deferred" "$LOG"              # deferred acquires (merge gate)
grep -c "stage=acquire_ready" "$LOG"                 # acquires that were already ready
grep -c "unknown syncobj" "$LOG"                     # freed-syncobj replay
grep -c "DRM eventfd unavailable" "$LOG"             # must be 0: no fallback
```

The **merge gate is non-zero deferrals, not just the imports.** Per the spec's
Testing §4, a run with zero deferrals proves nothing — the acquire points were
already met and the release path was never exercised under contention. The
`present_pace` instrument logs each deferred acquire as `PACE-INSTR ...
stage=acquire_deferred` (`process_request.rs:10265`) and each already-ready
one as `stage=acquire_ready` (`:9474`); require `stage=acquire_deferred` to be
`> 0`, and require `DRM eventfd unavailable` to be `0` (no fallback warning).
Compare against the recorded baseline (2,221 requests / 473 deferred / 0.87 ms
mean) as a shape, not a threshold — the iGPU is 2 CUs against a 6900HX, so
absolute timings differ; what must match is the invariant (every deferred
acquire eventually signals, no fallback warning).

`unknown syncobj` is the freed-syncobj bookkeeping bug
(`docs/status.md:548`) becoming reachable, not a regression from this change —
that path fails at the registry miss and never reaches an ioctl, with identical
text before and after. Record the count; do not fix it. Note there is
deliberately **no** check for a failed release signal: the kernel clamps stale
points and returns success, so out-of-order releases are silent by
construction.

- [ ] **Step 5: Update `docs/status.md`**

Record: the capability now deriving from `DRM_CAP_SYNCOBJ_TIMELINE` on the
render node; the counts from Step 4 (including the IGT results per node from
Step 2); and amend the claim at **`docs/status.md:297`** that
`PresentPixmapSynced` is "structurally untestable on this box", which named
this exact gate. Verify that line number before editing — this plan's first
draft had it as `:316`, read from a different branch.

- [ ] **Step 6: Flip the spec's status line**

Change `**Status:** DESIGN (2026-08-08)` to `IMPLEMENTED`, with the hardware
result and the box, following `2026-07-20-nvidia-gbm-scanout-allocation.md`.

- [ ] **Step 7: Commit**

```bash
git add docs/status.md docs/superpowers/specs/2026-08-08-dri3-syncobj-drm-signal-design.md
git commit -m "docs(dri3): record the DRM-signalled syncobj result on hardware"
```

---

## Deferred / out of scope

- **Cross-driver validation on a second box.** The Mesa path changes from a
  Vulkan host signal to a DRM host signal. The nvidia box now covers both
  nvidia-drm (IGT `syncobj_*` on `renderD128`) and amdgpu (full-Mesa run +
  IGT/`syncobjprobe` on `renderD129`), which is the cross-driver evidence the
  spec's merge gate needs. A second box (the bee, 6900HX / RADV) remains a
  nice-to-have confirmation, not a blocker.
- **The freed-syncobj bookkeeping bug** (`docs/status.md:548`). This change
  makes it reachable here; it does not cause it, and it does not make it more
  observable.
- **The retain-mode divergence in Task 3's disconnect purge.** A
  `RetainPermanent` client's syncobjs are purged at disconnect; Xorg would
  keep them. `client_disconnected` does not receive the close mode; fixing it
  would thread `close_mode` through the backend entry point. No practical DRI3
  1.4 client uses retain mode.
- **The fail-open arm in `PendingPresentSourceWait::is_ready`.** A failed
  timeline query is treated as ready and the copy proceeds. Unreachable on
  NVIDIA before this change; live after it. Task 6 Step 4's
  `DRM eventfd unavailable == 0` gate is the tripwire, not a fix.
- **GPU-side release signalling.** Signalling the client's release point from
  the queue rather than the host would let clients unblock earlier, but it is a
  separate design with its own measurement, and impossible on NVIDIA. The
  queue-wait half of that idea is foreclosed by the single shared queue — see
  the spec's root-cause section.
