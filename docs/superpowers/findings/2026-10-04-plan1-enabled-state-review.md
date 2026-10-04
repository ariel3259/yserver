## Verdict

**1 blocking, 1 major, 0 minor**

Coverage: **INCOMPLETE**

Two transition contracts conflict with the required behavior. This is a design-review result; no builds or tests were run.

## Incorporation audit

| Prior findings | Status |
|---|---|
| None supplied | First review; check 1 skipped. |

## Findings

### Blocking

**B-1 — Client-disable commits the disabled state before releasing held input**

The [plan, lines 41–44 and 135–138](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/plans/2026-10-04-xi-enabled-state-and-floating.md:135) makes `enabled` the derived conjunction of session-enabled and not client-disabled, then instructs a property write of 0 to set client-disabled **before** invoking backend cleanup.

That order contradicts [addendum A3, lines 49–55](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md:49), which requires releasing holds before marking the facet disabled. Xorg follows that order: `ReleaseButtonsAndKeys` precedes `dev->enabled = FALSE` (`xorg-server-21.1.24:dix/devices.c:466–468,504–506`).

Concrete failure:

1. A physical pointer facet holds a button.
2. `Device Enabled = 0` sets client-disabled, making derived `enabled` false.
3. Cleanup generates its button release through [`Backend::on_host_input`, backend.rs:21180–21190](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver/src/kms/render/backend.rs:21180).
4. That entry point rejects the origin before processing ([backend.rs:21493–21515](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver/src/kms/render/backend.rs:21493)), because [`pointer_fanout.rs:75–83`](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/core_loop/pointer_fanout.rs:75) rejects a disabled facet.

The required client-visible release is discarded. Task 4’s held-button test demands behavior its instructions prevent.

**Smallest correction:** have the lifecycle owner drain holds while the old effective enabled state remains valid, then commit client-disabled and the remaining A3 steps.

### Major

**M-1 — Task 1 removes the attached disable descriptor that Task 2 requires**

[Task 1, lines 57–60](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/plans/2026-10-04-xi-enabled-state-and-floating.md:57) says every non-enabled facet reports `attached_master = None`, explicitly including hierarchy data. [Task 2, lines 91–95](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/plans/2026-10-04-xi-enabled-state-and-floating.md:91) instead requires marking it disabled and emitting an **attached** `XIDeviceDisabled` descriptor before floating it.

Following Task 1’s hierarchy rule produces `use = 5, attachment = 0` once enabled becomes false, including during Task 2’s disable event. The existing emitter obtains descriptors from current registry attachment ([hotplug.rs:278–284,342–359](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/xinput/hotplug.rs:278)); the plan supplies no exception or snapshot contract reconciling these instructions.

[Addendum A3, lines 53–55](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/docs/superpowers/specs/2026-10-04-xi-registry-maintainer-review-addendum.md:53) requires the attached disable descriptor. Xorg emits hierarchy before clearing `dev->master` (`xorg-server-21.1.24:dix/devices.c:528–539`).

**Smallest correction:** explicitly make Task 1’s floating rule a completed-transition invariant. Preserve the pre-float attachment when building `XIDeviceDisabled`, with `enabled = false`, and float before subsequent queries.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation:** skipped because there is no prior review.
2. **Architecture and cross-task contracts:** assessed enabled-state ownership, backend cleanup entry points, hierarchy descriptor production, facet isolation, and the existing configuration lane. The two findings concern conflicting producer/consumer contracts.
3. **Safety, ownership and failure semantics:** verified the concrete release-order failure and attachment publication conflict. Grab detach/reattach storage was inspected; Task 1’s prohibition on reattaching disabled facets agrees with A2.
4. **Specification and verification:** read A1–A7/E2 and relevant main-design ownership, lifecycle, reset, and property sections. Compared relevant transitions and property validation against the requested Xorg tag, and A7 against xf86-input-libinput’s shared-device disable/check paths. The plan assigns formatting, tests, regular Clippy and hardware validation to implementation. No compiler, test, benchmark, install, or portability experiment was performed.

**Reading:** target plan read once; **24/24 additional excerpts** used. One cleanup excerpt inadvertently contained 121 lines, exceeding the per-excerpt cap by one line.

**Follow-ups (not defects):** investigation stopped at the budget boundary. These questions remain unassessed, rather than established sound:

- Does the existing request scheduling preserve Task 4’s required order between `Device Enabled` requests and asynchronous libinput writes?
- How do client-disabled changes reach the process-lifetime inventory before reset, given that inventory resides outside `ServerState` ([input_inventory.rs:24–28](/home/ariel_santangelo/Projects/yserver-xi-dynamic-registry/crates/yserver-core/src/core_loop/input_inventory.rs:24))?