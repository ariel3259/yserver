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

## Global Constraints

- Branch: `dri3-syncobj-drm-signal` (already created off `master`; the spec is
  committed there as `a7d4ce8f`).
- Format with `cargo +nightly fmt` before every commit.
- Lint exactly as CI does: `cargo clippy --all-targets -- -D warnings`. CI
  fails on any warning and `--all-targets` lints test code too.
- No new ioctl plumbing is introduced — every ioctl used here already exists in
  the `drm` crate, so the AGENTS.md `libc::Ioctl` portability rule
  (glibc/musl/FreeBSD) is not triggered. If you find yourself writing a raw
  `ioctl` call, stop: you have left the plan.
- `docs/status.md` must be updated (Task 6). AGENTS.md requires it current, and
  it covers more than render work.
- Do not fix the freed-syncobj bookkeeping bug this change makes reachable
  (`docs/status.md:567`). It is out of scope and tracked separately.

---

### Task 1: `ImportedSyncobj` — the DRM-backed syncobj resource

**Files:**
- Create: `crates/yserver/src/kms/render/imported_syncobj.rs`
- Modify: `crates/yserver/src/kms/render/mod.rs:18` (add the module)

**Interfaces:**
- Consumes: `crate::drm::Device` (already `Arc`-held as
  `platform.device: Arc<drm::Device>`).
- Produces: `ImportedSyncobj::import(Arc<crate::drm::Device>, BorrowedFd) ->
  io::Result<Self>`, `.timeline_value() -> io::Result<u64>`,
  `.signaled_eventfd(u64) -> io::Result<OwnedFd>`, and an
  impl of `yserver_core::backend::SyncobjHandle` whose `signal(u64)` uses
  `syncobj_timeline_signal`. Tasks 2 and 3 depend on these exact names.

- [ ] **Step 1: Write the failing test**

Create `crates/yserver/src/kms/render/imported_syncobj.rs` containing only the
test module for now:

```rust
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ::drm::control::Device as DrmControlDevice;

    use super::*;

    /// Full round trip against a real DRM node, mirroring the server's
    /// sequence: the client exports a syncobj fd, the server imports it,
    /// signals a release point, and the client's own handle observes it.
    /// Ignored because it needs a DRM device — run with
    /// `cargo test -p yserver --lib imported_syncobj -- --ignored`.
    #[test]
    #[ignore = "needs a DRM render node"]
    fn signal_reaches_the_clients_handle() {
        let drm = Arc::new(
            crate::drm::Device::open("/dev/dri/renderD128").expect("open render node"),
        );

        // The client's half.
        let client_handle = drm.create_syncobj(false).expect("create syncobj");
        let fd = drm.syncobj_to_fd(client_handle, false).expect("export fd");

        // The server's half, as dri3_import_syncobj will do it.
        let imported = ImportedSyncobj::import(drm.clone(), fd.as_fd()).expect("import");

        assert_eq!(imported.timeline_value().expect("query"), 0);

        yserver_core::backend::SyncobjHandle::signal(&imported, 7).expect("signal");

        // The client must observe the release through its own handle, or it
        // would wait forever.
        let mut points = [0u64; 1];
        drm.syncobj_timeline_query(&[client_handle], &mut points, false)
            .expect("client query");
        assert_eq!(points[0], 7, "server signal did not reach the client handle");
        assert_eq!(imported.timeline_value().expect("query"), 7);

        drm.destroy_syncobj(client_handle).expect("destroy");
    }

    #[test]
    #[ignore = "needs a DRM render node"]
    fn eventfd_fires_on_the_registered_point() {
        use std::os::fd::AsFd;

        let drm = Arc::new(
            crate::drm::Device::open("/dev/dri/renderD128").expect("open render node"),
        );
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
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p yserver --lib imported_syncobj -- --ignored`
Expected: compile error — `ImportedSyncobj` is not defined, and the module is
not registered in `mod.rs`.

- [ ] **Step 3: Register the module**

