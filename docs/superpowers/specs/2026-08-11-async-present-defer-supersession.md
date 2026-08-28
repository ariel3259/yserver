# Async Present Defer + Target-Scoped Supersession Spec (delta)

Date: 2026-08-11. Status: **Amended after review on 2026-08-28.**
Related findings: `docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md`
(§4a, §4b). Supersedes the "async collapses the due rule to always-now"
decision recorded in `present_scheduler.rs` for the async (`PresentOptionAsync`)
case only.

## Problem

A fullscreen game without vsync (CS2 on RADV) presents uncapped (~200-300/s,
`PresentPixmapSynced` + `PresentOptionAsync`). Because async presents get
`effective_target_msc = None`:

- `classify_msc_due(None, _, _)` returns `ExecuteNow` always
  (present_scheduler.rs:94-95) → no parking.
- `supersede_covered_pending_presents` early-returns on `effective_target_msc ==
  None` (process_request.rs:8810-8812) → no supersession.

Result: every present executes immediately, marks full damage, and re-composes
on the same GPU the game renders into. Xorg scraps only requests whose CRTC and
target MSC are equal; an unknown effective target is not enough to prove that
two requests belong to the same scrap group.

## Design

Async requests may be deferred behind an in-flight flip, but supersession stays
restricted to requests with a known, equal effective target MSC.

### 1. `classify_msc_due` — park async presents while a flip is in flight

```
classify_msc_due(eff=None, clock_msc, flip_in_flight):
    return Park      if flip_in_flight
    return ExecuteNow otherwise
```

Rationale: an async present cannot flip before the current in-flight flip
retires (KMS allows one flip in flight per CRTC). Parking to the next vblank
adds no latency vs today (ExecuteNow also waits for the flip), but makes the
in-flight dependency explicit in the scheduler. It does not by itself prove
that two unknown-target requests are equivalent for scrapping.

### 2. `supersede_covered_pending_presents` — preserve Xorg target identity

A successor may scrap a covered pending predecessor only when all of these are
equal:

- window;
- CRTC and CRTC epoch;
- known `effective_target_msc` (`Some(target)`).

The async option bit does not establish equivalence. Async requests with the
same known effective target may supersede; requests with different targets may
not. If either effective target is `None`, the safe behavior is to retain the
predecessor until equivalence is known. The existing coverage predicate remains
an additional conservative gate.

## Out of scope

- Direct scanout for fullscreen games (separate efficiency work — the
  scanout-m1/m2 gate relaxations are in the companion plan, which also adds
  direct-level latest-wins supersession). This spec is the PRIMARY flood fix.
- GetImage / MIT-SHM readback latency (separate).

## Acceptance

1. An async request behind an in-flight flip parks until that flip retires.
2. Async requests with the same known effective target supersede when coverage
   passes; different targets do not.
3. `effective_target_msc = None` never supersedes, including `None`/`None`.
4. Superseded requests preserve ordered `CompleteNotify{Skip}` delivery.
5. CI: `cargo clippy --all-targets -- -D warnings` clean.
