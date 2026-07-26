# Protocol silent-success audit implementation plan

Date: 2026-07-26

## Goal

Remove misleading success responses from unsupported X11 request paths while
preserving deliberate, Xorg-compatible no-ops. Xorg in `../xserver` is the
compatibility oracle; protocol specifications are secondary when observable
Xorg behavior differs.

This work is intentionally split by behavior class. Unknown requests, known
but unsupported requests, and partially implemented requests must not be
changed in one undifferentiated patch.

## Phase 1: unknown core request opcodes

- [x] Confirm Xorg behavior in `../xserver/dix/tables.c`.
- [x] Keep opcode 127 as `NoOperation`.
- [x] Return core `BadRequest` for unassigned or unknown major opcodes.
- [x] Assert the error code, sequence, and major opcode on the wire.

Xorg's `ProcVector` routes reserved core opcodes 120 through 126 and
unregistered extension slots to `ProcBadRequest`. Yserver previously logged
and returned `RequestOutcome::Handled` without sending an error.

## Phase 2: unknown extension minor opcodes

- [x] Inventory the default arm of every locally handled extension.
- [x] Locate the corresponding Xorg dispatcher in `../xserver`.
- [x] Convert silent defaults to the matching core or extension error.
- [x] Add table-driven wire regressions covering error code, major opcode,
  and minor opcode.

Do not include a known minor in this phase merely because its implementation
is incomplete. This phase is only for request numbers outside the extension's
defined dispatch table.

The comparison covered RENDER, RANDR, SYNC, SHAPE, XFIXES, COMPOSITE,
MIT-SHM, DAMAGE, XTEST, DPMS, PRESENT, DRI3, X-Resource, XI1/XI2, XKB,
Generic Event, BIG-REQUESTS, MIT-SCREEN-SAVER, XINERAMA, and XC-MISC.
All use core `BadRequest` for an out-of-table minor in Xorg. GLX remains the
intentional exception: its unknown-minor path uses the GLX extension error and
already has separate coverage.

Primary Xorg dispatcher references:

- `../xserver/render/render.c::ProcRenderDispatch`
- `../xserver/randr/randr.c::ProcRRDispatch`
- `../xserver/Xext/{sync,shape,shm,xtest,dpms,xres,geext,bigreq}.c`
- `../xserver/xfixes/xfixes.c::ProcXFixesDispatch`
- `../xserver/composite/compext.c::ProcCompositeDispatch`
- `../xserver/damageext/damageext.c::ProcDamageDispatch`
- `../xserver/present/present_request.c::proc_present_dispatch`
- `../xserver/dri3/dri3_request.c::proc_dri3_dispatch`
- `../xserver/Xi/extinit.c::ProcIDispatch`
- `../xserver/xkb/xkb.c::ProcXkbDispatch`

RENDER, RANDR, and XFIXES have defined request numbers that yserver does not
yet implement. Their defaults distinguish out-of-table minors (now
`BadRequest`) from defined-but-incomplete minors (left for Phase 4).

## Phase 3: known core no-ops and stub replies

- [x] Audit `GrabServer` and `UngrabServer` against Xorg's cross-client
  scheduling semantics.
- [x] Audit `RecolorCursor` and add real cursor-state behavior if clients can
  observe it.
- [x] Audit `GetMotionEvents` and other reply-bearing core stubs.
- [x] Classify each path as compatible no-op, missing implementation, or
  capability that should be reduced.

`NoOperation` remains a required successful no-op.

`GrabServer` now records an owner in `ServerState`; the core loop parks other
clients' requests in arrival order until the owner ungrabs or disconnects.
This follows Xorg's `grabClient` / `grabWaiters` behavior in
`../xserver/dix/dispatch.c::ProcGrabServer` and `ProcUngrabServer`.

`GetMotionEvents` now validates the requested window like Xorg
`../xserver/dix/devices.c::ProcGetMotionEvents`, returning `BadWindow` rather
than a successful reply for an invalid xid. Its zero-event reply is an
intentional current capability boundary: yserver does not retain pointer
motion history.

The reply-bearing paths originally grouped as "stubs" are classified as
implemented: `SetFontPath` / `GetFontPath` use backend-owned state,
`ListInstalledColormaps` uses tracked server state and validates its window,
and pointer/modifier mapping requests update state and emit `MappingNotify`.

