# Plan: GLX reply parity with Xorg

Design: `docs/superpowers/specs/2026-08-03-glx-reply-xorg-alignment-design.md`
(rev 4, **approved** after Opus review rounds 1-3)
Branch: `glx-reply-xorg-alignment`, off `master` @ `468e4f2`.
**All line numbers below are against that base.** Re-derive before
editing if the branch has moved.

## Goal

Bring two GLX replies into agreement with Xorg: `GetDrawableAttributes`
(D1-D5 + the error arm) and `MakeCurrent` (D6).

D1 is a live defect against Mesa clients taking the `glXCreateWindow`
path. Landing D1-D5 is also what makes the NVIDIA A/B experiment
interpretable — Task 7. This plan does **not** assume the NVIDIA bug is
thereby fixed.

**D7 (`QueryContext`) and D8 (`CreateContext` misparse) are out of
scope** — cut against the project's indirect-GLX non-goal. Do not
implement them here; see the design's "Deferred" section. Task 5 records
them.

## Ordering rationale

D1 ships first and alone: highest severity, proven-failing test,
independently correct. Task 2 (drawable kind + event mask) is a
prerequisite for the attribute restructure. D6 is independent of both.

## Tasks

### Task 1 — D1: geometry from the backing drawable

TDD. Write the failing test first.

- Test in `process_request.rs` tests: create a 64×32 X pixmap, wrap it
  via a `CREATE_PIXMAP` request, call `drawable_attributes_for`, assert
  `GLX_WIDTH = 64` / `GLX_HEIGHT = 32` / `GLX_FBCONFIG_ID = 0x101`.
  **Confirm it fails against current code with `left: 0, right: 64`**
  before writing the fix.
- Fix: in `drawable_attributes_for` (`process_request.rs:10646-10659`)
  resolve the geometry XID as `d.x_drawable` when a record exists and is
  not a pbuffer, else `xid`.
- A reference implementation of exactly this change, built and
  test-verified then reverted so the design could be reviewed first, is
  at `~/yserver-glx-logs/2026-08-03-investigation/glx-geometry-fix.patch`
  (verified `git apply --check` clean against `468e4f2`). Its hunk
  headers carry stale line numbers — written against a different branch —
  so it applies only via git's context search. Treat as a starting point,
  not as pre-approved; the change itself is four lines. It also contains
  the regression test, which was confirmed to fail pre-fix with
  `left: 0, right: 64`.

### Task 2 — drawable kind and event mask on `GlxDrawable`

- Add `GlxDrawableKind { Window, Pixmap, Pbuffer }` to `server.rs:1187`.
  **No `Default` impl** — a missed construction site must be a compile
  error.
- Set it at **all three** `glx_drawables.insert` sites:
  - `process_request.rs:11847` — `CREATE_WINDOW | CREATE_PIXMAP`
    (branch on the minor)
  - `process_request.rs:11887` — `CREATE_PBUFFER`
  - `process_request.rs:12277` —
    `VENDOR_CODE_CREATE_GLX_PIXMAP_WITH_CONFIG_SGIX` → `Pixmap`
  - plus the struct literal at `server.rs:5690`, which is inside
    `xid_occupied_covers_every_namespace()` (`server.rs:5490`) — a
    resource-ID allocator test, **not** a snapshot/restore mechanism.
    `ServerState` has no snapshot/restore at all.
- Add `event_mask: u32` (default 0). Change
  `CHANGE_DRAWABLE_ATTRIBUTES` (`process_request.rs:11978`) to store
  **only** `GLX_EVENT_MASK` into it and ignore every other attribute,
  matching `glxcmds.c:1494-1503`. Remove `GlxDrawable::attributes` if no
  other reader remains (grep before deleting).
- Re-key `drawable_attributes_for`'s `is_pbuffer` and
  `glx_pbuffer_geometry` (`process_request.rs:27409-27423`) onto the
  kind. The 0×0-pbuffer edge case changes behaviour in **two** places;
  preserve both:
  - `glx_pbuffer_geometry` returns `None` when
    `width == 0 && height == 0`. **Keep that size guard** — dropping it
    makes a 0×0 pbuffer return `Some(0×0)`, changing the `GetGeometry`
    fallthrough at `process_request.rs:20828` from `BadDrawable` to
    Success.
  - In `drawable_attributes_for`, `is_pbuffer` is currently *false* for a
    0×0 pbuffer, so geometry falls through to `resources.pixmap(xid)`,
    which hits — `CREATE_PBUFFER` creates a backing pixmap clamped to
    `max(1)` (`process_request.rs:11905-11907`) — and reports 1×1.
    Re-keyed onto `kind == Pbuffer` it would report 0×0.
- Existing tests must stay green.

### Task 3 — D2/D3/D4/D5 + error arm: attribute set matching Xorg

