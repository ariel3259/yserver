# Stage 2c-i fix round — session F-13a (Task 3: single-sourced managed layout) review

## Verdict — ACCEPTED; F-13b next

Reviewed `64900867..75edc865` (`4eb5a96b` code, `75edc865` fold-back).
Implementer: Sonnet. Reviewer: Opus (this document).

F11-B1 is closed the way the F11-F12 review asked: `StorageLease::current_layout`
is gone (`resources/storage.rs`), `StorageAllocation::current_layout` is the only
copy, `Storage::current_layout(Option<&mut ResourceService>) -> Result<_, ResourceError>`
/ `set_current_layout(layout, Option<…>) -> Result<(), _>` dispatch `Managed`
through `with_storage_read`/`with_storage_write`, `None` on a Managed path is
`Err(InvalidState)`, `Busy` propagates, Legacy is byte-for-byte the old field
access. `Drawable::record_layout_transition` on `Managed` delegates to
`record_layout_transition_managed` (the Cell-writing arm is deleted, the Managed
match arm is `unreachable!` behind the early dispatch). Service threaded through
`current_layout_for_drawable` and its callers, the emit/close/rollback sites and
the two backend test helpers; production passes `None` at 24 sites (R8); every
`unwrap()` the threading added is inside `mod tests`.

Decisive test (`c0_2ci_storage_record_layout_transition_managed_reserves_write_vulkan`)
now proves, under the same live `_reader`: the plain arm is `Err(Busy)` and the
payload is unchanged; with no service it is `Err(InvalidState)`; after the reader
drops, a `retain_storage` twin observes the transition through `with_storage_read`
(single source of truth); the accessor surface `current_layout(None)` is
`InvalidState`. F11-M1: the M-20 test now asserts reservability while parked and
`service_ready()`/reserve-failure after the drop.

## Mutation checks (this reviewer)

1. `is_exportable` Managed + no service → `true`: `c0_2ci_storage_is_exportable_managed_reserves_read_and_refuses_when_written` fails. ✔
2. Sonnet's two pasted mutations (plain arm returning `Ok(())` without delegating → `left: Ok(()) right: Err(Busy)`; `poll_retired` `mem::forget` → `reserve(old_key).is_err()` fails) match the mechanisms; not re-run here beyond reading the test bodies.
3. **`Storage::set_current_layout` Managed arm replaced by a no-op `Ok(())`: 134/134 still pass.** The write accessor the engine's close/rollback path uses is not covered by any assertion. Minor, see F13a-m1.

## Gate (this box)

`cargo +nightly fmt --check` clean · `cargo clippy --all-targets -- -D warnings`
clean · `cargo test -p yserver --lib c0_2ci -- --include-ignored` 134 passed / 0
failed · raw `.storage.<field>` derefs outside `store.rs`: 0. Sonnet's fold-back
records the 12-run flake loop (0 flakes) and full suite 1659/0/85.

## Residuals carried into F-13c

- **F13a-m1** — extend the M-18/F11-B1 test with `set_current_layout` under a
  live reader (`Err(Busy)`, payload unchanged) and a twin observing a
  `set_current_layout` made through the drawable; the no-op mutation above must
  then fail.
- F12-m1 was resolved by logging rather than `Result` (Sonnet's call, reasoned:
  ~180 bool call sites for a path nothing reaches under R8). Accepted; revisit
  when 2c-iii gives `is_exportable` a managed caller.

F-13b (Task 8 seam: F7-B1, F7-B2) may start.