`RecolorCursor` validates the cursor and returns `BadCursor` for an unknown
xid, matching `../xserver/dix/events.c::ProcRecolorCursor`. Core monochrome
cursor records now retain transparent/foreground/background pixel roles, so
the primary yserver KMS backend can regenerate and upload the sprite with new
colors even when its old foreground and background colors were identical.
Active sprites refresh immediately. RENDER/ARGB cursors remain unchanged,
matching Xorg's `xf86RecolorCursor_locked`. Separately, ynest forwards the
exact core request to its host server for nested-backend parity; Xnest is a
behavior reference, not the implementation target. Animated cursor wrappers
retain Xorg's constituent-frame color behavior.

## Phase 4: known extension requests with partial or empty success

- [ ] RANDR reply-bearing stopgaps and silently accepted void requests.
  - [x] Provider minors 33–41 return Xorg-compatible `BadProvider` while
    `GetProviders` advertises no providers; this closes reply hangs in 33, 36,
    37, and 41 as well as false success in the void provider requests.
  - [x] `FreeLease` returns `BadLease` while `CreateLease` cannot create one.
  - [x] Reply and selection paths validate their window/output/CRTC before
    returning data, using RANDR `BadOutput` / `BadCrtc` rather than generic
    `BadValue` where Xorg does.
  - [x] `SetOutputPrimary` updates tracked state, supports clearing with
    `None`, and rejects an unknown output.
  - [ ] Implement or accurately reject the remaining custom-mode, output
    property, transform, panning, primary-output notification, monitor, and
    lease paths.
- [ ] X-Resource byte/PID accounting replies.
  - [x] `QueryClientPixmapBytes` computes 64-bit live pixmap storage with X11
    bits-per-pixel and scanline padding rules; unknown client ranges return
    Xorg's `BadValue`.
  - [x] `QueryClientIds` reports ClientXID identities and correctly omits PID
    identities because peer credentials are not retained.
  - [ ] Add peer-PID retention and recursive `QueryResourceBytes` accounting.
- [ ] GLX texture-from-pixmap behavior that currently binds without indirect
  texture sampling.
- [x] XI1 validated zero-reply/no-state-change paths.
  - [x] Replace the empty `GetSelectedExtensionEvents` reply with canonical
    per-window selection state, including Xorg's this-client then all-clients
    class lists. Server-wide notification delivery is now a derived aggregate
    rather than the only copy of selection state.
  - [x] Make window teardown remove core, XI1, and XI2 subscriptions; XI1's
    server-wide delivery aggregate is rebuilt from surviving windows.
  - [x] Reclassify the broad `TODO(no-stub)` block: its reply-bearing mapping,
    focus, grab, state, and most control paths already use tracked state;
    remove the obsolete blanket marker.
  - [x] Return Xorg's `BadValue` from `DeviceBell` when the selected feedback
    has no bell procedure (all feedbacks yserver currently exposes), instead
    of silently succeeding; preserve Xorg's percent-first validation order.
  - [x] Implement `ChangeFeedbackControl` for the KbdFeedback and PtrFeedback
    classes yserver advertises. Changes share core keyboard/pointer state,
    validate and stage like Xorg, and support the tagged big-endian payload.
  - [x] Confirm against Xorg initialization that yserver's relative axes
    correctly advertise resolution/min/max-resolution `0/0/0`; pin the valid
    zero round trip and complete big-endian `ChangeDeviceControl` swapping.
  - [x] Retain the last 256 translated pointer-motion samples at the single
    authoritative fanout boundary. Core `GetMotionEvents` filters and
    translates them per window; XI1 `GetDeviceMotionEvents` returns time plus
    four valuators for pointer devices.
- [ ] Other explicit `stub`, `unsupported`, and `TODO(no-stub)` sites found by
  the inventory.
  - [x] Replace XTEST `CompareCursor`'s hardcoded `same=true` with inherited
    window-cursor comparison, `None` / `XTestCurrentCursor` handling, resource
    validation, and byte-order-aware request decoding.

For each request:

1. Compare the real Xorg dispatcher and implementation.
2. Check whether yserver advertises the operation through its negotiated
   version or capabilities.
3. Prefer implementation when real clients use it.
4. Otherwise return the Xorg-compatible error or lower the advertised
   capability/version.
5. Add a regression that proves both wire behavior and relevant state
   behavior.

## Validation

Every phase must run:

- `cargo +nightly fmt`
- `cargo clippy --all-targets -- -D warnings`
- focused `yserver-core` tests
- `cargo test --workspace`

Protocol behavior with any ambiguity should additionally be compared using
`x11trace -n` against Xorg or the appropriate nested Xorg server.

Latest validation (2026-07-26, Phase 4 worktree): nightly formatting and
CI-equivalent Clippy pass; the workspace suite passes with 1,994 tests passed
and 173 ignored.
