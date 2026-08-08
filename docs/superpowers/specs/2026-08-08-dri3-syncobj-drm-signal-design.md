# DRI3 1.4 syncobj: signal through DRM, not Vulkan

**Status:** DESIGN (2026-08-08). Hardware evidence gathered on the nvidia box
(RTX 5060 Ti, driver 610.57.04, kernel 7.1.6-gentoo-dist). Not yet implemented.

**Related:** `2026-05-09-phase4-2-dri3-present-glx-design.md` (§3.3 defined the
Vulkan route this supersedes), `2026-07-20-nvidia-gbm-scanout-allocation.md`
(same shape of decision — leaving a Vulkan-native path for the ecosystem-standard
one), `2026-07-31-present-deferred-execution-supersession-design.md` (its
`PresentPixmapSynced` branch is the one this unblocks for testing).

**Probes:** `~/yserver-glx-probes/syncobjprobe.c`, `vksyncobjprobe.c`,
`dri3ver.c`, `syncwatch.c` + `run-syncwatch.sh`.

## Problem

yserver advertises DRI3 1.3 instead of 1.4 on NVIDIA proprietary, so
`ImportSyncobj`, `FreeSyncobj` and `PresentPixmapSynced` are never offered.
The cap comes from `supports_dri3_syncobj()`
(`crates/yserver/src/kms/vk/device.rs:87`), a driver blacklist:

```rust
!matches!(self.driver_id, vk::DriverId::NVIDIA_PROPRIETARY)
```

Two costs follow. Clients that want explicit sync silently fall back to the
1.3 fence path. And the `PresentPixmapSynced` branch of the merged
deferred-Present design has never run on hardware — `docs/status.md:316`
records it as *"structurally untestable on this box"*, naming this exact gate.

The gate arrived in commit `8c68a281` (2026-05-20). Its message is one line,
`Disable DRI3 syncobj on NVIDIA`, with an empty body. No rationale was
recorded anywhere, so the premise had to be re-measured rather than trusted.

## Evidence

Three measurements, all on the nvidia box, 2026-08-08.

**1. The kernel supports the whole DRM syncobj path.** `syncobjprobe` runs the
server's exact sequence — `fd_to_syncobj`, `timeline_signal`, cross-handle
`timeline_wait`, `query`, `eventfd` + signal — and passes 12/12 on both
`/dev/dri/card0` and `/dev/dri/renderD128`. `DRM_CAP_SYNCOBJ` and
`DRM_CAP_SYNCOBJ_TIMELINE` are both 1. Two handles imported from one fd alias
the same payload, which is what makes a server-side signal reach the client.

**2. The Vulkan import genuinely fails.** `vksyncobjprobe` replays
`import_drm_syncobj` (`crates/yserver/src/kms/vk/sync.rs:58`) byte for byte:
timeline `VkSemaphore`, `vkImportSemaphoreFdKHR` with
`VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_OPAQUE_FD_BIT`, on a real DRM syncobj fd.
Result: `VK_ERROR_INITIALIZATION_FAILED`.

The gate's premise was correct. Note that
`vkGetPhysicalDeviceExternalSemaphoreProperties` reports
`IMPORTABLE_BIT` set for timeline OPAQUE_FD on this driver — it advertises the
capability and then rejects the fd. That is not a driver bug: `OPAQUE_FD` is a
driver-private payload format and the Vulkan spec never promised it is a DRM
syncobj. Mesa makes the two identical, which is why the path works on RADV.
**Consequence for the design: there is no capability query that can predict
this.** Detection must be either a trial import or, as chosen below, a
question asked of the kernel instead.

**3. A real client on this box is waiting for explicit sync.**
`libGLX_nvidia.so.0` — which `nvidia_icd.json` also names as the Vulkan ICD —
carries `xcb_dri3_import_syncobj_checked`, `xcb_dri3_free_syncobj` and
`xcb_present_pixmap_synced_checked` as strings, resolved through `dlsym` rather
than linked (which is why an undefined-symbol scan misses them). `syncwatch`
interposes `dlsym` and confirms the lookups fire at runtime: mpv on Vulkan X11
WSI against Xwayland `:0` (verified at DRI3 1.4 / Present 1.4 by `dri3ver`)
resolves the full triad three times, once per swapchain rather than once at
init.

Residual uncertainty: symbol resolution is not proof of invocation. The
per-swapchain repetition and the completeness of the triad make use the
overwhelmingly likely reading, and lifting the gate converts this to direct
observation of `ImportSyncobj` arriving at the dispatcher.

For contrast, no other installed client takes this path: Mesa 26.0.8
(`libgallium`) uses `xcb_dri3_fence_from_fd`, the 1.3 route.

## Root cause

The Phase 4.2 design specified the Vulkan route deliberately
(`2026-05-09-phase4-2-dri3-present-glx-design.md:385-392`):

> `PresentPixmapSynced` (v1.4) `wait_value` / `idle_value` are 64-bit timeline
> values fed into `VkTimelineSemaphoreSubmitInfo` on the submit.

