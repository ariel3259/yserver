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

**Tech Stack:** Rust, `drm` 0.15 (`syncobj_timeline_signal`,
`syncobj_timeline_query`, `syncobj_eventfd`, `fd_to_syncobj`,
`DriverCapability::TimelineSyncObj`), `ash` (only where it already was).

**Spec:** `docs/superpowers/specs/2026-08-08-dri3-syncobj-drm-signal-design.md`

**Revision:** rewritten 2026-08-08 after two adversarial review rounds returned
9 blocking findings against the first draft. The task boundaries changed — what
were Tasks 2 and 3 are now one task, because splitting them left the tree
functionally broken on Mesa at a commit boundary.

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
- `docs/status.md` must be updated (Task 5). AGENTS.md requires it current.
- Do not fix the freed-syncobj bookkeeping bug this change makes reachable
  (`docs/status.md:548`), and do not fix the fail-open arm in
  `PendingPresentSourceWait::is_ready`. Both are recorded in the spec's Risks
  section as out of scope.

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
  whose `signal(u64)` uses `syncobj_timeline_signal`. Tasks 2 and 4 depend on
  these exact names.

- [ ] **Step 1: Add a master-free constructor**

`Device::open` (`crates/yserver/src/drm/device.rs:41`) calls
`acquire_master_lock()` and propagates with `?`. `DRM_IOCTL_SET_MASTER` returns
`EACCES` on a render node — `drm_ioctl_permit` rejects any non-`DRM_RENDER_ALLOW`
ioctl from a render client — and also on `card0` under a live session. There is
no way to build a `crate::drm::Device` over a render node today, so the tests
below cannot exist without this.

Add to `impl Device`, after `for_tests()` (`:39`):

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

Note `pub(crate)` on both the module and `render_node()`: Tasks 3 and 5 reuse
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
    /// so the caller keeps ownership and drops it normally.
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
- Modify: `crates/yserver/src/kms/render/present_source_wait.rs:20-25,40-52`

**Interfaces:**
- Consumes: everything Task 1 produced.
- Produces: field `dri3_syncobjs: HashMap<u32, Arc<ImportedSyncobj>>`;
  `PendingPresentSourceWait::syncobj_pin` retyped to
  `Option<Arc<ImportedSyncobj>>`. `dri3_sync_resources` survives, fences only.

**Why this is one task and not two.** The registry split and the acquire-path
rewrite cannot be separate commits. `arm_present_syncobj_wait`
(`backend.rs:13166`) looks up `dri3_sync_resources` for the acquire syncobj; the
moment `ImportSyncobj` writes elsewhere, that lookup's `ok_or_else` fires and
the error propagates out of the request handler via `?`
(`process_request.rs:10251`). It still compiles and `cargo test --lib` still
passes, so nothing catches it — but on Mesa, where the capability is still
advertised until Task 4, every synced present with an acquire syncobj breaks at
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

- [ ] **Step 3: Add the new field**

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
    /// syncobj handle, not a `VkSemaphore`.
    pub(crate) dri3_syncobjs:
        HashMap<u32, std::sync::Arc<crate::kms::render::imported_syncobj::ImportedSyncobj>>,
