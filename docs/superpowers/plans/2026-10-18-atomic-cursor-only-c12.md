# Atomic-only cursor — C12: decide cursor-only commits after queued client requests

> **For agentic workers:** one task. Implement against the spec section, not against this text's wording.

**Goal:** a flip event no longer sends a cursor-only commit from inside its handler. The choice between the cursor
riding the scene commit and a cursor-only commit is made once per core-loop iteration, after the iteration's backend
events and queued client requests are processed (spec §2.6).

**Spec:** `docs/superpowers/specs/2026-10-15-atomic-cursor-only-design.md` §2.6 and §4 item 12.

## Global Constraints

- Tests follow rules A/B/C/F:
  - A: end-state and leak checks;
  - B: driven only through `Backend`/`run_core`;
  - C: hardware only in C11, with the user;
  - F: GPU-waiting tests run 20× under 16 busy loops.
- No `cfg(test)` branches in production bodies. No `for_tests_*` setters in the middle of a scenario.
- No `YSERVER_*` environment variables that change behaviour.
- Do not copy or translate code from copyleft projects (KWin, Mutter).
- Keep every C11 r1–r3 rule:
  - the recovery detach stays cursor-only (spec §3);
  - a failed scene submit releases its cursor intents to the cursor-only lane;
  - retry backoff never parks cursor intent;
  - a deferred sprite upload stays retryable.
- Keep `run.rs` changes minimal. The core must tell the backend whether queued requests remain after the bounded
  dispatch (count/time budget, run.rs ~:500–554 and ~:1811–1851). It reports a boolean only, and the budgets stay
  unchanged. The report reaches the backend after each dispatch and **before** composition (`run_iteration_tail` →
  `maybe_composite`, run.rs ~:3097). The decision points are the composition tail and `Backend::before_block`.
  `before_block` keeps its GPU-reclamation duty.
- The pending decision's 2 ms deadline must feed the poll timeout. The timeout is computed before `before_block`
  today, so place the deadline in the existing backend deadline path. Do not block past it.

## Review Focus

1. **The real ordering:** the PresentPixmap is already queued when the flip event is handled. The current
   `round_g_vblank_frame_work.rs` delivers scene work before the event, and that is why it missed the bug.
2. **An idle scene:** the cursor-only commit must still go out in the same iteration, with no extra vblank and no
   timer.
3. **Several outputs:** a decision on output A must not block or submit for output B. The gate stays output-local.
4. **Held direct frames and tearing:**
   - the held-direct lane honours the gate, so a direct FIFO client's successor carries the cursor (the C11 AMD
     `anim-direct` run alternated too);
   - the tearing rules (cursor software while tearing is On) are unchanged.
5. **NVIDIA-like drivers** (scene and cursor-only in the same vblank): the cursor rate must not drop.

---

### Task C12

**Files:**
- Modify: `crates/yserver/src/kms/render/backend/kms/scanout.rs`. The drain block in
  `backend_scanout_on_page_flip_ready` (currently ~:2454) becomes "mark that this output needs a cursor decision".
- Modify: one output-local gate consulted by every ordinary cursor-only lane:
  - the flip handler;
  - the held-direct lane (~:2842);
  - independent peer-output updates (~:2936);
  - the post-compose fallback (~:3008);
  - the decision points (composition tail, `before_block`).
- Modify: the core's backlog report (`yserver-core` run loop, minimal).
- Test: a new `crates/yserver/src/kms/render/backend/tests/atomic_cursor_only/round_i_flip_ordering.rs`, plus its
  `mod` line.

**Interfaces:**
- Consumes the existing items: `output_has_scene_frame_work`, `drain_pending_cursor_intent_for_output`,
  `has_eligible_atomic_cursor_intent_for_output`, `cursor_recovery_detach_pending_for_output`, and the
  grouped-direct and tearing guards used today in the flip handler.
- Produces:
  - a per-output "cursor decision pending" mark, holding the time of the retired flip event; flip retirement sets
    it, and the post-dispatch decision consumes it exactly once;
  - a core→backend backlog report (one boolean per iteration).
- The mark is cleared or re-keyed on a topology epoch change, output removal, DPMS off and VT leave. It never
  submits for an output that no longer exists or is inactive.

**Invariants (spec §2.6):**
- While an output is awaiting a frame, no ordinary lane sends a cursor-only commit for it. A scene or direct commit
  built for it meanwhile carries the intent.
- The exceptions bypass the gate: the recovery detach, a failed scene's released intents, and the retry rule.
- The wait ends at the first decision point where the output has no scene or direct work **and** the backlog report
  is false, or at the first decision point after 2 ms of loop time since the flip. It then sends one cursor-only
  commit. The 2 ms deadline feeds the poll timeout. Dispatch is never preempted.
- A mark is never lost. An iteration that retired a flip with eligible intent produces either a `SceneWithCursor`
  carrying the intent or a cursor-only commit, no later than the next compose.

**Tests (spec §4 item 12), through `run_core`:**
1. **FIFO client with the Present already queued.** Vblank-locked model, 60 fps client, continuous motion, 30
   vblanks: `SceneWithCursor` ≥ 28, and no `CursorOnly` while a frame is queued. The client must present when it
   observes COMPLETE, so the request is queued before the next flip event is processed.
2. **Idle scene.** The same model with the client stopped: a cursor-only commit every vblank, sent in the iteration
   that retired the flip.
3. **Budget backlog.** The PresentPixmap is queued behind more requests than the dispatch budget: the decision
   waits and the cursor rides the scene (`SceneWithCursor`, no `CursorOnly` for that vblank).
4. **Request flood.** A backlog of unrelated requests stays queued for the whole run. The injected clock advances
   during dispatch and across expiry. A cursor-only commit goes out at the first decision point after 2 ms, never
   earlier while the backlog lasts.
5. **Held direct FIFO client.** The held direct frame of a fullscreen FIFO client has fully retired. Its successor
   Present is queued *beyond the dispatch budget*, and injected time stays below 2 ms. Composition is reached with
   eligible cursor intent while the successor is still queued in the core. Assert:
   - no cursor-only submission in that pass;
   - after the successor is dispatched, the successor carries the cursor delta.

   Removing the held-direct gate must cause the premature cursor-only submission. Without the backlog, the
   successor sets `scanout_m2.pending` and composition returns before the held-direct lane, so that case proves
   nothing about the gate.
6. **Regressions:** the existing `round_g_vblank_frame_work.rs` and `round_h_retry.rs` pass, and so do the tearing
   and atomic_cursor_only suites.

**Mutations that must fail:**
- restore the drain inside the flip handler (test 1 fails);
- evaluate the gate before queued client requests (test 1 fails);
- defer the idle cursor-only commit to the next vblank (test 2 fails);
- drop the mark when the gate defers and the scene then fails to submit (an `r3` test or test 1 fails);
- ignore the backlog report (test 3 fails);
- remove the 2 ms bound (test 4 fails);
- let the held-direct lane bypass the gate (test 5 fails), or let the post-compose lane bypass it (test 3 fails).

**Gate:**
- `cargo +nightly fmt`;
- `clippy -D warnings` (workspace, `--features xdmcp`);
- `cargo test --workspace --locked --features xdmcp`;
- the full ignored lavapipe suite;
- `tools/check-layout.sh`;
- the new tests 20× under 16 busy loops.
