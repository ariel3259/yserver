# Stage 2c-i fix round — session F-4b (Task 5) review

## Verdict

**Partially accepted; the F8 stop is legitimate and is resolved below;
one new blocking finding for F-4c.** F4-B1 is closed (88 deterministic
`c0_2ci` tests — the ninth removed test was the old `Spy` predecessor of
the decisive test, and restoring it would re-violate F3; accepted).
F4-M1 is closed. F4-B2's first blocker (socket device) is genuinely fixed
— the fixture now opens the real primary node matched to the Vulkan
device's DRM identity (`VK_EXT_physical_device_drm`; this box is
multi-GPU, a blind pick paired NVIDIA's node with AMD's Vulkan device and
`ADDFB2` failed `EINVAL`), without master, and `PRIME_FD_TO_HANDLE`/`ADDFB2`/
`RMFB` succeed. F4-M2's code is the right shape but the test that would
prove it cannot run yet, and when it does it will hit a second problem
the session did not see (F4b-B1). 5.1 stays unticked.

Reviewed `e8df2de2..a9d71a82` (`f475b04c` code, `a9d71a82` fold-back).
Gate here: clippy clean, `c0_2ci` 88/88, hardware 9/10 with the decisive
test failing at `select_scanout_bo_for_rect(.., OnScreenOnly)`: "root
screenshot rect has no on-screen scanout bo".

## The F8 stop — ruling

Sonnet's finding: `BoPhase::OnScreen` is reachable only through a real
`DRM_IOCTL_MODE_ATOMIC`/`SETCRTC`, the kernel gates that on `DRM_MASTER`
unconditionally (`drm_ioctl.c`), F4-B2 forbids master, and the fixture's
synthetic output names no real CRTC anyway (`ENOENT`). Correct on all
three counts. The two pre-existing upstream tests on this fixture
(`root_get_image_reads_scanout_pixels_not_root_storage`,
`root_overlay_xor_pass_reaches_scanout`) were therefore never live
either; they now run further and fail honestly. Out of scope; noted for
the user as an upstream test-health item.

**Ruling:** the decisive test does not need a flip. `read_scanout_region`
takes a `ScanoutReadSelection`; `PermissiveDump` accepts
`OnScreen | Pending | Submitted | Recording`. After the fixture's
composite tick, the bo has been rendered into by the GPU
(`vkQueueSubmit2` precedes the atomic commit) and the commit's rejection
leaves it in a phase `PermissiveDump` accepts. The test uses
`PermissiveDump`, documents why (no master in the fixture → no flip →
never `OnScreen`; the composited pixels are real), and asserts the
selected bo's phase is one of those four so a future fixture change
cannot silently pick the wrong buffer. No `#[cfg(test)]` phase override:
that would be fixture state pretending to be a KMS outcome.

## Findings

### Blocking (for F-4c)

**F4b-B1 — after `register_managed_scanout_bo`, the pool bo is a husk;
`read_scanout_region` reads the husk.** F-2's conversion moves
`vk_image`/`vk_transfer` out of the `ScanoutBo` into the
`ScanoutAllocation` payload (`take_physical_backing`), leaving nulls.
`read_scanout_region` (`backend.rs` ~15833) reads `bo.vk_image` and
`bo.vk_transfer.staging_*` straight from the pool — so the decisive test
as written (composite → select → register → read) would fail with
"scanout staging buffer too small" or read a null image even once
selection succeeds, and the same holds for composition *into* a managed
bo through the legacy scene path. This is not a test bug; it is the
missing half of Task 5: **managed bos are read and written through the
payload under a lease, never through the pool struct.** That is 5.5's
"wire managed scene and read adapters" and 5.3's registration, i.e. F4-M3.

Fix (F-4c): `read_scanout_region_for_managed_source` obtains image and
staging from the `ScanoutAllocation`'s `SharedBacking` under a `Read`
reservation (`with_scanout_read` or equivalent on `ResourceService`,
mirroring F-3's `with_storage_read`), and the scene submission's
managed-route branch obtains the render target the same way under
`Write` with `prepare_retirement_batch` registering the GPU obligation
and `PendingAck` carrying the batch. The decisive test then registers
the bo **before** the composite tick and drives composition through
that branch. Under R8 the branch is never taken in production.

### Accepted

- **F4-B1** — `Option<Arc<VkContext>>` with `cfg(test)`-only `None`;
  `ticket_status` `expect`s `Some` after the test hook; eight tests
  deterministic again plus two real-fence `_vulkan` variants. 88
  deterministic; the ninth was the `Spy` predecessor — accepted.
- **F4-M1** — racy assertions removed.
- **F4-M2** — real source via `register_managed_scanout_bo`, real
  scratch via `into_managed`, source key derived from the selected bo's
  `managed_key()` with fail-closed `InvalidProof` when the bo is not
  managed. Unverified at runtime (F4b-B1); re-reviewed with F-4c.
- **F4-B2** — `TestDevice::open_real_drm_matching` +
  `Device::from_file_for_tests`, panic without a node. The identity
  matching is a genuine improvement to the fixture.

### Minor

**F4b-m1** — the fixture replaces the device on *every* `KmsDevice` in
`base.platform.devices` with the same `Rc`; fine for the single-output
fixture, but the loop should assert `devices.len() == 1` so a future
multi-device fixture does not silently share one fd.

## What F-4c must do

1. F4b-B1 / F4-M3 — read and write managed scanout bos through the
   payload under leases; `prepare_retirement_batch` at the managed-route
   branch of scene submission; `PendingAck` carries the batch;
   `cancel_pre_submit_batch`/`freeze_uncertain_batch` on the failure
   exits; `drain_pending_pool_releases` consults the service before
   returning a managed bo. All inert under Legacy (R8).
2. The decisive test: register the bo before the composite tick,
   composite through the managed branch, read with `PermissiveDump`,
   assert the phase and the pixels, then the source/scratch retention
   sequence. Paste it green.
3. F4-m1 (`#[allow(dead_code)]` on the adapter) and F4b-m1.
4. If the scene-submission branch does not fit with the rest in one
   session, F8-stop with the split: read path + test first, write path
   second.
