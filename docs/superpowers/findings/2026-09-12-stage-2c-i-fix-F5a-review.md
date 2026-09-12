# Stage 2c-i fix round — session F-5a (Task 6, resources half) review

## Verdict — ACCEPTED, one item carried into F-5b

Reviewed `6b2225ee..265b228e` (`5f025fed` code, `265b228e` fold-back).
B-11 is closed the right way: `CoreRetirementBatch.serviced_deadline`
stamped at `register_batch` from `serviced_elapsed.checked_add(max)`
(overflow quarantines that batch), `service_completions` expires only
batches past their own deadline and never sets `exhausted`; the seat
clock is driven from the real `on_vt_release`/`on_vt_acquire`/DPMS seams
(inert without a service, R8). M-16: `next_deadline` no longer hides the
poll deadline while inactive; 6.1 runs on the real core-loop fixture
(`c0_2ci_progress_no_composition_on_core_loop_fake_backend`). M-14:
`close()` refuses with outstanding grants (`force_close` kept for the
foreign-proof path), `issue_handover_permit` validates `LegacyDrained` +
dispositions, `try_finish_legacy_transport` consults
`legacy_transport_gate_permits_finish`. B-6: `RecipientReservation::new_for_tests`
and the whole `RetainingSupervisor` fixture are `#[cfg(test)]`.

Mutation checks here: (A) deadline made global again → both
`c0_2ci_serviced_deadline_*` tests fail; (B) `close()` ignoring grants →
`c0_2ci_transport_gate_close_refuses_outstanding_grants` fails. Gate:
clippy clean, `c0_2ci` 93/93 on twelve runs, hardware 10/10, musl and
freebsd check (per Sonnet; the sessions' reports have been accurate).

## Carried into F-5b

**F5a-M1 — M-13 is only half closed: `begin_quiescing` reads a trait
object, but the production impl is a `Cell` nobody sets.**
`transport.rs`: `DirectOwnershipSignal` implements `DirectOwnershipState`
over two `Rc<Cell<bool>>` with `set_direct_ownership_busy` /
`set_unflip_outstanding` — and `grep` finds no production caller of
either. So `Busy` is still a free-floating boolean, one layer down; in
production it would always read "not busy". The trait is the right seam;
the impl must be over the **real** ownership-unit state — the struct
that tracks direct scanout `Current`/`Submitted`/`Successor` and the
unflip requested/retired state in `platform.rs`/`backend.rs` (Task 8's
`DirectCapacity` roles, or the existing direct-present state, whichever
is the source of truth today). F-5b, which touches the sinks anyway,
implements it and deletes `DirectOwnershipSignal`. The
`FakeDirectOwnershipState` test double stays.

**F5a-m1** — `service_completions` calls plain `service_ready`, so a
file-owned scanout entry is re-dirtied every tick until someone calls
`service_ready_with_registry`; the backend's service tick will need the
registry (F-4d/F-8). Note only.

F-5b may start.
