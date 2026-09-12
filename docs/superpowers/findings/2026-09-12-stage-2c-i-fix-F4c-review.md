# Stage 2c-i fix round — session F-4c (Task 5) review

## Verdict — ACCEPTED, with the write half scheduled as F-4d

Reviewed `1f468950..28d1fcff` (`4e870930` code, `28d1fcff` fold-back).
The read half of F4b-B1 is real: `ResourceService::with_scanout_read` /
`with_scanout_write` (lease-scoped, reserving `Read`/`Write` on top of the
caller's lease, mirroring the storage accessors); `read_scanout_region`
split into shared helpers (`scanout_copy_needed_bytes`,
`submit_scanout_copy_to_staging`) so the legacy and managed paths cannot
diverge; `read_managed_scanout_region_bytes` takes image and staging from
the `ScanoutAllocation` payload under a `Read` reservation. The decisive
test `c0_2ci_read_source_scratch_regression_vulkan` registers the bo as
managed before the composite tick, reads with `PermissiveDump` per the
F-4b ruling, asserts the phase and the pixels, then the source/scratch
retention sequence — **green on this box**.

Mutation check here: reading `vk::Image::null()` instead of
`shared.image` → the test process dies with SIGSEGV in the driver copy
(the read genuinely uses the payload's image, not the husk). Sonnet's
own two mutations (null/zero fields → "staging buffer too small";
`record_read_outcome` no-op → "source_read_pending must be 0") are
consistent with that. Gate: clippy clean, `c0_2ci` 98/98 with
`--include-ignored` (88 deterministic + 10 hardware).

## F8 stop — write half (5.3/5.5), ruling

Sonnet traced the seam precisely: `submit_shared_scanout_frame` in
`scene.rs` (from `tick_one_output`'s `Shared` arm) reads `bo.vk_image`
directly for the composite render; a managed branch needs a render-target
wrapper spanning the pool `ScanoutBo` (semaphore, size, timestamps) and
the payload `SharedBacking` (image, view, CB, timestamp pool), and — the
real cost — no `ResourceService` reaches the compositor tick today, so it
must be threaded through `tick`/`maybe_composite`/`tick_one_output`/
`submit_shared_scanout_frame`/`drain_pending_pool_releases`, with every
`PendingAck` retirement site resolving/freezing a `CoreRetirementBatch`.
Three sessions have now independently identified this same piece.

**Ruling:** it is its own session, **F-4d**, scheduled after F-9 and
before F-10 (M-19) — it is hot-path conversion work like M-19, nothing
in F-5..F-9 depends on it (they live in `resources/`), and under the
subscription deadline the mechanism sessions come first. Plan steps 5.3
and 5.5 stay unticked until F-4d; 5.1 is ticked (test green).

## Residuals carried

- F4-m1: the two `#[allow(dead_code)]` on the adapters stay until F-4d
  gives them a non-test caller path (verified: removing them fails
  clippy today).
- The two upstream `#[ignore]`d fixture tests
  (`root_get_image_reads_scanout_pixels_not_root_storage`,
  `root_overlay_xor_pass_reaches_scanout`) now fail honestly at
  `OnScreenOnly` selection; never live before; for Jos.

F-5 may start.
