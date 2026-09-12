# Stage 2c-i fix round — session F-2 (Task 4) review

## Verdict

**Not yet accepted — 3 blocking, 3 major, 2 minor.** B-13 is genuinely
done: `take_physical_backing` moves every physical field out of
`ScanoutBo`/`CopiedRenderSource` into real payloads, the husk's `Drop` is
inert, `acquire_managed_scanout_bo` owns the `BoPhase` transition, and the
real-GBM `_drm` test exists and runs on this box. M-22 and the `mock`
half of M-23 are closed; the `Option<Arc<VkContext>>` half was correctly
stopped-and-reported under F8 (ruling below). What is not right is the
lifetime of the converted pool buffer and the normal-path discharge that
F-2 added around it: a registered managed bo has no root and is
RMFB'd on the next service tick; a normally-released payload leaves a
stale alias in the registry inventory; and a failed discharge destroys
the right it claims to retain. All three were confirmed here by probe or
by construction.

Reviewed `89cf771f..08cb7b91` (`fea5c043` code, `08cb7b91` fold-back,
`e8781b25` re-baseline note) by the coordinating session
(`claude-opus-5`), inline, with probes. Gate re-run here: clippy clean,
`c0_2ci` 81/81 on twelve runs, musl and freebsd check,
`c0_2ci_fd_family_barrier_real_gbm_payload_drm` and
`c0_2ci_scanout_managed_conversion_and_bophase_ownership_vulkan` pass.

## Findings

### Blocking

**F2-B1 — a converted pool buffer has no root; the next service tick
discharges it.** `platform.rs` `register_managed_scanout_bo` adopts the
`ScanoutAllocation`, takes `display_lease.key()` and **drops the lease**
(`drop(display_lease)`). The entry now has zero live uses and zero
obligations, so `can_destroy` is true and it is already on the dirty list
(lease `Drop` marks it). The first `service_ready_with_registry` after
registration discharges `file_owned` — `RMFB` on the framebuffer, GEM
close (or gbm_bo drop) — and removes the entry, while the pool slot still
lists the husk with `managed_key = Some(key)` and may be scanning the
buffer out. Sonnet's own
`c0_2ci_scanout_service_ready_with_registry_discharges_before_destroy`
demonstrates the mechanism (adopt, drop lease, tick → `RemoveFb`,
`CloseGem`, entry gone). The adapter test does not see it because it calls
`acquire_managed_scanout_bo` before any tick.

Fix: the pool slot owns the `Retain` lease — `ScanoutBo { managed:
Option<AllocationLease> }` (or on the pool entry) instead of a bare key;
`managed_key()` derives from it; `detach_managed_entries` and
`drain_scanout_pool_at` drop it (that is when the entry becomes
destroyable). Test: register, tick `service_ready_with_registry`, assert
the entry is still present and `calls` is empty; then detach, tick, assert
`RemoveFb` + the right GEM disposition.

**F2-B2 — normal-path discharge never unregisters the payload alias.**
`service_ready_with_registry` (`mod.rs`) discharges and removes the entry
but never calls `registry.unregister_payload_alias(key)`;
`unregister_payload_alias` has no non-test caller. Probe run here: adding
`assert_eq!(registry.payload_aliases(), 0)` at the end of
`c0_2ci_scanout_service_ready_with_registry_discharges_before_destroy`
fails with `left: 1`. Consequence: every payload released normally during
the incarnation's life leaves a key in `payload_alias_keys`; at teardown
the barrier walk hands that key to the discharge callback, whose only
implementations look the entry up and `expect("entry present")` — a
panic, or an `Err` that makes the barrier unmintable, in the *common*
case (buffers were released before teardown).

Fix: unregister on successful discharge in `service_ready_with_registry`;
assert `payload_aliases() == 0` in that test; and add a test that
releases one payload normally, then mints the barrier with a second
payload outstanding — the callback must be invoked exactly once, for the
second key only.

