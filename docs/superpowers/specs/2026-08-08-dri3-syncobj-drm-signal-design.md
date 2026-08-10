# DRI3 1.4 syncobj: signal through DRM, not Vulkan

**Status:** IMPLEMENTED (2026-08-10, branch `dri3-syncobj-drm-signal`). HW-verified
on the nvidia box, full-Mesa: yserver on the Raphael iGPU (**card1**, RADV,
`renderD129`) + mpv Vulkan X11 WSI reproduced a 15 s testsrc; `DRI3::QueryVersion
-> 1.4`, 6 `ImportSyncobj` imports, 192 synced presents with 1 acquire deferred
and subsequently signalled, 0 `unknown syncobj`, 0 `DRM eventfd unavailable`
fallback warnings — the spec's merge gate (non-zero deferrals, no fallback) is
satisfied. IGT `syncobj_basic` 12/12 on nvidia-drm; `syncobj_eventfd/wait/timeline`
skip because the 7.1 kernel compiles CONFIG_SW_SYNC but does not expose
`/dev/sw_sync`.

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
deferred-Present design has never run **on this box** — `docs/status.md:297`
records it as *"structurally untestable on this box"*, naming this exact gate.

**That qualifier carries the risk profile of this whole change and must not be
dropped.** The path is not unvalidated. `docs/status.md:407-424` records it
passing on bee **and** silence (Mesa/RADV) on 2026-07-28, across single- and
dual-head and three desktop environments, on a run carrying 2,221 synced
requests of which 473 (21.3%) arrived before their acquire point, were
deferred, and all 473 subsequently signalled — 0.87 ms mean, 6 ms maximum, no
fallback warning.

So the direction of this change is the opposite of what "untested path" would
suggest: it replaces the release signal *and* the readiness poll on the one
stack where the path is **proven**, in order to reach a stack where it has
never run. Validation planned only on NVIDIA cannot, by construction, detect a
Mesa regression. See "Risks" for what now covers that.

The gate arrived in commit `8c68a281` (2026-05-20) with a one-line message and
an empty body, but its rationale **is** recorded — in a doc comment the same
commit added directly above the function (`crates/yserver/src/kms/vk/device.rs:78-85`,
restated at `docs/status.md:297`):

> The implementation currently imports DRI3 syncobj fds as timeline semaphores
> with `OPAQUE_FD`. That works on the Vulkan stacks we have used for
> Venus/Mesa testing, but NVIDIA proprietary rejects the very first import with
> `ERROR_INITIALIZATION_FAILED` […]. Advertising only DRI3 1.3 on that driver
> lets clients fall back to the older fence-fd path instead of dying on
> `ImportSyncobj`.

So the gate was a correct, documented response to a real failure. What follows
is not a discovery that it was wrong; it is that the *conclusion drawn from it*
— cap the version — was one of two available responses, and the other one
serves every driver.

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
Result: `VK_ERROR_INITIALIZATION_FAILED` — the same error the 2026-05-20 doc
comment already named. This measurement **corroborates** the recorded
rationale rather than discovering it; its value is that the failure still
reproduces on driver 610.57.04 three months later, so the constraint is
structural and not a driver bug that has since been fixed.

Note that
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

**This is a departure from the Phase 4.2 design, not a restoration of it.** That
design defined syncobj support conjunctively — `OPAQUE_FD` **and**
`VK_KHR_timeline_semaphore` **and** DRM_SYNCOBJ ioctls (design:463-464, mirrored
in the shipped `Dri3Caps` doc at `trait_def.rs:290-292`) — and its fallback
matrix lists *"Kernel lacks `DRM_SYNCOBJ` ioctls"* (design:449) as one more
condition on top of the Vulkan ones, not instead of them. Dropping the two
Vulkan conjuncts is the actual change here, and it is justified by the same
thing that justifies the rest of this spec: nothing imports the syncobj into
Vulkan any more, so Vulkan's opinion of the fd stopped being relevant.