The point was to wait and signal *inside the queue submit*, keeping the CPU out
of the loop — something Xorg cannot do and yserver, being a Vulkan compositor,
could. Importing the client's syncobj into a `VkSemaphore` is what that plan
required.

**That plan did not survive hardware.** Commit `b92b3dd7` (2026-07-28, round 3
of three) rewrote the acquire path to *"follow Xorg's non-blocking Copy-path
ordering"*: a DRM timeline eventfd plus deferral of the copy, with Vulkan
counter polling only as a fallback. Deferral is what fixed fullscreen mpv; a
GPU-side wait was not the answer.

The reason it was not the answer still holds. yserver has **one queue**
(`crates/yserver/src/kms/vk/device.rs:299` — one family, one
`get_device_queue`), shared between whole-screen compositing and every client.
A GPU-side wait on a client's acquire point parks that single queue behind that
client. `b92b3dd7` measured 473 of 2,221 synced presents (21.3%) arriving
before their acquire point; under a queue wait each of those would have stalled
the compositor instead of being deferred.

So the imported `VkSemaphore` is left holding no architectural purpose. It
enters no `queue_submit` anywhere — the only consumers of `.semaphore()` are
`dri3_fd_from_fence` and two `PresentCompletionSignal` sites, a different type.
Its one remaining job, the release signal, is `vkSignalSemaphore`, which is a
**host** operation. It is already a CPU signal.

## Fix

Serve DRI3 syncobjs through the kernel interface that owns them, on every
driver. Replace `vkSignalSemaphore` with `DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL`
and `vkGetSemaphoreCounterValue` with `DRM_IOCTL_SYNCOBJ_QUERY`, at the same
call sites. Both replacements are host-side, exactly like what they replace, so
no ordering guarantee changes.

Everything needed already exists in the `drm` 0.15 crate —
`syncobj_timeline_signal` (`control/mod.rs:947`), `syncobj_timeline_query`
(:913), `syncobj_eventfd` (:957), `fd_to_syncobj` (:851). No new ioctl
plumbing, so the AGENTS.md warning about `libc::Ioctl` typing across
glibc/musl/FreeBSD does not come into play.

### Components

`dri3_sync_resources` (`crates/yserver/src/kms/render/backend.rs:805`) is today
a single `HashMap<u32, Arc<OwnedSemaphore>>` serving two unrelated X resource
types:

| Resource | Registered by | What it actually needs |
|---|---|---|
| XSync `Fence` | `FenceFromFD` (:19264) | Vulkan — `FDFromFence` exports a sync_file from the `VkSemaphore` (:19308) |
| DRI3 `Syncobj` | `ImportSyncobj` (:19373) | No Vulkan — signal, eventfd and query all have DRM ioctls |

The fence half legitimately needs Vulkan, and works on NVIDIA already because
those semaphores are Vulkan-created rather than imported. So `OwnedSemaphore`
cannot simply be stripped. Split the types instead:

- **`OwnedSemaphore`** keeps its Vulkan body and serves only
  `FenceFromFD` / `FDFromFence`. Its `drm_syncobj` field, `signaled_eventfd()`
  and `timeline_value()` are removed — they existed solely for the syncobj half.
- **`ImportedSyncobj`** (new module) holds `Arc<crate::drm::Device>` plus a
  `syncobj::Handle`, and nothing else. It implements `SyncobjHandle::signal`
  over `syncobj_timeline_signal`, plus `timeline_value()` over
  `syncobj_timeline_query` and `signaled_eventfd()` over `syncobj_eventfd`. No
  `Arc<VkContext>`.
- **`dri3_sync_resources` splits into two typed maps.** Beyond clarity this
  removes a latent confusion: today `FDFromFence` on a syncobj xid resolves
  into the same map and half-works.
- **`present_source_wait.rs:22`** changes `syncobj_pin` from
  `Arc<OwnedSemaphore>` to `Arc<ImportedSyncobj>`, and the readiness check at
  :41 moves from `vkGetSemaphoreCounterValue` to `syncobj_timeline_query`.
- **Deleted:** `import_drm_syncobj` (`vk/sync.rs:58`) and
  `supports_dri3_syncobj` (`vk/device.rs:87`).

### Capability derivation

`Dri3Caps::syncobj` (`backend.rs:18967`) stops asking the Vulkan driver and
asks the DRM device: `DriverCapability::TimelineSyncObj` (exposed by the `drm`
crate at `lib.rs:309`). Version becomes `(1, 4)` when that capability is
present and `(1, 3)` when it is not.

This is what the Phase 4.2 design already listed as the correct gate — its
fallback matrix names *"Kernel lacks `DRM_SYNCOBJ` ioctls"* (design:449) as the
condition for dropping to 1.3. It was simply never implemented that way.

The Present extension's syncobj capability mirrors `Dri3Caps::syncobj`
(design:470), so it follows automatically — that bit is how a client learns
`PresentPixmapSynced` is usable, and it must not be advertised without the DRI3
half.

### Data flow

Unchanged in shape; only the executor of each step differs.