```

Add `dri3_syncobjs: HashMap::new(),` next to `dri3_sync_resources:
HashMap::new(),` in **both** initialisers (`:2334` and `:3222`). Missing the
second is a compile error, not a silent bug.

- [ ] **Step 4: Rewire the four syncobj methods**

Replace `dri3_import_syncobj` (`:19313-19375`). The `dup`, the Vulkan gate, the
import and the rollback all go away — `fd_to_syncobj` borrows the fd and there
is nothing left to roll back:

```rust
    fn dri3_import_syncobj(
        &mut self,
        syncobj_xid: u32,
        fd: std::os::fd::OwnedFd,
    ) -> io::Result<()> {
        use std::os::fd::AsFd;

        let imported = crate::kms::render::imported_syncobj::ImportedSyncobj::import(
            self.platform.device.clone(),
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

- [ ] **Step 5: Move the registry lookup ahead of the Vulkan gate**

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

- [ ] **Step 6: Repoint the acquire path (this is what keeps the tree working)**

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

- [ ] **Step 7: Retype the pin and its readiness check**

In `present_source_wait.rs`, replace lines 20-25:

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

- [ ] **Step 8: Fix the two existing tests this task breaks**

`syncobj_handle_accessor_returns_arc_clone` (`backend.rs:30010-30029`) inserts
an `OwnedSemaphore` into `dri3_sync_resources` and asserts
`dri3_syncobj_handle` finds it. After Step 4 it returns `None` and the test
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
Vulkan, which is no longer why it errs. Replace it:

```rust
    /// ImportSyncobj no longer needs Vulkan. Without a real DRM device in
    /// for_tests() it still Errs, but on the ioctl rather than a Vk gate.
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
        assert!(
            b.dri3_import_syncobj(0x4040_3333, fd).is_err(),
            "importing a non-syncobj fd must Err",
        );
    }
```

- [ ] **Step 9: Check whether `for_tests_dummy` still has callers**

`OwnedSemaphore::for_tests_dummy` (`owned_semaphore.rs:80`) existed for the
test just rewritten. Run:

```bash
grep -rn "for_tests_dummy" crates/
```

If nothing remains, delete it — `#[cfg(test)]` dead code still fails
`clippy --all-targets -- -D warnings`.

- [ ] **Step 10: Run the tests**

```bash
cargo test -p yserver --lib                                    # no regressions
cargo test -p yserver --lib -- --ignored                       # the DRM ones
```
Expected: PASS on both.

- [ ] **Step 11: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/render/backend.rs crates/yserver/src/kms/render/present_source_wait.rs crates/yserver/src/kms/render/owned_semaphore.rs
git commit -m "refactor(dri3): hold syncobjs in their own DRM-backed registry

FenceFromFD and ImportSyncobj registered two different X resource types
into one HashMap of OwnedSemaphore. Only the fence half needs Vulkan
(FDFromFence exports a sync_file from the VkSemaphore); the syncobj half
needs a DRM handle. Split them so the type mismatch is unrepresentable.

The acquire path moves in the same commit rather than a later one: it
looks up the same map, so splitting the two would leave every synced
present failing on Mesa at the intermediate commit.

FDFromFence now looks up the registry before the Vulkan gate, so an
unknown xid reports as unknown rather than as a Vulkan failure."
```

---

### Task 3: Strip the syncobj half out of `OwnedSemaphore`

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

### Task 4: Derive the capability from the kernel, not the driver

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:18967-18992`
  (`dri3_capabilities`) and the test at `:27891-27935`
- Modify: `crates/yserver/src/kms/render/platform.rs` (cache field)
- Modify: `crates/yserver/src/kms/vk/device.rs:78-89` (delete the blacklist and
  its doc comment)
- Modify: `crates/yserver/src/kms/render/backend.rs:1049` (doc comment citing
  the deleted function)

**This task turns the feature on. Do it last.**

- [ ] **Step 1: Cache the capability at init**

`dri3_capabilities()` is called once per DRI3 request
(`process_request.rs:10873`) and by `present_capabilities()`
(`backend.rs:19611`) on every Present `QueryCapabilities`. Today that is an
in-memory `driver_id` match; a raw ioctl on that path would be a regression.
Read it once when the platform is built.

In `PlatformBackend`, next to `render_node_fd`, add:

```rust
    /// `DRM_CAP_SYNCOBJ_TIMELINE` on the render node, read once at init.
    /// Whether DRI3 can offer syncobjs is a property of the kernel driver
    /// behind that fd, not of the Vulkan driver — the resource is a DRM
    /// syncobj and every operation on it is a DRM ioctl.
    pub(crate) syncobj_timeline: bool,
```

Populate it where `render_node_fd` is resolved, querying **the render node**,
not the KMS node: DRI3 hands clients the render node, and on split-device
boxes (Pi 4 vc4/v3d, Asahi apple-drm/AGX) the display device's answer says
nothing about the render device's. Set `false` when there is no render node.

- [ ] **Step 2: Write the failing test**

The old test asserted `caps.syncobj == supports_dri3_syncobj()`, a tautology
against the blacklist. Replace it with one that pins the real mapping. Add to
the tests module in `backend.rs`:

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

Replace the body of `dri3_capabilities`:

```rust
    fn dri3_capabilities(&self) -> Dri3Caps {
        // DRI3 entirely unavailable when render-node fd or Vulkan weren't
        // resolved at backend init: pixmap import/export still needs both.
        if self.platform.render_node_fd.is_none() || self.platform.vk.is_none() {
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

**Trait path warning for Step 1's query:** `get_driver_capability` lives on the
`drm::Device` ROOT trait, not on `drm::control::Device`, which this file already
imports for the syncobj calls. `crates/yserver/src/kms/cursor_plane.rs:30-34`
shows the distinction — it imports `Device as DrmDevice` alongside
`control::{Device as ControlDevice, …}`. `DriverCapability::TimelineSyncObj`
is at `drm` `lib.rs:309`; `get_driver_capability` returns `io::Result<u64>`, so
`.is_ok_and(|v| v != 0)` is the test.

- [ ] **Step 5: Delete the blacklist and its stale references**

- Delete `supports_dri3_syncobj` and its doc comment from
  `crates/yserver/src/kms/vk/device.rs:78-89`. **This is a compile error in the
  test target** until Step 2's replacement lands — `backend.rs:27926-27929`
  called it. Removing it is not optional and `--all-targets` will catch it.
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
path."
```

---

### Task 5: Make the success path observable, then validate on hardware

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

- [ ] **Step 2: Capture**

```bash
# The wire-level DRI3/PRESENT debug lines live on the default target, so
# present_pace alone is NOT enough -- process_request must be included.
just yserver-mate-hw "info,present_pace=debug,yserver_core::core_loop::process_request=debug"

# inside the session, forcing the Vulkan X11 WSI. It must be Vulkan, not GL:
# docs/status.md:4063 records NVIDIA's libGL failing to bind DRI3 against
# yserver for unrelated reasons, so a GL client cannot answer either way.
mpv --gpu-api=vulkan --vo=gpu-next --gpu-context=x11vk \
    --length=15 av://lavfi:testsrc=size=1280x720:rate=60
```

Copy the log aside immediately — `just yserver-mate-hw` (`Justfile:293`)
truncates `yserver-hw-mate.log` with `>` on every start, including a normal
desktop session:

```bash
cp yserver-hw-mate.log dri3-syncobj-$(date +%Y%m%d).log
```

- [ ] **Step 3: Check what must appear**

```bash
LOG=dri3-syncobj-<date>.log
grep -c "DRI3::QueryVersion.*-> 1.4" "$LOG"      # server advertises 1.4
grep -c "DRI3::ImportSyncobj.*-> imported" "$LOG" # client takes the path
grep -c "present acquire" "$LOG"                  # acquires resolve
grep -c "unknown syncobj" "$LOG"                  # freed-syncobj replay
```

The last one is the freed-syncobj bookkeeping bug (`docs/status.md:548`)
becoming reachable, not a regression from this change — that path fails at the
registry miss and never reaches an ioctl, with identical text before and after.
Record the count; do not fix it. Note there is deliberately **no** check for a
failed release signal: the kernel clamps stale points and returns success, so
out-of-order releases are silent by construction.

- [ ] **Step 4: Update `docs/status.md`**

Record: the capability now deriving from `DRM_CAP_SYNCOBJ_TIMELINE` on the
render node; the counts from Step 3; and amend the claim at **`docs/status.md:297`**
that `PresentPixmapSynced` is "structurally untestable on this box", which named
this exact gate. Verify that line number before editing — this plan's first
draft had it as `:316`, read from a different branch.

- [ ] **Step 5: Flip the spec's status line**

Change `**Status:** DESIGN (2026-08-08)` to `IMPLEMENTED`, with the hardware
result and the box, following `2026-07-20-nvidia-gbm-scanout-allocation.md`.

- [ ] **Step 6: Commit**

```bash
git add docs/status.md docs/superpowers/specs/2026-08-08-dri3-syncobj-drm-signal-design.md
git commit -m "docs(dri3): record the DRM-signalled syncobj result on hardware"
```

---

## Deferred / out of scope

- **Cross-driver validation.** The Mesa path changes from a Vulkan host signal
  to a DRM host signal and there is no AMD or Intel GPU on this box. The bee
  (6900HX / RADV) is where that gets confirmed.
- **The freed-syncobj bookkeeping bug** (`docs/status.md:548`). This change
  makes it reachable here; it does not cause it, and it does not make it more
  observable.
- **No client-teardown purge for `dri3_syncobjs`.** Nothing removes entries
  except `FreeSyncobj`, so a client that dies with syncobjs imported leaks a
  DRM handle and the fence chain it pins. Pre-existing in shape, newly exposed
  in practice.
- **The fail-open arm in `PendingPresentSourceWait::is_ready`.** A failed
  timeline query is treated as ready and the copy proceeds. Unreachable on
  NVIDIA before this change; live after it.
- **GPU-side release signalling.** Signalling the client's release point from
  the queue rather than the host would let clients unblock earlier, but it is a
  separate design with its own measurement, and impossible on NVIDIA. The
  queue-wait half of that idea is foreclosed by the single shared queue — see
  the spec's root-cause section.
