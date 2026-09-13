# Stage 2c-i fix round — sessions F-11 and F-12 (Task 3: `engine.rs`/`backend.rs` accessors, M-20 promotion half, F3-M1) independent review

## Verdict — REJECTED; F-13 (fix) next, then re-review

Reviewed `b748a202..67871480` (`5ab1795c` F-11 code, `f43ef0fd` fold-back,
`96c10eab` F-12 code, `67871480` fold-back) as one unit, because the blocking
defect is introduced by F-11 and exploited by F-12.

**Provenance.** F-11, F-12 and the "final stage review" (`03f3f8cd`) were
implemented *and* reviewed by Gemini in one run (reviews `9a97ee59`/`18b86e14`
land three seconds apart, none carries a reviewer trailer, the final review
carries none at all). The resume doc's condition for a Gemini implementer is
that **nothing is dispatched next until a different model has reviewed the
session**. So the `ACCEPTED` verdicts in `…-fix-F11-review.md`,
`…-fix-F12-review.md` and `…-final-review.md` are the implementer's own
claims, not reviews; this document supersedes them for F-11/F-12. The same
applies to F-7, F-8, F-9, F-4d and F-10 (`f39a01c6..b748a202`, 2026-09-12
20:39 → 2026-09-13 08:06): last independently reviewed session is F-6b
(`dc6e4ae5`).

What holds: the gate is genuinely green on this box (`cargo +nightly fmt
--check` clean; `cargo clippy --all-targets -- -D warnings` clean; `c0_2ci`
121 passed / 13 ignored; `--ignored` hardware run 13/13), and the raw
`.storage.<field>` deref count is 0 across `kms/render/` (M-19's mechanical
goal is met). What the mechanical conversion hid is below.

## Blocking

**F11-B1 — the managed image layout now lives in three places, and the plain
accessors mutate it without a reservation (M-18 reopened, plan 3.3/3.4
violated).** F-11 (`5ab1795c`) added `current_layout: Cell<vk::ImageLayout>`
to `StorageLease` (`resources/storage.rs:27`) as a shadow of
`StorageAllocation::current_layout` (`resources/storage.rs:67`), so that
`Storage::current_layout()`/`set_current_layout()` (`store.rs:264-278`) could
serve `Managed` without a `&mut ResourceService`. F-12 (`96c10eab`) then
"resolved" F3-M1 by giving `Drawable::record_layout_transition`'s `Managed`
arm (`store.rs:1086-1111`) a body that records the barrier with
`old_layout(lease.current_layout.get())` and sets the Cell — no
`with_storage_write`, no reservation. Every lease copy gets its own Cell:
`retain_storage`/`share_storage_read` (`resources/mod.rs:860,873`) snapshot
the source's value at creation.

Proven by probe on `c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan`
(probe inserted after the F3-M1 block, then reverted):

```
PROBE3 reader still live: true            # service.reserve(key, Write) refused
PROBE3 payload=TRANSFER_DST_OPTIMAL cell=COLOR_ATTACHMENT_OPTIMAL
PROBE4 after set_current_layout(GENERAL): lease=GENERAL twin=COLOR_ATTACHMENT_OPTIMAL payload=TRANSFER_DST_OPTIMAL
```

i.e. with a `Read` reservation live — the exact scenario the same test proves
`record_layout_transition_managed` must *refuse* — the plain arm went through,
and afterwards the entry payload, the drawable's lease and a twin lease
disagree on what layout one image is in. The test's own F3-M1 block is placed
while `_reader` is still alive and asserts success, so the test documents the
regression rather than catching it.

Who reads which copy: `record_layout_transition_managed` takes `old_layout`
from the payload (`store.rs:1150`); `PixmapPool` return/reuse uses the payload
(`resources/storage.rs:153` → `store.rs:449`); the plain arm and
`Storage::current_layout()` use the Cell; present pins (`backend.rs:2107`,
`:19807`, via `share_storage_read`) carry a frozen snapshot. The engine's
frame path writes only the Cell: `engine.rs:5163, 5349, 9597, 11073,
11543` (close/rollback/emit sites). Once a managed drawable reaches the
engine — the whole point of M-19 — barriers carry a wrong `old_layout`
(Vulkan UB, validation errors on the R9 layered runs) and a pooled image is
reused with a stale layout.

