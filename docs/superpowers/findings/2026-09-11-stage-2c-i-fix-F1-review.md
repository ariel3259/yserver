# Stage 2c-i fix round — session F-1 (Task 2) review

## Verdict

**Not yet accepted — 1 blocking, 2 major, 2 minor.** The direction is
right and the mechanism is now the R5 shape (barrier discharges aliases
instead of waiting for them; inventory keyed by `AllocationKey`; rights
sealed; fake inventory under `cfg(test)`; `retire_closed_family` returns
`Err`). But the rewritten 2.5 proves step 2 of the barrier and not step 3,
and the production mint has no preconditions at all. Both are fixable in
a short F-1b session before F-2.

Reviewed `89e52434..53f7ca23` (`95f9dba6` code, `53f7ca23` fold-back) by
the coordinating session (`claude-opus-5`), inline, with a mutation check.
Gate re-run here: clippy clean, `c0_2ci` 78/78 on six runs, musl and
freebsd check.

## Findings

### Blocking

**F1-B1 — the production `try_mint_file_family_closed` has no
preconditions; it is an unconditional barrier (R9).**
`drm_cleanup.rs:370-383`: the only checks are inside `#[cfg(test)]` and
apply only when a `FakeFamilyInventory` is installed. In a non-test build
the function discharges every alias and mints on the first call. R5's
conditions (every submitter detached, helper reaped, control alias closed,
non-payload aliases zero) are not represented by any production state, so
there is nothing for Task 9 to set from real evidence, and a caller that
reaches this function today mints a `FileFamilyClosed` with no proof —
the exact fabrication R9 forbids ("a closed IPC fd or single closed alias
is never a … proof"; here not even that is required). That there is no
production caller yet (R8) does not make an unconditional proof
constructor acceptable; the previous review's B-4 was the same defect one
layer up.

Fix: a non-test `FamilyInventory { submitters_detached, helper_reaped,
control_closed, non_payload_aliases }` owned by the registry, **defaulting
to unsatisfied**, checked unconditionally before the discharge loop; the
`#[cfg(test)]` setters (`close_fake_control` etc.) mutate that struct, and
Task 9 (F-8) supplies the real setters driven by executor evidence. The
`c0_2ci_drm_cleanup_fake_family_barrier_requires_all_closed` assertions
then hold in production semantics, not only when a fake is installed.

### Major

**F1-M1 — the rewritten 2.5 cannot detect a registry that never performs
the last close (R5 step 3).** The test builds the registry with
`new_with_io` (`tests.rs:545`), i.e. **without a device**; the only
`Rc<Device>` holder is the payload. So `weak.upgrade().is_none()` after
the mint is satisfied by `discharge_file_owned` setting `alloc.device =
None` — the payload's drop is the description's last close, the reverse
of the plan's step 3 ("the registry closes its own last alias as the final
step"). Mutation check performed here: with `self.device = None` deleted
from `try_mint_file_family_closed`, the test still passes. The comment at
`tests.rs:604` ("The registry closed the description's last alias itself")
is not what is asserted.

Fix: build with `new_with_device_and_io(Rc::clone(&device), …)`; inside
the discharge closure, after `discharge_file_owned`, assert
`weak.upgrade().is_some()` (the registry still holds it — the payload's
close was *not* the last); after the mint assert `weak.upgrade().is_none()`.
That ordering assertion is the one that fails under the mutant. Also
install the fake inventory and satisfy its three conditions first, so the
test exercises the real precondition path once F1-B1 lands.

**F1-M2 — no test for a discharge failure mid-walk, and no test that the
discharge is not invoked while preconditions are unsatisfied.** The loop
at `drm_cleanup.rs:385-389` has the right shape (failed key stays
registered, right returned at `FramebufferRemoved`, family not closed,
retry resumes), but nothing exercises it. And
`c0_2ci_drm_cleanup_fake_family_barrier_requires_all_closed` never
registers a payload alias, so its panicking closure proves nothing about
ordering between preconditions and discharge.

Fix: (a) a test where `MockCleanupIo` fails the first `close_gem`, assert
`Err`, key still in the inventory, right at `FramebufferRemoved`, family
not closed; second mint issues only `CloseGem` and succeeds. (b) In the
existing all-closed test, register a payload alias up front and keep the
panicking closure through the three failing mints; switch to a counting
closure for the successful one and assert it ran exactly once.

### Minor

**F1-m1 — `consume` refuses after `freeze_incarnation`, which would block
the barrier discharge if quarantine ever freezes the registry.**
`drm_cleanup.rs:276-278`: a frozen registry marks the right `Frozen`
permanently. `freeze_incarnation` has no production caller today, so this
is latent, but F-8 must decide: either quarantine never freezes the
registry (only service entries), or the barrier discharge is the one
sanctioned issuer after freeze. Record the decision in Task 9's text when
F-8 is written; do not leave it to be discovered.

**F1-m2 — the discharge closure in the test reaches into
`service.entries` and `entry.payload.borrow_mut()` directly.** That is
M-18's porous seam. F-2 must add the service-side
`discharge_file_owned_for_barrier(&mut self, registry, key)` and the test
should switch to it; the fold-back's B-2 verdict already defers the
adoption half to F-2, so add this to that deferral explicitly.

## Fold-back audit

Honest and complete: per-finding verdict table, pre-fix evidence via the
deleted inverted assertion, API contract updated, the false Gemini claim
struck through with the correction beside it, deferrals to F-2 named. The
only gap is that the deleted-test argument shows the *old* test was wrong,
not that the *new* test is strong — F1-M1 is what that would have caught.

## What F-1b must do

1. F1-B1 — production `FamilyInventory`, fail-closed.
2. F1-M1 — 2.5 over `new_with_device_and_io` with the ordering assertion.
3. F1-M2 — the two missing tests.
4. Fold back with the same table shape. Then F-2.
