## Verdict

**2 blocking, 0 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result. No builds or tests were run, and implementation is not approved.

## Incorporation audit

| Prior findings | Status |
|---|---|
| None; this is the first review. | Check 1 skipped as requested. |

## Findings

### Blocking

**B-1 — The required XTEST valuator mode contradicts authoritative C3**

The [plan, lines 12–16 and 49–58](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/plans/2026-10-04-xi-xtest-grabs-and-query-classes.md:12) explicitly requires relative valuators and a wire test asserting relative mode. [Addendum C3, lines 138–140](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md:138) explicitly requires two **absolute** valuators.

Xorg supports the plan’s choice: `xorg-server-21.1.24:dix/devices.c:687–690` calls `InitPointerDeviceStruct`, whose initialization at lines 1644–1646 selects `Relative`.

Executing Task 2 therefore produces a `XIQueryDevice(4)` reply that violates the authoritative addendum while passing the plan’s prescribed assertion. Task 3 propagates the same discrepancy into initial master classes and classes copied from XTEST.

**Smallest correction:** explicitly resolve this authority conflict before execution and align the descriptor and expected wire mode. The Xorg-compatible resolution is to correct C3 to relative; the current plan cannot satisfy both stated authorities.

**B-2 — Unconditional reattachment on grab replacement breaks XI2 re-grabs**

The [plan, lines 93–98](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/plans/2026-10-04-xi-xtest-grabs-and-query-classes.md:93) says grab replacement reattaches XTEST slaves without distinguishing a replacement that continues an XI2 grab.

Xorg preserves floating attachment during same-device XI2 replacement:

- `xorg-server-21.1.24:dix/events.c:1621–1624` calls `DetachFromMaster` during activation.
- Lines 1463–1464 return immediately when the device is already floating.
- Lines 1649–1650 free the previous grab without reattaching the device.

This matches the checkout’s existing same-client replacement path: it overwrites the grab and calls detach, without first reattaching ([dispatcher, lines 19069–19089](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/core_loop/process_request.rs:19069)).

Concrete failure: grab pointer 4, move its floating position away from master 2, then replace the grab with another XI2 grab owned by the same client. Following the plan literally either leaves 4 attached during the replacement grab or reattaches and detaches it again. The latter deletes its floating position during reattachment and initializes the next floating position from `pointer_root` ([server, lines 1623–1629 and 1652](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/server.rs:1623)). Its independent position is lost, contrary to Xorg and [B’s floating-position contract, lines 116–121](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md:116).

**Smallest correction:** state that same-device XI2 replacement preserves floating attachment, saved master, and floating state. Qualify reattachment to replacement paths that actually deactivate the floating grab in Xorg.

### Major

None established.

### Minor

None established.

## Coverage and implementation checks

1. **Incorporation:** skipped because there is no prior review.
2. **Architecture and cross-task contracts:** assessed all five task descriptions against B, C1–C5, D, and relevant main-design lifecycle/query requirements. Verified dispatcher injection, registry-based identity resolution, slave-switch bookkeeping, query descriptor selection, and explicit grab replacement boundaries.
3. **Safety, ownership and failure semantics:** assessed stored class identity across removal, floating attachment ownership, position lifetime, and validation-before-injection ordering. B-2 identifies a concrete state-loss sequence. Backend floating-XKB enrollment and retirement for virtual keyboard 5 remain unassessed.
4. **Specification and verification strategy:** compared the proposed assertions with the normative requirements and inspected Xorg validation, initialization, query-state serialization, and grab attachment behavior. B-1’s assertion validates the plan’s chosen mode while contradicting C3. The imported task rules assign formatting, library/integration tests, mutation checks, and exactly `cargo clippy --all-targets -- -D warnings` to implementation.

**Excerpts used: 24/24:** three specification excerpts, one imported task-rules excerpt, ten checkout-source excerpts, and ten tagged Xorg excerpts. Investigation stopped at the budget.

**Follow-ups (not defects):** the specific unresolved question is whether existing `sync_floating_keyboard_states` enrolls virtual keyboard 5 after detachment and retires its independent state through ungrab/disconnect without affecting master XKB state. Those paths were not verified and are not declared sound.

Rust signatures, borrowing, fixtures, actual test results, and build/portability results are deferred to implementation.