In `crates/yserver/src/kms/render/mod.rs`, add in alphabetical position (after
line 16, `glyph_pixels`):

```rust
pub(crate) mod imported_syncobj;
```

- [ ] **Step 4: Write the implementation**

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
//! (`vkImportSemaphoreFdKHR` → `VK_ERROR_INITIALIZATION_FAILED`, measured
//! 2026-08-08). See
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
    /// Import a client's `DRM_SYNCOBJ` fd as a process-local handle. The fd
    /// is only borrowed — `DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE` does not consume
    /// it — so the caller keeps ownership and drops it normally.
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
    /// also a host operation — the ordering guarantee is unchanged.
    fn signal(&self, value: u64) -> std::io::Result<()> {
        self.drm.syncobj_timeline_signal(&[self.handle], &[value])
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p yserver --lib imported_syncobj -- --ignored`
Expected: 2 passed. If `crate::drm::Device::open` has a different signature,
check `crates/yserver/src/drm/device.rs` and adapt the test's construction
only — not the production code.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/render/imported_syncobj.rs crates/yserver/src/kms/render/mod.rs
git commit -m "feat(dri3): add ImportedSyncobj, a DRM-backed syncobj resource

A DRI3 syncobj is a kernel object and every operation the server needs
on one -- signal, query, eventfd -- has a DRM ioctl. Importing it into a
VkSemaphore only works where the driver's OPAQUE_FD payload happens to
be a DRM syncobj, which is false on NVIDIA proprietary.

Not wired up yet; the registries still hold OwnedSemaphore."
```

---

### Task 2: Split the syncobj registry from the fence registry

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:805` (field),
  `:2334` and `:3222` (both initialisers), `:19291-19299`
  (`dri3_syncobj_handle`), `:19313-19375` (`dri3_import_syncobj`),
  `:19376-19382` (`dri3_free_syncobj`), `:19384-19392`
  (`dri3_signal_syncobj`)
- Test: inline `#[cfg(test)] mod tests` in `backend.rs`

**Interfaces:**
- Consumes: `ImportedSyncobj::import`, `SyncobjHandle` impl from Task 1.
- Produces: field `dri3_syncobjs: HashMap<u32, Arc<ImportedSyncobj>>`.
  `dri3_sync_resources` survives, now holding fences only. Task 3 reads
  `dri3_syncobjs`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `backend.rs`, next to the existing
`dri3_import_syncobj_no_vk_errs` test (around `:28065`):

```rust
/// Fences and syncobjs are different X resource types and must not share
/// one registry: before the split, FDFromFence on a syncobj xid resolved
/// into the same map and half-worked.
#[test]
fn fd_from_fence_does_not_resolve_a_syncobj_xid() {
    let mut b = KmsBackend::for_tests();
    // No Vulkan in for_tests(), so a resolved entry and an unresolved one
    // both Err. Distinguish them by message: an unknown xid must be
    // reported as unknown, not as a Vulkan failure.
    let err = b
        .dri3_fd_from_fence(0x4040_5555)
        .expect_err("unknown fence xid must Err");
    assert!(
        err.to_string().contains("unknown fence"),
        "expected an unknown-fence error, got: {err}",
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p yserver --lib fd_from_fence_does_not_resolve -- --nocapture`
Expected: FAIL — `dri3_fd_from_fence` checks Vulkan availability before the
registry lookup, so the message is "DRI3 FDFromFence: Vulkan unavailable".

- [ ] **Step 3: Add the new field**

In `backend.rs`, immediately after the `dri3_sync_resources` field (`:805`),
add:

```rust
    /// DRI3 1.4 syncobj resources keyed by the client's xid, imported by
    /// `ImportSyncobj`. Separate from `dri3_sync_resources` because these
    /// are a different X resource type with a different backing primitive:
    /// a DRM syncobj handle, not a `VkSemaphore`.
    pub(crate) dri3_syncobjs:
        HashMap<u32, std::sync::Arc<crate::kms::render::imported_syncobj::ImportedSyncobj>>,
```

Narrow the doc comment on `dri3_sync_resources` (`:800-804`) to fences only:

```rust
    /// DRI3 sync-fence resources keyed by the client's xid, from
    /// `FenceFromFD` falling through the xshmfence path (sync_file fd →
    /// `VkSemaphore`). Syncobjs live in `dri3_syncobjs`.
```

Add `dri3_syncobjs: HashMap::new(),` next to `dri3_sync_resources:
HashMap::new(),` in **both** initialisers (`:2334` and `:3222`). Missing the
second one is a compile error, not a silent bug.

- [ ] **Step 4: Rewire the four syncobj methods**

Replace `dri3_import_syncobj` (`:19313-19375`) entirely — the `dup`, the
Vulkan gate, the import, and the rollback all go away, because
`fd_to_syncobj` borrows the fd and there is nothing to roll back:

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

Replace `dri3_free_syncobj` (`:19376-19382`):

```rust
    fn dri3_free_syncobj(&mut self, syncobj_xid: u32) -> io::Result<()> {
        // Arc Drop destroys the DRM handle when the last reference goes
        // away, which may be later than this call: the deferred completion
        // path pins clones past FreeSyncobj.
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

In `dri3_fd_from_fence` (`:19301-19311`), the lookup must come first so an
unknown xid reports as unknown. Replace its body:

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

- [ ] **Step 6: Fix the pre-existing test that asserted the old shape**

`dri3_import_syncobj_no_vk_errs` (`:28065`) asserts the import Errs without
Vulkan. That is no longer true — the import no longer touches Vulkan. Replace
the test with one that asserts what now holds:

```rust
/// ImportSyncobj no longer needs Vulkan. Without a DRM device in
/// for_tests() it still Errs, but on the DRM ioctl rather than a Vk gate.
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

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p yserver --lib dri3_ -- --nocapture`
Expected: PASS, including `fd_from_fence_does_not_resolve_a_syncobj_xid` and
`dri3_import_syncobj_errs_without_a_usable_drm_handle`.

- [ ] **Step 8: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/render/backend.rs
git commit -m "refactor(dri3): hold syncobjs in their own registry, backed by DRM

FenceFromFD and ImportSyncobj registered two different X resource types
into one HashMap of OwnedSemaphore. Only the fence half needs Vulkan
(FDFromFence exports a sync_file from the VkSemaphore); the syncobj half
needs a DRM handle. Split them so the type mismatch is unrepresentable,
and move ImportSyncobj/FreeSyncobj/SignalSyncobj onto ImportedSyncobj.

FDFromFence now looks up the registry before the Vulkan gate, so an
unknown xid reports as unknown rather than as a Vulkan failure."
```

---

### Task 3: Move the deferred acquire path off Vulkan

**Files:**
- Modify: `crates/yserver/src/kms/render/present_source_wait.rs:22-25,41-49`
- Modify: `crates/yserver/src/kms/render/backend.rs:13161-13185` (the
  `dri3_sync_resources` lookup in the acquire path)

**Interfaces:**
- Consumes: `ImportedSyncobj::timeline_value`, `.signaled_eventfd` (Task 1);
  `dri3_syncobjs` (Task 2).
- Produces: `PendingPresentSourceWait::syncobj_pin` retyped to
  `Option<Arc<ImportedSyncobj>>`. No later task depends on this.

- [ ] **Step 1: Retype the pin and its readiness check**

In `present_source_wait.rs`, replace lines 20-25:

```rust
    /// Keeps an explicitly imported acquire syncobj alive until its timeline
    /// point signals. Implicit dma-buf waits leave this empty.
    pub(crate) syncobj_pin: Option<Arc<super::imported_syncobj::ImportedSyncobj>>,
    /// Used only when DRM_SYNCOBJ_EVENTFD is unavailable and readiness must
    /// be checked by polling the syncobj's timeline value.
    pub(crate) timeline_value: Option<u64>,
```

Replace the `is_ready` timeline arm (lines 40-50):

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

- [ ] **Step 2: Point the acquire path at the new registry**

In `backend.rs`, in the block starting at `:13161`, change the lookup and the
fallback log line:

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

- [ ] **Step 3: Run the existing tests to verify nothing broke**

Run: `cargo test -p yserver --lib present_source_wait`
Expected: PASS. The two existing tests construct `syncobj_pin: None`, so they
compile unchanged — that is the signal that the retype is contained.

- [ ] **Step 4: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/render/present_source_wait.rs crates/yserver/src/kms/render/backend.rs
git commit -m "refactor(present): poll deferred acquires through the DRM timeline

The acquire eventfd was already DRM; only its polling fallback still
went through vkGetSemaphoreCounterValue. Move that to
DRM_IOCTL_SYNCOBJ_QUERY so the whole acquire path is driver-neutral."
```

---

### Task 4: Strip the syncobj half out of `OwnedSemaphore`

**Files:**
- Modify: `crates/yserver/src/kms/render/owned_semaphore.rs` (remove
  `drm_syncobj`, `new_drm_syncobj`, `signaled_eventfd`, `timeline_value`, the
  `SyncobjHandle` impl, and the DRM branch of `Drop`)
- Modify: `crates/yserver/src/kms/vk/sync.rs:55-80` (delete
  `import_drm_syncobj`)

**Interfaces:**
- Consumes: nothing new.
- Produces: `OwnedSemaphore` reduced to `{ vk, semaphore }` with `new`,
  `semaphore`, `signal_vk`. No later task depends on the removed items.

- [ ] **Step 1: Reduce `OwnedSemaphore`**

Replace the whole of `owned_semaphore.rs` below the module doc comment. Update
the doc comment first to say what it is now:

```rust
//! RAII wrapper for a `vk::Semaphore` so it can be `Arc`-shared for the
//! deferred PRESENT completion path. Destruction happens on the last Arc
//! drop (via `vkDestroySemaphore`), independent of the X11 resource id's
//! lifetime.
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

    /// Signal a timeline-semaphore value via `vkSignalSemaphore`.
    pub(crate) fn signal_vk(&self, value: u64) -> Result<(), vk::Result> {
        crate::kms::vk::sync::signal_timeline(&self.vk, self.semaphore, value)
    }

    /// Test-only constructor: holds a null semaphore handle. Drop is a no-op
    /// (vkDestroySemaphore on null is allowed by the Vulkan spec but the
    /// guard below skips it anyway).
    #[cfg(test)]
    pub(crate) fn for_tests_dummy(vk: Arc<VkContext>) -> Self {
        Self {
            vk,
            semaphore: vk::Semaphore::null(),
        }
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

- [ ] **Step 2: Delete the Vulkan syncobj import**

In `crates/yserver/src/kms/vk/sync.rs`, delete `import_drm_syncobj` (lines
55-80, doc comment included). Leave `import_sync_file`, `export_sync_file` and
`signal_timeline` alone — all three still serve the fence path.

- [ ] **Step 3: Build and let the compiler find the stragglers**

Run: `cargo build -p yserver 2>&1 | head -40`
Expected: PASS. If anything still references `new_drm_syncobj`,
`import_drm_syncobj`, or `OwnedSemaphore::timeline_value`, the compiler names
it — fix those call sites to use `ImportedSyncobj` rather than reinstating
what you just deleted. If a call site cannot be converted, stop and report:
it means a consumer was missed in the design.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test -p yserver --lib`
Expected: PASS.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/render/owned_semaphore.rs crates/yserver/src/kms/vk/sync.rs
git commit -m "refactor(dri3): drop the Vulkan syncobj import

OwnedSemaphore carried a retained DRM handle, an eventfd registration and
a timeline query that existed only for the syncobj half, now served by
ImportedSyncobj. What remains is the XSync fence resource it always was.

import_drm_syncobj goes with it: vkImportSemaphoreFdKHR on a DRM syncobj
fd is a Mesa-only accident, not a portable interop path."
```

---

### Task 5: Derive the capability from the kernel, not the driver

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:18967-18992`
  (`dri3_capabilities`)
- Modify: `crates/yserver/src/kms/vk/device.rs:87-89` (delete
  `supports_dri3_syncobj`)
- Test: inline `#[cfg(test)] mod tests` in `backend.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks at the type level; depends on Tasks 1-4
  being done, because this is the step that starts advertising 1.4.
- Produces: free function `dri3_version_for(syncobj: bool) -> (u32, u32)`.

**This task turns the feature on. Do it last.**

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `backend.rs`:

```rust
/// The DRI3 version follows syncobj support: 1.4 advertises
/// ImportSyncobj/FreeSyncobj and, through the mirrored Present
/// capability, PresentPixmapSynced. Without syncobj support the server
/// must stay at 1.3 or clients will send requests it cannot serve.
#[test]
fn dri3_version_follows_syncobj_support() {
    assert_eq!(dri3_version_for(true), (1, 4));
    assert_eq!(dri3_version_for(false), (1, 3));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p yserver --lib dri3_version_follows -- --nocapture`
Expected: FAIL — `dri3_version_for` is not defined.

- [ ] **Step 3: Rewrite the capability derivation**

Add this free function in `backend.rs` immediately above the `impl Backend for
KmsBackend` block that contains `dri3_capabilities`:

```rust
/// DRI3 version for a given syncobj capability. 1.4 is the version that
/// carries `ImportSyncobj` / `FreeSyncobj`; without them the server caps at
/// 1.3 and clients fall back to the fence path.
fn dri3_version_for(syncobj: bool) -> (u32, u32) {
    if syncobj { (1, 4) } else { (1, 3) }
}
```

Replace the body of `dri3_capabilities` (`:18967-18992`):

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
        // driver: the resource is a DRM syncobj and every operation on it is
        // a DRM ioctl. The previous NVIDIA blacklist here was really a
        // symptom of vkImportSemaphoreFdKHR rejecting DRM syncobj fds, which
        // no longer matters because nothing imports them into Vulkan.
        let syncobj = ::drm::Device::get_driver_capability(
            self.platform.device.as_ref(),
            ::drm::DriverCapability::TimelineSyncObj,
        )
        .is_ok_and(|v| v != 0);
        Dri3Caps {
            version: dri3_version_for(syncobj),
            modifiers,
            fence_fd,
            syncobj,
        }
    }
```

**Watch the trait path:** `get_driver_capability` is on the `drm::Device` root
trait, *not* on `drm::control::Device`, which is the one this file already
imports for the syncobj calls. `crates/yserver/src/kms/cursor_plane.rs:30-34`
shows the distinction — it imports `Device as DrmDevice` alongside
`control::{Device as ControlDevice, ...}`. Use a `use` alias rather than the
fully-qualified call if that reads better, but do not assume one trait has the
other's methods.

- [ ] **Step 4: Delete the driver blacklist**

In `crates/yserver/src/kms/vk/device.rs`, delete `supports_dri3_syncobj`
(lines 87-89) together with its doc comment.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p yserver --lib`
Expected: PASS. If a test asserted DRI3 1.3 on this box, it was asserting the
blacklist; update it to assert the kernel-derived value.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add crates/yserver/src/kms/render/backend.rs crates/yserver/src/kms/vk/device.rs
git commit -m "feat(dri3): advertise 1.4 whenever the kernel has timeline syncobj

supports_dri3_syncobj() blacklisted NVIDIA_PROPRIETARY, which capped DRI3
at 1.3 and left PresentPixmapSynced untestable on the nvidia box. The
real condition was never the Vulkan driver: it is whether the kernel
exposes DRM_CAP_SYNCOBJ_TIMELINE, which is what the Phase 4.2 design's
fallback matrix said in the first place.

Measured on the nvidia box: nvidia-drm serves the whole syncobj path, and
libGLX_nvidia resolves import_syncobj plus present_pixmap_synced once per
swapchain against a 1.4 server."
```

---

### Task 6: Hardware validation and documentation

**Files:**
- Modify: `docs/status.md`
- Modify: `docs/superpowers/specs/2026-08-08-dri3-syncobj-drm-signal-design.md`
  (Status line)

- [ ] **Step 1: Run the server and capture**

Start yserver and run a Vulkan client through X11 WSI. It must be Vulkan, not
GL — `docs/status.md:4084` records NVIDIA's libGL failing to bind DRI3 against
yserver for unrelated reasons, so a GL client cannot answer either way.

```bash
# `just yserver-mate-hw` (Justfile:293) takes a log spec. The wire-level
# DRI3/PRESENT debug lines live on the default target, so present_pace
# alone is NOT enough — process_request must be included or ImportSyncobj
# never appears and the capture looks like a negative result.
just yserver-mate-hw "info,present_pace=debug,yserver_core::core_loop::process_request=debug"

# inside the session, forcing the Vulkan X11 WSI:
mpv --gpu-api=vulkan --vo=gpu-next --gpu-context=x11vk \
    --length=15 av://lavfi:testsrc=size=1280x720:rate=60
```

Copy the log aside immediately (`cp yserver-hw-mate.log dri3-syncobj-<date>.log`).
Every yserver start truncates `yserver-hw-mate.log` and
`yserver-mate.submit.tsv` at the repo root with `>` plus `rm -f`, including a
normal desktop session — analyse or copy before the next run or the data is
gone.

- [ ] **Step 2: Check the four things that must appear**

```bash
LOG=dri3-syncobj-<date>.log
grep -c "DRI3::QueryVersion.*-> 1.4" "$LOG"      # server advertises 1.4
grep -c "DRI3::ImportSyncobj" "$LOG"             # client takes the path
grep -c "present acquire" "$LOG"                 # acquires resolve
grep -c "signal release syncobj.*failed" "$LOG"  # must be 0 or explained
```

A non-zero count on the last one is most likely the pre-existing freed-syncobj
bookkeeping bug (`docs/status.md:567`) becoming reachable, not a regression
from this change. Record which it is; do not fix it here.

- [ ] **Step 3: Update `docs/status.md`**

Add an entry with: the capability now deriving from
`DRM_CAP_SYNCOBJ_TIMELINE`; that `PresentPixmapSynced` is no longer
"structurally untestable" (amend the claim at `:316`, which named this exact
gate); the counts from Step 2; and whether the freed-syncobj signals appeared.

- [ ] **Step 4: Flip the spec's status line**

Change the spec header from `**Status:** DESIGN (2026-08-08)` to
`IMPLEMENTED`, with the hardware result and the box it ran on, following the
style of `2026-07-20-nvidia-gbm-scanout-allocation.md`.

- [ ] **Step 5: Commit**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
git add docs/status.md docs/superpowers/specs/2026-08-08-dri3-syncobj-drm-signal-design.md
git commit -m "docs(dri3): record the DRM-signalled syncobj result on hardware"
```

---

## Deferred / out of scope

- **Cross-driver validation.** The Mesa path changes from a Vulkan host signal
  to a DRM host signal and there is no AMD or Intel GPU on this box. The bee
  (6900HX / RADV) is where that gets confirmed.
- **The freed-syncobj bookkeeping bug** (`docs/status.md:567`). This change
  makes it reachable here; it does not cause it.
- **GPU-side release signalling.** Signalling the client's release point from
  the queue instead of the host would let clients unblock earlier, but it is a
  separate design with its own measurement, and it is impossible on NVIDIA
  (the import that would be required is what fails). See the spec's root-cause
  section for why the queue-wait half of that idea is foreclosed by the single
  shared queue.