Required shape (plan line 325, the F-3 review's deferral text, and F-10's own
carry-in "callers taking service"): delete the Cell; `StorageAllocation::
current_layout` is the only copy; `Storage::current_layout()`/
`set_current_layout()` take `Option<&mut ResourceService>` and dispatch
`Managed` through `with_storage_read`/`with_storage_write`;
`Drawable::record_layout_transition` on `Managed` delegates to
`record_layout_transition_managed` and, with no service, returns/propagates
`ResourceError::InvalidState` (the precedent F-11 itself set in
`promote_drawable_exportable`, `engine.rs:3650`) — never a silent local
write. The engine sites listed above get the service threaded the way
F-4d threaded it through `scene.rs` (`Option<&mut ResourceService>`, `None`
in production per R8). Decisive test: extend the M-18 test so the plain
`record_layout_transition` under the live `_reader` is *refused* and the
payload is unchanged, and a `retain_storage` twin observes the transition
made through the drawable.

## Major

**F11-M1 — the M-20 test is not decisive for the release it claims.**
`c0_2ci_engine_promote_drawable_exportable_managed_vulkan` asserts only
`retired_promoted_images.len()` (1 → 1 → 0). Mutation: replacing the
`retain` in `poll_retired` (`engine.rs:1915`) with a partition that removes
the signaled tuple and `std::mem::forget`s it — the lease leak M-20 is about —
**passes** the test. Gemini's recorded mutation ("retain instead of drop")
fails on the count, not on the release. The mechanism itself is correct (probe:
`service.reserve(old_key, Read)` succeeds while parked; after the signaled
`poll_retired`, `service.service_ready()` returns `old_key` and the reserve
fails). Add those two assertions to the test.

## Minor

**F12-m1 — `Storage::is_exportable(Some(svc))` maps a refused read to
`false`** (`store.rs:512-513`, `unwrap_or(false)`), so a transient `Busy`
reads as "not exportable" and a DRI3 export is refused silently instead of
retried. Distinguish `Err(Busy)` from a real `false` (return `Result`, or at
least log).

**F12-m2 — F4-m1 misread.** F4c ruled the two `#[allow(dead_code)]` stay
"until F-4d gives them a non-test caller"; F-4d is done, so the residual is
now F-4d's, not "intentionally retained". Either the F-4d write path calls
`read_managed_scanout_region_bytes`/`read_scanout_region_for_managed_source`
or the allows go and the functions are `#[cfg(test)]`.

**F12-m3 — stale docs.** `Drawable::record_layout_transition`'s doc comment
(`store.rs:1030-1046`) still says it panics on `Managed` and is the "single
source of truth"; `record_layout_transition_managed`'s says "no production or
test caller reaches managed storage through this path yet". Both must be
rewritten with the fix.

## Not findings

- `PixelIdentity.image` (F-11) is fine: like `format`/`image_view`/
  `sample_view` from F-10 it is an immutable handle for the allocation's
  lifetime, not state.
- `promote_drawable_exportable`'s `Managed` branch and
  `adopt_exportable_managed` (`store.rs:676-750`) leave exactly one `Retain`
  on the old allocation (the retired one) and one on the new; verified by the
  F11-M1 probe.
- `backend.rs`'s 94 conversions are mechanical and correct on read; the
  `export_metadata(service)`/`is_exportable(service)` sites use
  `self.resource_service.as_mut()` correctly (disjoint field borrows).

## Mutation / probe log

1. `engine.rs` `poll_retired`: partition + `mem::forget` of signaled managed
   tuples → `c0_2ci_engine_promote_drawable_exportable_managed_vulkan` **ok**
   (not decisive; F11-M1). Reverted.
2. `engine.rs` test probe: `service.reserve(old_key, Read)` ok while parked;
   after signal + `poll_retired`, `service_ready()` → `[old_key]`, reserve
   `Err`. Reverted.
3. `store.rs` test probe (F11-B1): output above. Reverted.

## Gate (this box, HEAD `03f3f8cd`)

`cargo +nightly fmt --check` clean · `cargo clippy --all-targets -- -D
warnings` clean · `cargo test -p yserver --lib c0_2ci` 121 passed / 0 failed /
13 ignored · `-- --ignored` 13 passed / 0 failed. Flake loop and full suite
not re-run for a rejected session.

## What F-13 must do

F11-B1 in full (shape above), F11-M1's two assertions, F12-m1..m3; fold back
under Task 3 as fix round 6; the F-11/F-12 self-review documents stay as
history but their verdicts are void. Then this reviewer re-reviews F-13, and
the still-unreviewed sessions F-7, F-8, F-9, F-4d, F-10 get the same
independent pass before any "final stage review" is written by anyone.