**Which fd to ask — one device, decided, not preferred.** The capability query
and every syncobj ioctl **must** issue on the same fd, and that fd is the
render node, because the render node is what DRI3 hands the client. This is a
requirement on the implementation, not a preference to be resolved during it.

The failure mode of getting it wrong is not symmetric, which is why it needs
deciding here rather than at the call site:

| Cap read on | ioctls on | Result |
|---|---|---|
| render node | render node | correct |
| KMS node | KMS node | quiet under-advertising on split boxes: 1.3 where 1.4 would have worked |
| render node | KMS node | **advertises 1.4 on the strength of one device and then operates another** |

The third row is the dangerous one and it is the easy one to write by
accident, because `PlatformBackend` exposes the KMS node as a
`crate::drm::Device` (ready to `clone()` into the new type) while the render
node is only a bare `render_node_fd`. Reaching for the convenient field is
exactly how capability and use end up on different devices. Whatever
`ImportedSyncobj` holds must therefore be derived from the render node; if
that means `PlatformBackend` has to retain a `Device` for the render node
rather than an fd, that is part of this change, not an obstacle to it.

On this box the two nodes are the same device, so no local test can catch the
mismatch. The project's hardware matrix has two split configurations —
Raspberry Pi 4 (vc4 display, v3d render) and Asahi (`apple-drm` card, AGX
render node) — and neither is available here. The correctness argument has to
carry it, which is why the rule is stated as an invariant.

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

- Import failure returns `BadAlloc` (`process_request.rs:11500`). This is a
  **behaviour change, not a preservation**: today a `fd_to_syncobj` failure
  only warns and falls back to the Vulkan-only path
  (`backend.rs:19330-19339`), and just the Vulkan import failing yields
  `BadAlloc`. With no Vulkan path left there is nothing to fall back to, and
  the kernel-derived capability means a `fd_to_syncobj` failure on a device
  that advertised `TimelineSyncObj` is a genuine error rather than an expected
  driver gap.
- Eventfd registration failure still falls through to polling, now via DRM
  query.
- A kernel without `TimelineSyncObj` degrades to `Dri3Caps.syncobj = false` and
  version `(1, 3)` — the same degradation as today, keyed on the kernel rather
  than on a driver list.

### Protocol conformance: one root cause, four symptoms

Today the syncobj registry is a bare `HashMap<u32, Arc<…>>` with **no owning
client**. Xorg instead makes a syncobj a first-class X resource —
`dri3_syncobj_type = CreateNewResourceType(dri3_syncobj_free, "DRI3Syncobj")`
(`dri3/dri3.c:106`) — and every conformance property below falls out of that
one decision. Lifting the 1.3 cap is what first makes these reachable by a
real client, so they belong to this change even though the registry predates
it.

Verified against the local Xorg checkout (`~/Projects/xserver`):

| # | Contrato que debe cumplirse | Xorg | yserver (este cambio) |
|---|---|---|---|
| 1 | Un xid que no es legal para el cliente se rechaza (convención core X11 para todo recurso nuevo) | `LEGAL_NEW_RESOURCE(stuff->syncobj, client)` antes de cualquier trabajo → BadIDChoice (`dri3/dri3_request.c:609`) | `BadAlloc` — divergencia deliberada de códigos (ver "Divergence from Xorg") |
| 2 | Un request sin fd se rechaza | `if (fd < 0) return BadValue` (`dri3/dri3_request.c:619-620`) | `BadAlloc` — divergencia deliberada |
| 3 | Ningún cliente puede liberar el syncobj de otro | `FreeSyncobj` hace `dixLookupResourceByType(…, dri3_syncobj_type, client, DixWriteAccess)` y devuelve su status (`dri3/dri3_request.c:634-637`) | ownership enforced; un solo código `BadValue` — divergencia deliberada |
| 4 | `PresentPixmapSynced`: syncobj None/no-importado o points ilegales → **Value error** (presentproto 1.4 §7) | `VERIFY_DRI3_SYNCOBJ` en ambos xids, luego `BadValue` si algún point es 0 o `acquire >= release` en el mismo syncobj (`present/present_request.c:296-302`) | `BadValue` — protocolo presentproto 1.4, sin divergencia |