TDD, one test per drawable kind plus the error case.

- Tests asserting the exact attribute list and order for: naked X window,
  GLXWindow, GLXPixmap, pbuffer. Reference `glxcmds.c:1890-1914`. Naked
  window must assert `GLX_FBCONFIG_ID` **absent** and
  `GLX_DRAWABLE_TYPE = GLX_WINDOW_BIT` present.
- **Request-level wire tests** through `process_request`, asserting reply
  bytes (`numAttribs`, `length = 2n`) for at least the naked-window case,
  and **both error arms separately** (see below). Convention:
  `glx_create_pixmap_records_x_drawable_and_destroy_clears_it`
  (`process_request.rs:52040`).
- Restructure `drawable_attributes_for` per the design. Drop
  `GLX_RENDER_TYPE`. Delete the override pass
  (`process_request.rs:10674-10679`).
- `GET_DRAWABLE_ATTRIBUTES` (`process_request.rs:12008`) emits **two
  different error codes**, per the design's error-arm table: a naked X
  pixmap → `GLX_FIRST_ERROR + ERROR_GLX_BAD_DRAWABLE`; an XID that is not
  a drawable at all → **core `BadDrawable`**. Do not collapse them.
- Add missing constants to `yserver-protocol/src/x11/glx.rs` with
  citations, matching the file's convention:
  - `GLX_EVENT_MASK`, `GLX_STEREO_TREE_EXT` (glxext.h)
  - `ERROR_GLX_BAD_DRAWABLE: u8 = 2` (`glxproto.h:43`)

  `GLX_SCREEN` (0x800C) and `GLX_VISUAL_ID` (0x800B) already exist at
  `glx.rs:327-328` and are numerically identical to the `_EXT` spellings.

### Task 4 — D6: `MakeCurrent` returns tag 0 on release

TDD.

- Tests: release form returns `contextTag = 0`; non-null context returns
  nonzero. Cover **both** minors.
- Parse the body in the `MAKE_CURRENT | MAKE_CONTEXT_CURRENT` arm
  (`process_request.rs:11776`), which ignores it entirely today. Layouts
  are in the design's D6 table, verified against `glxproto.h:225-233` and
  `glxproto.h:471-481`. Body is header-relative-4, consistent with the
  existing `GET_DRAWABLE_ATTRIBUTES` parse.

### Task 5 — `docs/status.md`

Update per AGENTS.md. Three things must land:

- The GLX reply-parity work (D1-D6 + the error arm).
- **The NVIDIA `BadAlloc` remains open.** Do not let the status file
  imply this change fixed it.
- **D7 and D8 as known, deliberately-deferred defects**, with a pointer
  to the design's "Deferred" section. D8 in particular is a real
  misparse that is currently inert — record it so it is not
  rediscovered from scratch.

### Task 6 — gates

- `cargo +nightly fmt`
- `cargo clippy --all-targets -- -D warnings` (exactly as CI runs it)
- `cargo test -p yserver-core` — full suite green, not just the GLX
  filter.

### Task 7 — on-hardware measurement (user-run, tty2)

Not an agent task; yserver takes the console and KMS.

- Rerun `~/yserver-glx-probes/run-probe-ab.sh`.
- The decisive datum: does `nvidia-glxwindow` still `BadAlloc` now that
  the reply is fully conformant? If yes, `GetDrawableAttributes` is
  exonerated and the investigation moves to the next suspect. Record the
  result in the design's "What this design does not claim" section either
  way.
- Watch the teardown: the last full-session shutdown logged
  `atomic disable_output failed` ×3 and `DROP_MASTER: EINVAL`, a
  suspected cause of the tty corruption seen on 2026-08-03. The short
  probe session did not reproduce it.

## Commits

One commit per task, conventional-commit style, scoped `glx`. Do not put
a Claude session URL in any commit message (CLAUDE.md).

## Stop conditions

Stop and report rather than improvising if:

- Any line number in this plan does not point at what it claims — the
  branch has moved; re-derive rather than guessing.
- The `MakeCurrent` body layouts do not match the design's D6 table
  (Task 4). Getting this wrong silently breaks every GL client.
- Dropping `GLX_FBCONFIG_ID` for naked windows, or either new error arm,
  breaks an existing test in a way that suggests a real client
  dependency (Task 3).
- `GlxDrawable` field changes ripple into disconnect cleanup or the
  resource-id allocator test (`server.rs:5490`) beyond a mechanical
  update.
- You find yourself wanting to touch `QUERY_CONTEXT`
  (`process_request.rs:12047`) or the `CREATE_CONTEXT` arm
  (`process_request.rs:11728`). Both are **out of scope** (D7/D8) and
  both have known traps documented in the design's "Deferred" section.
  Report instead of expanding the change.