**F2-B3 — a failed discharge destroys the right it claims to retain
(F6).** `ScanoutAllocation::discharge_file_owned` (`resources/scanout.rs`)
on `Err` does `self.file_owned = Some(returned); return Err((err,
self.file_owned.take().unwrap()))` — it reinstalls the backing and
immediately takes it out again into the `Err`. `service_ready_with_registry`'s
`Err(_) =>` arm then drops that `FileOwnedBacking`: the right (at
`FramebufferRemoved`), the gbm_bo and the device alias are gone; the
payload's `file_owned` is `None`; the next tick finds nothing to discharge
and destroys the entry. Net: for `GemOwner::Right` the GEM handle is never
closed; for `Gbm` the gbm_bo drop ran with nothing ordering it. The comment
in that arm ("The right kept its `FramebufferRemoved` retry state inside
the still-rooted payload") describes the opposite of what the code does.

Fix: `discharge_file_owned` keeps the backing in `self.file_owned` on
failure and returns only the `io::Error` (as
`DirectFramebufferAllocation::discharge_file_owned` already does); the
`_drm` test's `.map_err(|(err, _)| err)` goes away. Test: `fail_gem` on
the mock, tick, assert `file_owned().is_some()` with the right at
`FramebufferRemoved` and the entry still present; clear, tick, assert one
more `CloseGem` and the entry gone.

### Major

**F2-M1 — the leak paths F-2 fixed still exist beside the fixes.** Plain
`adopt` still accepts a `Scanout` payload with `file_owned: Some` and
registers nothing (the `_drm` test itself uses it); plain `service_ready`
still drops such an entry undischarged; and none of the three new checks
(`adopt_with_registry`'s match, `service_ready_with_registry`'s match,
`apply_teardown_release`'s M-10 check) covers
`AllocationPayload::DirectFramebuffer`, which also carries a right and an
`Rc<Device>` and has its own `discharge_file_owned`. "Additive" leaves the
old hole open. Fix: `adopt` returns `Err(InvalidState)` for any payload
whose file-owned half is `Some` (forcing `adopt_with_registry`);
`service_ready` re-dirties instead of destroying an entry whose payload
still has a file-owned half; all three matches cover both payload kinds
through one `payload.file_owned_alias_present()` helper.

**F2-M2 — `register_managed_scanout_bo`'s failure path discards extracted
ownership and orphans the renderer half.** After `take_physical_backing`,
a failed `adopt` (`Exhausted`, `Frozen`) is handled by
`.map_err(|(e, _)| e)?`, which drops the returned `ScanoutAllocation`: FB
never removed, image/memory leaked, gbm_bo closed out of order, right
lost. And the renderer source is adopted *before* the display half, so a
display failure leaves a `CopiedSourceAllocation` in the service with a
husk in the pool and no key on it — precisely the orphan the pre-check
comment says it prevents. Fix: verify admission (`service.can_admit()` or
equivalent) before extracting; adopt display first, renderer second, and
on renderer failure release the display adoption; on any failure after
extraction restore the backing into the bo (`restore_physical_backing`,
the inverse of `take`).

**F2-M3 — the `_drm` test repeats F1-M1: a device-less registry cannot
show that the registry performs the last close.** `tests.rs`
`c0_2ci_fd_family_barrier_real_gbm_payload_drm` builds the registry with
`new_with_io` and asserts `weak.upgrade().is_none()` after the mint under
the comment "The registry performed the description's last close" — but
the payload's `drop(device)` inside `discharge` was the last close; the
mint's `self.device = None` is a no-op on `None`. The justification in the
test ("the gbm-before-device drop order is load-bearing") is inverted:
with the registry holding its own alias, the gbm_bo's `GEM_CLOSE` is
guaranteed to run against an open description *and* step 3 becomes
observable. Fix exactly as F1-M1: `new_with_device_and_io`, assert
`weak.upgrade().is_some()` inside the closure after
`discharge_file_owned`, `is_none()` after the mint. Delete the
justification comment.

### Minor

**F2-m1 — the husk keeps a counted alias.** `take_physical_backing`
clones `self.drm` because `ScanoutBo.drm` is not `Option`; the husk in the
pool therefore still holds an `Rc<Device>` — a non-payload alias the
barrier's `non_payload_aliases` must account for, or the pool must be
drained before the barrier is attempted. Record which in Task 9's text
(F-8); F-8's 9.5 deterministic test should include a pool husk.

**F2-m2 — `SharedBacking.vk` / `render_vk` / `sink_vk` stay
`Option<Arc<VkContext>>`.** Stopped-and-reported correctly under F8.
**Ruling:** accepted as is. F5 is amended: an `Option<Arc<VkContext>>` is
permitted when the only constructor that produces `None` is
`#[cfg(test)]` and every non-test constructor takes `Arc<VkContext>` by
value — which is now the case for all three. The `Drop`s' `if let Some`
guards are then unreachable in production. No further action.

## What was verified and holds

- B-13: extraction is complete for both types; `ScanoutBo::Drop` on a
  husk issues nothing (fb/gem `None`, Vulkan handles null, transfer
  guarded); `CopiedRenderSource::Drop` gained the transfer guard the husk
  needs; `BoPhase` transitions to `Recording` on managed acquire and legacy
  `acquire_scanout_bo` then refuses the slot (asserted in the `_vulkan`
  test).
- M-22: explicit `drop(gbm_bo); drop(device);`;
  `CopiedSourceAllocation::Drop` destroys transfer resources.
- B-12: `GemOwner::Gbm` now exists with a real gbm_bo and the transport
  shows exactly `[RemoveFb]`; the four vacuous tests were replaced with
  ones that call the function they are named after.
- F1b-m1: one-condition-at-a-time loop present; Sonnet reports the
  `submitters_detached` mutant is now caught.
- Fold-back: honest, per-finding table, the M-23 stop correctly reported.
  The re-baseline addendum (`e8781b25`) is accurate.

## What F-2b must do

1. F2-B1 — pool slot owns the lease.
2. F2-B2 — unregister on normal discharge; two-payload barrier test.
3. F2-B3 — `discharge_file_owned` keeps the backing on failure.
4. F2-M1 — close the plain `adopt`/`service_ready` paths; cover
   `DirectFramebuffer`.
5. F2-M2 — admission before extraction; ordered adoption with rollback.
6. F2-M3 — the `_drm` test over `new_with_device_and_io`.
7. Fold back; note F2-m1 for F-8 in Task 9's text now, so it is not lost.
