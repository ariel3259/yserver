# Stage 2c-i fix round — session F-5b (Task 6, sinks half) review

## Verdict — ACCEPTED; gamma's four-way test carried as a hardware case

Reviewed `22a4d885..4eb05029` (`842745a3` code, `4eb05029` fold-back;
the session was cut once by the API limit and resumed). B-10/R11: seven
sinks carry the gate check **inside the function that issues the ioctl**
(`page_flip.rs` `submit_flip_with_fences`; `modeset.rs`
`submit_direct_scanout`, `submit_composed_scanout`, `commit_modeset`,
`disable_output`; the four cursor methods in `platform.rs`; the executor
`send_authorized`/`dispatch_blocking_at_boundary_authorized` which is the
only sink that consumes an `OwnerWriteGrant`, at the serialized send
when the executor accepts — R7). Every production caller derives the
`legacy_write_permitted` argument from `PlatformBackend::allows_legacy`
except the three construction-time rollbacks (`PlatformBackend::drop`,
`InitialScanoutRollbackGuard`, `activate_initial_scanout_outputs`), which
pass `true` and are classified in the inventory as running before any
gate can exist (R8). Test-only modeset and the vblank arm are
observational; FB removal/GEM close are Task-2 rights. The R11 inventory
table is in the fold-back. F5a-M1: `DirectOwnershipSignal` deleted;
`ScanoutM2OwnershipHandle`, synced at every real mutation site in
`backend.rs`, implements `DirectOwnershipState`. The executor's public
2-arg `send`/`dispatch_blocking_at_boundary` were kept byte-for-byte for
the 28 external call sites; the gate-carrying variants are `pub(crate)`.

Mutation checks here: (A) `submit_flip_with_fences` gate removed →
`c0_2ci_sink_legacy_page_flip_gate_four_states` fails; (B)
`submit_direct_scanout` gate removed →
`c0_2ci_sink_direct_atomic_flip_gate_four_states` fails. Sonnet's three
(disable_output guard, `authorize_write` at send, non-consuming
`consume_owner_write`) are consistent. Gate: clippy clean, `c0_2ci`
101/101 on twelve runs, hardware 10/10, **full `cargo test -p yserver`
green including the integration tests**, musl and freebsd check (per
fold-back).

## Carried

**F5b-m1 — gamma's four-way test.** The guard is in
`apply_gamma_to_live_output`, but `live_crtc_and_gamma_size` issues a real
`GETCRTC` first, which `Device::for_tests()`'s socket cannot answer, so no
deterministic test reaches the check. Sonnet correctly declined to take
DRM master unattended. `GETCRTC` does not need master: a `_drm` test over
the real primary node opened without master (the F-4b fixture pattern,
`TestDevice::open_real_drm_matching` + `Device::from_file_for_tests`)
with the counting transport beneath `set_gamma` can drive the four states.
Goes to **F-9** (Task 10, hardware matrix). Plan step 6.5a stays
unticked for the gamma row only.

**F5b-m2** — the two rollbacks that have `self` in scope
(`PlatformBackend::drop`, the guard) could pass `self.allows_legacy(..)`
instead of `true` at no cost; only `activate_initial_scanout_outputs`
genuinely has no backend yet. Cosmetic; fold into F-9 with F5b-m1.

F-6 may start.