Symptom 4 is the worst of the four: an X client that gets no reply and no
error has no recovery, and it is indistinguishable from a server hang.

The same missing ownership causes the leak recorded under "Risks": Xorg's
resource system frees a client's syncobjs on disconnect for free, whereas
yserver's map is only ever pruned by an explicit `FreeSyncobj`.

**Scope decision.** Give the registry an owning client and error semantics.
That is the minimum that makes DRI3 1.4 safe to advertise: rows 3 and 4 are
cross-client and denial-of-service shaped respectively, not cosmetic
divergences. yserver does **not** adopt Xorg's resource-type machinery:
syncobjs stay a backend `HashMap` with an owner field (HLD non-goal "being a
drop-in clone of Xorg internals"). The exact error codes in rows 1-3 diverge
from Xorg deliberately — see "Divergence from Xorg" below. Only row 4's Value
errors are protocol-mandated (presentproto 1.4).

### Divergence from Xorg — deliberate, and why it is not a spec violation

AGENTS.md's "follow Xorg" rule exists for behaviour real 40-year-old clients
observe on the wire. It does not obligate yserver to replicate Xorg's error
codes where the protocol is silent, nor Xorg's internal resource machinery.
For DRI3 1.4 syncobjs this change deliberately diverges in two ways, both
consistent with the HLD non-goals:

1. **Resource model.** Xorg makes a syncobj a first-class X resource
   (`dri3_syncobj_type = CreateNewResourceType(dri3_syncobj_free, "DRI3Syncobj")`).
   yserver keeps syncobjs in a backend `HashMap<u32, (ClientId,
   Arc<ImportedSyncobj>)>`. The owner field reproduces the observable
   behaviour (ownership checks, disconnect purge) without cloning the dix
   resource machinery — non-goal "being a drop-in clone of Xorg internals".
2. **Error codes.** The protocol specifies exact error codes for
   `PresentPixmapSynced` (presentproto 1.4: Value errors for None/not-imported
   syncobjs, zero points, and `acquire >= release` on the same syncobj) —
   those are kept verbatim (row 4). It does **not** specify codes for
   `ImportSyncobj` / `FreeSyncobj` failures, so yserver uses its own,
   consistent with the rest of its DRI3 surface: `BadAlloc` for any
   `ImportSyncobj` failure (xid invalid, missing fd, import error) and
   `BadValue` for `FreeSyncobj` failures (unknown, or not the owning client).
   Xorg's BadIDChoice / BadAccess distinction is an implementation accident,
   not a client contract — no modern DRI3 client branches on it — and
   replicating it would be "preserving behavior that exists only because of
   Xorg implementation accidents" (HLD non-goal).

What matters is the hang-free property, which both Xorg and this design
satisfy: every failure path emits an X error rather than blocking with no
reply.

### Monotonicity: measured, and not what was first assumed

An earlier draft of this spec claimed both primitives reject a stale point and
called the swap semantically identical. That is **false**, measured on this
kernel (`~/yserver-glx-probes/stalepoint.c`, both nodes):

```
signal point=10  rc=0  -> query reads 10
signal point=5   rc=0  -> query reads 10   <- stale point SUCCEEDS, silently
signal point=10  rc=0  -> query reads 10   <- duplicate SUCCEEDS
signal point=20  rc=0  -> query reads 20
```

`DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL` clamps to `max(current, point)` and
returns success. On the Vulkan side, `value` exceeding the current counter is a
VUID on `vkSignalSemaphore` — undefined behaviour, not an error return. So
neither primitive reports an out-of-order signal, and the swap is **not**
"unchanged".

It is, however, safer. `signal_all_retained_present_wakes`
(`backend.rs:6198-6203`) iterates `retained_present_wakes`, a `HashMap`, so
shutdown flushes release points in arbitrary order. Under `vkSignalSemaphore`
that is UB with a real risk of leaving a payload below its highest signalled
point; under DRM the clamp guarantees the maximum wins. Out-of-order release
signalling stops being a hazard and becomes merely silent.

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
   Validation client must be Vulkan, not GL: `status.md:4063` records that
   NVIDIA's libGL fails to bind DRI3 against yserver for unrelated reasons, so
   a GL client cannot answer either way. Expected observations: `ImportSyncobj`
   arriving at the dispatcher and deferred acquires in the `present_pace` log.

   Note the success path of `ImportSyncobj` logs nothing today — the only
   `DRI3::ImportSyncobj` line in the tree is the `BadAlloc` branch
   (`process_request.rs:11493`), so a healthy run is indistinguishable from a
   run where no client ever sent the request. A success-path `debug!` has to be
   added for this validation to mean anything.
4. **Hardware, Mesa — a merge gate, not an extra.** NVIDIA validation proves
   the change reaches a stack that never worked; it says nothing about the
   stack the change puts at risk. A Mesa run must reproduce the 2026-07-28
   result qualitatively: synced presents arriving before their acquire point,
   deferred, and *all* subsequently signalled, with no fallback warning. A run
   with zero deferrals proves nothing — it means the acquire points were
   already met and the release path was never exercised under contention, so
   the deferred count must be non-zero for the run to count.

   Run it on the local RADV device — `card1` drives the display as of
   2026-08-08, so this is available now; the bee is the fallback. Both env
   overrides in "Risks" are mandatory, or the run silently renders on NVIDIA
   and proves nothing about Mesa.

   Compare against the recorded baseline (2,221 requests / 473 deferred /
   0.87 ms mean) as a shape, not a threshold: the iGPU is 2 CUs against a
   6900HX, so absolute timings will differ and a slower mean is not a
   regression. What must match is the invariant — every deferred acquire
   eventually signals, and no fallback warning appears.

## Risks / open questions

- **Pre-existing bookkeeping bug becomes reachable, and stays invisible.**
  `status.md:548` records *"1,676 failed idle-syncobj signals across several
  destroyed Vulkan child surfaces"* — the scheduler retains completed frames
  and treats them as pending after their syncobjs are freed. Not fixed here.
  Two sub-cases, and neither produces a new signal to watch for:
  - Freed xid: `dri3_signal_syncobj` fails at its own registry miss
    (`backend.rs:19384-19392`) with the same `unknown syncobj` text before and
    after this change. It never reaches an ioctl.
  - Pinned-handle replay (`signal_present_wake` →
    `dri3_signal_syncobj_via_handle`, `backend.rs:19568-19576`): the object is
    alive, and per the monotonicity measurement above a stale replayed release
    point returns success silently. No warning in either world.

  So this change makes the bug *reachable on this box* without making it more
  observable. If it needs watching during validation, the string to grep is
  `unknown syncobj`.
- **No client-teardown purge for the syncobj registry.** Nothing removes
  entries except `FreeSyncobj`, so a client that dies with syncobjs imported
  leaks them. Pre-existing in shape — today it leaks a `VkSemaphore` plus a DRM
  handle — but this change is what makes DRI3 1.4 reachable on the box where it
  will actually be exercised, so the exposure is new in practice. **No longer
  out of scope:** it shares its root cause with the conformance table above
  (the registry has no owning client), so the ownership work that fixes rows 3
  and 4 is what closes this too. Fixing it separately would mean building the
  same ownership twice.
- **The polling fallback fails open.** `PendingPresentSourceWait::is_ready`
  treats a failed timeline query as ready and proceeds to copy. On NVIDIA that
  arm was unreachable (the import always failed, so no client got this far);
  after this change it is live, and failing open means copying a buffer whose
  producer may not have finished writing. The arm predates this change and is
  not altered by it, but it acquires teeth here.
- **Cross-driver validation is mandatory, and is now possible locally.** The
  Mesa path changes from a Vulkan host signal to a DRM host signal on the one
  stack where `PresentPixmapSynced` is proven (see "Problem"), so a Mesa run is
  a merge gate, not a nice-to-have. As of 2026-08-08 this box has a second,
  Mesa device: the Ryzen 7 7700 Raphael iGPU (`1002:164e`, gfx1036) on
  `card1` / `renderD129`, with RADV built (Mesa 26.1.6,
  `AMD Ryzen 7 7700 (RADV RAPHAEL_MENDOCINO)`). Measured the same day:
  - `syncobjprobe /dev/dri/renderD129` passes 12/12, so the DRM path is viable
    on amdgpu as well as nvidia-drm.
  - Cross-device syncobj sharing works **both directions**
    (`crosssyncobj.c`): create on one GPU, export the fd, `FDToHandle` on the
    other, `TimelineSignal` there, and the creator's eventfd, `Query` and
    `TimelineWait` all observe it. So a PRIME client on one GPU can be released
    by a server on the other.

  Two grades of coverage follow, and they are not equivalent:
  - **Client-only Mesa** (yserver on NVIDIA, RADV client over PRIME) needs no
    hardware change and exercises the interaction that matters — does a DRM
    host signal wake a RADV waiter. It adds one untested variable: NVIDIA
    importing an AMD-allocated dmabuf.
  - **Full Mesa** (yserver and client both on RADV/card1) is the real
    equivalent of the bee run. It needs a CRTC on `card1`, plus
    `YSERVER_DRM_DEVICE=/dev/dri/card1` **and**
    `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/radeon_icd.x86_64.json` — the
    second is not optional, see the device-selection defect below.

  **Resolved 2026-08-08, and it inverted which side is blocked.** The display
  now runs off the board's port: `card1-HDMI-A-1` is `connected`, 256-byte
  EDID, 31 modes, top mode 1920x1080 — the same resolution as the existing
  hardware-matrix entries, so timings stay comparable. The full-Mesa run is
  therefore unblocked.

  What is blocked now is the **NVIDIA** run: `card0` has no connected connector
  at all, so `discover_outputs` will find nothing there. Restoring it means
  either moving the cable back or a synthetic EDID on `card0-HDMI-A-2`
  (`drm.edid_firmware=HDMI-A-2:edid/tv.bin video=HDMI-A-2:e`; kernel 7.1
  removed the built-in EDID sets, so the blob must be supplied — copy it from
  `/sys/class/drm/card1-HDMI-A-1/edid` while that connector is live). Plan the
  two hardware runs as separate sessions rather than assuming both are
  available at once.

  Note the box is now a PRIME desktop — scanout on `card1` (amdgpu), every
  client rendering on `renderD128` (nvidia-drm), `renderD129` unused. So
  "there is an AMD GPU present" does not by itself mean a given process is
  exercising Mesa; check which render node the client actually opened.
- **Pre-existing defect, out of scope, newly reachable: the server can scan out
  and render on different GPUs.** `resolve_drm_device` (`lib.rs:658`) picks the
  KMS card by connected connector and `render_node::open_for_card` correctly
  follows the sysfs sibling, but `pick_physical_device`
  (`kms/vk/device.rs:588`) scores purely by
  `DISCRETE_GPU=3 > INTEGRATED_GPU=2`, with no correlation to the chosen DRM
  device. On a two-GPU box, pointing `YSERVER_DRM_DEVICE` at the iGPU yields
  KMS on AMD and Vulkan on NVIDIA, silently. This is not caused by this change,
  but this change's validation plan is what first depends on getting it right.

  **As of 2026-08-08 this is the default path on the nvidia box, not an edge
  case.** With the display moved to the board, `card1` is the only card with a
  connected connector, so `pick_drm_candidate` selects it while
  `pick_physical_device` still scores `DISCRETE_GPU` highest and selects the
  NVIDIA device. Launching yserver here with no environment overrides now
  produces the mismatched pair. Any hardware run for this change must set both
  `YSERVER_DRM_DEVICE=/dev/dri/card1` and
  `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/radeon_icd.x86_64.json`, and a run
  that omits them is not evidence about Mesa regardless of what it reports.
- **Symbol resolution is not invocation.** Measurement 3 shows NVIDIA resolving
  the client-side entry points, not calling them. Direct confirmation arrives
  with the hardware validation above.
