# XKB request-coverage audit (cinnamon-xorg.xtrace) + golden vectors

Audit of every XKEYBOARD (major 135) request the real Cinnamon clients send in
`cinnamon-xorg.xtrace`, vs what yserver handles. Prompted by a `setxkbmap`
failure during FU3 HW testing. The trace is gitignored/local; this is the
durable record. Reply structs cross-checked against
`/usr/include/X11/extensions/XKBproto.h`.

## Coverage table (client demand vs yserver, 2026-06-25)
```
minor  request           sent  yserver before this work     verdict
0      UseExtension      166   real                         ok
1      SelectEvents      260   real (+merge fix)            ok
5      LatchLockState     11   real                         ok
6      GetControls        13   real                         ok
8      GetMap            158   real                         ok
17     GetNames           64   real                         ok
21     PerClientFlags     70   real                         ok
23     GetKbdByName        5   real                         ok
24     GetDeviceInfo       9   real                         ok
13     GetIndicatorMap     8   STUB (reply_minimal)         BUG: FU4 wired the body to
                                                            minor 22 (ListComponents) by
                                                            mistake; real opcode 13 stubbed
4      GetState           12   STUB (all-zero)              wrong after a switch (zero=group0
                                                            only); emit real state
15     GetNamedIndicator   6   STUB (all-zero)              emit real (reuses IndicatorMap data)
7      SetControls        12   ignored (void None)          void req (None is protocol-correct);
                                                            gap = controls not applied
10     GetCompatMap        6   empty                        FIXED group-compat (was BadAlloc
                                                            in libX11 -> setxkbmap broke);
                                                            124 sym-interps still deferred
-      _XKB_RULES_NAMES   (read) never published on root    BUG: setxkbmap can't read current
                                                            rules -> falls back to rules='base'
                                                            -> "Error loading new keyboard
                                                            description"; Xorg always sets it
```

## GetState (minor 4) — `xkbGetStateReply` (sz=32)
Layout: `mods@8 baseMods@9 latchedMods@10 lockedMods@11 group@12 lockedGroup@13
baseGroup:INT16@14 latchedGroup:INT16@16 compatState@18 grabMods@19
compatGrabMods@20 lookupMods@21 compatLookupMods@22 pad1@23 ptrBtnState:CARD16@24
pad2@26 pad3:CARD32@28`. length=0.
Golden (trace lines 4271, 10952): **all-zero** — captured while state was group 0 /
no mods, so a zeroed reply is correct *for that state*. Derive the real reply from
`core.xkb_state` + `core.locked_group`:
- `group` (effective) = `lockedGroup` = `core.locked_group` (the authoritative group
  yserver stamps into events; effective==locked since yserver has no base/latched group).
- `mods` (effective) / `lockedMods` from `xkb_state` serialize_mods(EFFECTIVE/LOCKED);
  base/latched mods 0 on a steady-state query.
Test: a steady group-0/no-lock state must byte-match the all-zero golden; a constructed
group-1 state must report `group=1 lockedGroup=1`.

## GetNamedIndicator (minor 15) — `xkbGetNamedIndicatorReply` (sz=32)
Layout: `indicator:Atom@8 found@12 on@13 realIndicator@14 ndx@15 flags@16
whichGroups@17 groups@18 whichMods@19 mods@20 realMods@21 virtualMods:CARD16@22
ctrls:CARD32@24 supported@28 pad1@29 pad2@30`. length=0.
Request body: `deviceSpec(2) ledClass(2) ledID(2) pad(2) indicator:Atom(4)`.
Golden (trace 87810/87812) — the map fields EXACTLY match the IndicatorMap golden
(slot 0 Caps, slot 1 Num) in `2026-06-25-xkb-indicator-compat-golden-vector.md`:
```
Num Lock  (atom 0xfe): found=1 on=0 realIndicator=1 ndx=1 flags=0x80 whichGroups=0 groups=0
                       whichMods=0x04 mods=0x10 realMods=0x00 virtualMods=0x0001 ctrls=0 supported=1
Caps Lock (atom 0xfd): found=1 on=0 realIndicator=1 ndx=0 flags=0x80 whichGroups=0 groups=0
                       whichMods=0x04 mods=0x02 realMods=0x02 virtualMods=0x0000 ctrls=0 supported=1
```
Derivation: reuse FU4's indicator parse (slot+name+map). Resolve the requested atom by
interning each indicator name and comparing (`intern_atom(name)==requested`). `on` =
`xkb_state.led_name_is_active(name)`. `realIndicator` = ndx in the real set (0x7ff, ndx<11).
`supported`=1. Not found → found=0/supported=1/rest 0.

