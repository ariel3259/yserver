# Stage 2c-i fix round — session F-3 (Task 3) review

## Verdict

**Not yet accepted — 1 blocking, 1 major, 1 minor — plus a scheduling
decision on M-19 recorded below.** The substance is right: B-14 is closed
with a real-context test (mutation-checked), M-18's porous seam is closed
by visibility (`store.rs` can no longer reach `entry.payload`), M-20's
store half detaches the retain use at `destroy()` (mutation-checked),
M-21's DRI3 regressions now go through a real dma-buf round trip, and
F2b-m1's rollback case exists. The one blocking item is procedural and
trivial — four new hardware tests report a missing ICD as a pass — but
R12 is explicit and the previous sessions' tests did it right.

Reviewed `d57ab4b1..3677e4b6` (`12c5926c` code, `3677e4b6` fold-back) by
the coordinating session (`claude-opus-5`), inline, with mutation checks.
Gate re-run here: clippy clean, `c0_2ci` 89/89 on twelve runs, all seven
hardware tests pass, musl and freebsd check.

## Findings

### Blocking

**F3-B1 (R12) — four new `_vulkan` tests report an environmental skip as
a pass.** `store.rs`: `c0_2ci_storage_no_premature_pool_return_vulkan`,
`c0_2ci_storage_into_managed_pins_real_context_for_cleanup_vulkan`,
`c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan`,
`c0_2ci_storage_dri3_lease_regressions_vulkan` all do
`Err(e) => { eprintln!("skip: no Vk: {e}"); return; }` on
`VkContext::new()` (five occurrences). Without an ICD they pass. R12: "must
report an environmental skip as a skip — never as a pass"; the stage's
existing `_vulkan`/`_drm` tests `panic!` in that arm. The repo's older
tests use the `eprintln`+`return` idiom, which is why it crept in, but it
is the shape this stage forbids. Fix: `panic!("environmental skip: no live
Vulkan ICD; not claiming pass")` in all five arms.

### Major

**F3-M1 — `Storage::is_exportable` and `Drawable::record_layout_transition`
now `panic!` on `Managed`.** The `_managed` variants route correctly
through `with_storage_read`/`with_storage_write` (that is M-18, closed),
but the plain accessors went from an unreserved borrow to a panic — the
M-19 pattern, one more instance. Unreachable today under R8 (no producer
of managed storage outside `into_managed`), so not a live defect, but it
must not survive the M-19 conversion: when the callers are converted the
plain accessors take the service (or the `_managed` variants become the
only ones). Recorded as part of M-19's scope; no separate fix now.

### Minor

**F3-m1 — `Storage::destroy` on `Managed` overwrites the backing with a
fabricated `Legacy` stub.** It works (the lease drops, the retain use
ends, mutation-checked), but a `Storage` that reports `Legacy` after
having been managed is a lie the next reader may act on. A
`StorageBacking::Detached` variant is the honest shape. Fold into the
M-19 sessions, which touch every reader anyway.

## M-19 — decision on the F8 stop

Sonnet recounted: **180** external `.storage.` sites (`backend.rs` 91,
`engine.rs` 70, `scene.rs` 17, `frame_builder.rs` 1, `target.rs` 1), and
proposed three sessions (read-mostly consumers → `engine.rs` →
`backend.rs`). The stop is correct and the split is sound.

**Decision (coordinating session, 2026-09-12, user unavailable for the
call — flagged for confirmation):** the three M-19 sessions, together
with M-20's promotion half (`retire_image_after`/`destroy_retired_image`)
and F3-M1/F3-m1, are **deferred to the end of the fix round as F-10,
F-11, F-12**, after F-9, not run now. Reasons:

1. Nothing in F-4..F-9 depends on it. The `Deref` panics only on
   `Managed` storage, which no production path creates (R8); the
   adapters F-4 adds use the `_managed` accessors directly.
2. The three files are the ones Jos changes most (`backend.rs` +796 and
   +260, `engine.rs` +2482 in the last two master merges). A mechanical
   180-site conversion sitting on the branch for six more sessions is
   the worst possible merge-conflict surface; done last, immediately
   before the squash, its window is smallest.
3. It keeps the mechanism work — the point of this stage — moving.

If the user prefers M-19 now, the order is simply F-3c/d/e before F-4;
nothing else changes. Plan step 3.3 stays unticked until F-12.

## What was verified and holds

- B-14: `into_managed` takes `&PlatformBackend`, refuses a non-stub
  allocation without a context, backfills `vk`/`pixmap_pool`. Mutants:
  backfill removed → `..._pins_real_context_for_cleanup_vulkan` fails;
  refusal removed → `..._refuses_non_stub_without_vk_context` fails.
- M-18: `AllocationLease::new`/`.entry` and `AllocationEntry.payload` are
  `pub(in crate::kms::render::resources)`; `store.rs` compiles only through
  the service.
- M-20 (store half): Managed `destroy()` arm no-op'd →
  `c0_2ci_storage_managed_destroy_detaches_before_drop` fails.
- M-21: real import/export round trip, `DRM_FORMAT_MOD_INVALID`,
  distinguishable client size, once-only FD, deferred retirement.
- F2b-m1: `force_exhausted_for_tests` + exhausted-second-bo case in the
  `_vulkan` adapter test.
- Fold-back: honest; M-19 and M-20's promotion half explicitly NOT
  RESOLVED / SPLIT with the reasons; 3.3 and 3.5 unticked.

## What F-3b must do

1. F3-B1 — the five `eprintln`+`return` arms become `panic!`. Nothing
   else. Fold back with the verdict line. Then F-4.

## F-3b re-review (`36ab74a7` + fold-back `5c10f7ee`) — ACCEPTED

The five arms are `panic!`s; `grep 'eprintln!("skip'` in `store.rs` is
zero; diff is 9/14 lines in one file. Gate here: clippy clean, `c0_2ci`
96/96 with `--include-ignored` (89 deterministic + 7 hardware). Nit,
not worth a cycle: the fifth arm (dma-buf export unsupported) now says
"no live Vulkan ICD" — the message could name the real condition. F3-M1,
F3-m1 and M-19/M-20-promotion remain scheduled for F-10..F-12.

F-4 may start.
