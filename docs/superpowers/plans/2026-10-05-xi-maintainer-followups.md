# Maintainer follow-ups on PR #198 — plan 4

**Source:** the maintainer's second review of PR #198 (2026-10-05), four items.
**Spec:** [maintainer-review addendum](../specs/2026-10-04-xi-registry-maintainer-review-addendum.md),
amended in Task 0. **Branch:** `feat/xi-dynamic-registry-rebased`.
**Scope:** these four items only.

**Executor and rules:** identical to [plan 1](2026-10-04-xi-enabled-state-and-floating.md),
"Rules for every task".

---

### Task 0 — Addendum amendments (coordinator, docs only)

- **C3:** every XI1 consumer of the class shape follows the device's actual
  classes: `XListInputDevices`, `XOpenDevice`, `GetDeviceButtonMapping`,
  `SetDeviceButtonMapping` and `QueryDeviceState`.
- **C6 (new):** a wheel button (4–7) becomes smooth-scroll Motion only when the
  source device has a scroll class in that direction. Xorg `GetPointerEvents`
  converts through `v_scroll_axis` / `h_scroll_axis` (`dix/getevents.c`). XTEST
  4 has none, so it sends plain wheel buttons, and physical devices keep smooth
  scrolling.
- **G:** on a barrier timeout, the in-flight write is cancelled with a token
  the input thread checks before it applies. If libinput already applied it,
  the late result is reconciled: inventory and property are committed, and
  the property event is sent. The client keeps the `BadMatch` it got.
- **A1:** a session disable records `session_enabled = false` even for a
  client-disabled facet. Enabling it while suspended clears only the
  client fact, and it re-enables at resume.

### Task 1 — XTEST sends plain wheel buttons (C6)

**Do:** gate the 4–7 → smooth-scroll conversion in `pointer_fanout.rs`
(the paths around `(4..=7).contains(&event.detail)`) on the source device's
scroll classes. For a master event, use the generating slave's classes.

**Tests (B via XTestFakeInput and physical scroll input):**
1. XTEST button 5 delivers ButtonPress/Release 5 to slave 4 and master 2
   listeners, and no XI2 Motion with axes 2/3 on either. *Kills:* the
   ungated conversion.
2. Physical wheel input still produces smooth-scroll Motion (an existing
   test may already cover this; keep it).

### Task 2 — XI1 button mapping and device state use the real shape (C3)

**Do:** `GetDeviceButtonMapping`, `SetDeviceButtonMapping` and
`QueryDeviceState` (`process_request.rs`, around the XI1 handlers) derive the
button count and valuator count from the device's class shape (the same
source XIQueryDevice uses), for slaves and masters. Follow the Xorg handlers
(`Xi/getbmap.c`, `Xi/setbmap.c`, `Xi/queryst.c`) for errors and layout.

**Tests (B through the dispatcher):**
1. On 4: GetDeviceButtonMapping returns 10 entries, SetDeviceButtonMapping
   accepts a 10-entry map and rejects an 11-entry one as Xorg does, and
   QueryDeviceState reports 10 buttons and 2 valuators. *Kills:* the
   hard-coded 7/4.
2. On a physical pointer: unchanged values.

### Task 3 — VT-release timeout cannot lose an applied write (G)

**Do:** two parts, used together.
- **Cancel:** each submitted config command carries a cancel token shared
  with the core. On a barrier timeout, the core sets it before answering
  `BadMatch`. The input thread checks it before applying and reports
  "cancelled" without touching libinput.
- **Reconcile:** the core keeps a record of timed-out operations. A late
  "applied" result for one commits the confirmed value to inventory and
  property and sends the property notification, with no second reply. A late
  "cancelled" or failure result only drops the record.

Replace the "discard as stale" behavior of `fail_in_flight_xi_config_for_vt_release`
(`run.rs`). Keep the 500 ms bound.

**Tests (run_core path):**
1. Timeout before the input thread starts the command: token set, the
   command reports cancelled, libinput never sees it, and the state is
   unchanged. *Kills:* the missing cancellation check.
2. Timeout, then a late applied result: inventory and property hold the
   applied value, one property event, and no reply after the BadMatch.
   *Kills:* discarding the late result.

### Task 4 — Session disable is recorded for a client-disabled facet (A1)

**Do:** `publish_disabled_xi_facet` (`backend.rs`) records
`session_enabled = false` before its early return for an already disabled
facet. The early return then only skips the publication. The resume path
then re-enables the facet only if it is no longer client-disabled.

**Tests (B via Device Enabled requests and `on_host_input` lifecycle):**
1. Client-disable, suspend, `Device Enabled = 1` while suspended: the facet
   stays disabled and floating, with no Enabled events. After resume it is
   enabled, with Enabled events. *Kills:* the early return skipping the
   session fact.