## _XKB_RULES_NAMES root property
Xorg publishes `_XKB_RULES_NAMES` (type STRING, format 8) on the root at startup:
five NUL-terminated fields `rules\0model\0layout\0variant\0options\0`. `setxkbmap`
reads it (`XkbRF_GetNamesProp`) to learn the current rules before applying; absent →
it uses compiled-in default `rules='base'` (not `evdev`) and the load fails.
Fix: publish from the active RMLVO at startup, refresh on every keymap change
(GetKbdByName load + the property-write recompile). Loop-avoidance: the existing
ChangeProperty hook (`apply_rules_names_change`) recompiles on a client write — when
the SERVER writes the property to reflect a GetKbdByName load, the hook will re-parse
the same RMLVO and no-op (KeymapLoad unchanged), so it's safe; still, prefer setting
the property via a path that doesn't re-enter the recompile, or guard on
RMLVO-actually-changed. yserver already has `parse_rules_names` (the inverse) in
`xkb_layout.rs`.

## GetCompatMap (minor 10) — empty CompatMap BREAKS libX11 (root-caused via gdb)
An empty CompatMap (`length=0`) is NOT safely deferrable. libX11's
`_XkbReadGetCompatMapReply` (XKBCompat.c) calls `_XkbInitReadBuffer(dpy, &buf,
rep->length * 4)` with **no `if(rep->length)` guard** (the Map/Indicator/Names/
Geometry readers all have one). `_XkbInitReadBuffer` returns FALSE for `size <= 0`
(XKBRdBuf.c:40), so a zero-length compat → the reader returns `BadAlloc` →
`XkbGetKeyboardByName` BAILOUTs to NULL → `setxkbmap` prints "Error loading new
keyboard description". gdb backtrace: `_XkbReadGetCompatMapReply` (BadAlloc) ←
`XkbGetKeyboardByName` (XKBGetByName.c:166) ← `applyComponentNames`
(setxkbmap.c:1042) ← main. Cinnamon never hit this because muffin doesn't use
libX11's `XkbGetKeyboardByName`.
FIX (commit on this branch): `reply_get_compat_map` emits the group-compat block —
`groupsRtrn=0x0f` + 4 `xkbModsWireDesc` (group 1 none, groups 2-4 → Mod5 0x80),
byte-matching the captured Xorg reply (golden vector in
`2026-06-25-altgr-4level-golden-vector.md`) → `length=4>0`. `nSIRtrn=0`: the 124
sym-interpretations stay deferred (informational; libX11 accepts nSI=0). The full
124-SI encoder (63 are pointer/screen mouse-keys interprets) is the deferred piece.

## SetControls (minor 7) — DEFERRED to its own spec/plan (configurable autorepeat)
Void request (no reply) — returning None is protocol-correct, so nothing is broken today;
the gap is that the controls aren't *applied*. Sizing it revealed this is a feature, not a
stub-fill, and it spans more than XKB:
- yserver's autorepeat timing is HARDCODED (`REPEAT_INITIAL_DELAY=660ms`,
  `REPEAT_PERIOD=40ms` in `core_loop/run.rs:318`); the repeat-firing path
  (`fire_pending_repeats`) reads neither XKB SetControls NOR core ChangeKeyboardControl.
- `KeyboardControlState` (server.rs) has no repeat-delay/interval fields.
- `reply_get_controls` (xkb.rs) is static (reports 500/33, inconsistent with the actual
  660/40) and lives in the backend with no `ServerState` access — so GetControls
  consistency needs reshaping too.
Faithful implementation = "configurable autorepeat": add repeat config to state, parse
the XKB `changeControls` mask + values (and wire core ChangeKeyboardControl's repeat
fields too), rewire `fire_pending_repeats` to honor delay/interval + the per-key
`auto_repeats[32]` mask, and make GetControls read back the live values. Behavior-changing
on the interactive path (regression risk, needs a HW pass). Decided 2026-06-25 with the
user: track as its own spec/plan (codex-reviewed) rather than ad-hoc here.
GetControls(6) reply is already populated (trace line 4111, 92 bytes).