| Step | Today | After |
|---|---|---|
| `ImportSyncobj` | `dup` + `fd_to_syncobj` + `import_drm_syncobj` | `fd_to_syncobj` on the borrowed fd |
| acquire wait | `syncobj_eventfd` on the retained DRM handle | unchanged |
| acquire fallback | `vkGetSemaphoreCounterValue` | `syncobj_timeline_query` |
| release signal | `vkSignalSemaphore` | `syncobj_timeline_signal` |
| `FreeSyncobj` | Arc drop → `destroy_syncobj` + `vkDestroySemaphore` | Arc drop → `destroy_syncobj` |

The `dup` at `backend.rs:19329` disappears: it existed only because
`vkImportSemaphoreFdKHR` consumes the fd, so a copy was needed for the DRM
handle. `fd_to_syncobj` takes a `BorrowedFd`.

### Error handling

- Import failure returns `BadAlloc`, preserving current behaviour
  (`process_request.rs:11500`).
- Eventfd registration failure still falls through to polling, now via DRM
  query.
- A kernel without `TimelineSyncObj` degrades to `Dri3Caps.syncobj = false` and
  version `(1, 3)` — the same degradation as today, keyed on the kernel rather
  than on a driver list.
- DRM timelines require strictly increasing points, as `vkSignalSemaphore`
  does. Signalling a stale point fails on both. Semantics unchanged.

## Alignment with the HLD

The HLD lists as non-goals *"being a drop-in clone of Xorg internals"* and
*"preserving behavior that exists only because of Xorg implementation
accidents"*, and describes yserver as driving DRM/KMS *"via Vulkan"*. This
change deserves to be checked against that, since it moves work out of Vulkan.

It does not conflict, for three reasons.

**The ioctl is not an Xorg internal.** A DRM syncobj is a kernel object and
`DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL` is its owner's interface. Vulkan's external
semaphore import is an interop *bridge* to that object, and measurement 2 shows
the bridge does not exist on this driver. Xorg, mutter and Mesa all use the
kernel interface because it is the only one, not because of any Xorg
convention.

**There is precedent in this repo, for the same reason.** The GBM scanout spec
(2026-07-20) states plainly: *"yserver allocates scanout the non-standard way
(Vulkan), instead of via GBM like the whole ecosystem"*, and corrects an
earlier belief it calls *"too hasty"*. Same decision shape, different object.
The HLD's own phrasing is "DRM/KMS **and** Vulkan"; the server already speaks
DRM directly for modeset, atomic commit, `AddFB2` and scanout allocation.

**The CPU is not newly involved.** `vkSignalSemaphore` is already a host
operation and the acquire path already waits on a kernel eventfd. The swap is
CPU-to-CPU. The genuinely GPU-side design — waits inside the queue submit — was
abandoned on measured evidence in `b92b3dd7`, and cannot be revived while the
server has a single shared queue regardless of which sync primitive is used.

Where this *would* conflict is if deferral were adopted by imitation of Xorg.
It was not; it was adopted because one queue shared with every client cannot
safely block on any of them.

## Testing

1. **Unit, no hardware.** `Dri3Caps` derivation from a capability value;
   `FDFromFence` against a syncobj xid failing cleanly now that the maps are
   separate.
2. **Integration, `#[ignore]`** following the `dri3_fd_leak.rs` convention:
   the full round trip against the real DRM node — create, export, import,
   signal, query, eventfd. This is `syncobjprobe.c` as a Rust test, so it is
   runnable on any machine rather than living only as a binary in a scratch
   directory.
3. **Hardware.** `PresentPixmapSynced` becomes exercisable for the first time.
   Validation client must be Vulkan, not GL: `status.md:4084` records that
   NVIDIA's libGL fails to bind DRI3 against yserver for unrelated reasons, so
   a GL client cannot answer either way. Expected observations: `ImportSyncobj`
   arriving at the dispatcher, deferred acquires in the `present_pace` log, and
   releases signalling without warnings.

## Risks / open questions

- **The 2026-05-20 gate may have been hiding a second problem.** It was added
  with no recorded reason, and measurement 2 explains only the import failure.
  If something else was broken on the syncobj path in May, this change makes it
  reachable again. Hardware validation is where that would surface.
- **Pre-existing bookkeeping bug becomes reachable.** `status.md:567` records
  *"1,676 failed idle-syncobj signals across several destroyed Vulkan child
  surfaces"* — the scheduler retains completed frames and treats them as
  pending after their syncobjs are freed. That bug is not fixed here, and it is
  unreachable on this box today only because the gate suppresses the whole
  path. After this change it becomes reachable, and will present as ioctl
  `ENOENT` rather than a `VkResult`. A burst of failed-release warnings after
  this lands is that bug surfacing, not a regression from it.
- **Cross-driver validation needs other hardware.** The Mesa path changes from
  a Vulkan host signal to a DRM host signal, and there is no AMD or Intel GPU
  on this box. The bee (6900HX / RADV) is the machine that would confirm it.
- **Symbol resolution is not invocation.** Measurement 3 shows NVIDIA resolving
  the client-side entry points, not calling them. Direct confirmation arrives
  with the hardware validation above.
