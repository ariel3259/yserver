# XKB IndicatorMap + CompatMap — golden vectors (GetKbdByName embedded blocks)

External golden vectors decoded from a real-Xorg `xtrace` of Cinnamon loading a
`us,de` keymap (`cinnamon-xorg.xtrace:6202`, request
`symbols="pc+us+de:2+us:3+inet(evdev)+pc(pc105)"`). The trace itself is
gitignored/local; this is the durable record. The block walk consumed exactly
to the end of the reply (15912/15912 body bytes), so the offsets below are
verified.

Wire structures cross-checked against `/usr/include/X11/extensions/XKBproto.h`
(`xkbGetIndicatorMapReply`, `xkbIndicatorMapWireDesc`, `xkbGetCompatMapReply`,
`xkbSymInterpretWireDesc`, `xkbModsWireDesc`).

The `GetKbdByName` reply embeds five blocks, each a full standalone reply
(8-byte generic prefix + body), concatenated in XkbGBN order:
`[GetMap][CompatMap][IndicatorMap][GetNames][Geometry]`. The header carries
`found=0x007f reported=0x00ff minKeyCode=8 maxKeyCode=255 loaded=1 newKeyboard=0`.

Xorg POPULATES all three of CompatMap/IndicatorMap/Geometry. yserver historically
emitted empty-but-valid versions. **IndicatorMap is being populated (this work);
CompatMap is deferred (see below); Geometry stays empty with `found`=FALSE** —
libxkbcommon dropped geometry entirely and Xorg only has it via xkbcomp; no
Wayland/xkbcommon-x11 client consumes it.

## Derivation source

Both blocks' bodies are recoverable from `xkb_keymap_get_as_string(keymap,
XKB_KEYMAP_FORMAT_TEXT_V1)`:
- IndicatorMap: `xkb_keycodes` `indicator N = "Name";` decls (slot indices) +
  `xkb_compatibility` `indicator "Name" { ... };` defs (map bodies).
- CompatMap: `xkb_compatibility` `interpret ... { action= ...; };` statements.

Two IndicatorMap fields are NOT serialized by libxkbcommon (constant per the
standard XKB indicator names; supply from the XKB standard, golden-vector-tested):
- per-indicator `flags` (see table)
- `realIndicators` = `0x7ff` (the evdev real-indicator block, indicators 1–11;
  the compat-added virtuals Shift Lock/Group 2/Mouse Keys at indices 11–13 are
  excluded).

## IndicatorMap golden vector (`us,de`)

Header: `which=0xffffffff realIndicators=0x000007ff nIndicators=32`.

`xkb_keycodes` slot map (wire index = `indicator N` − 1):
```
1 Caps Lock→0   2 Num Lock→1   3 Scroll Lock→2   4 Compose→3   5 Kana→4
6 Sleep→5  7 Suspend→6  8 Mute→7  9 Misc→8  10 Mail→9  11 Charging→10
12 Shift Lock→11   13 Group 2→12   14 Mouse Keys→13
```
Indices 3–10 and 14–31 are named-but-empty (all-zero 12-byte maps).

Non-empty `xkbIndicatorMapWireDesc` (flags, whichGroups, groups, whichMods, mods,
realMods, vmods, ctrls = 1,1,1,1,1,1,2,4 bytes = 12B):
```
idx  flags wG groups wM mods real vmods   ctrls       rawhex                        source (compat indicator def)
 0   0x80  0  0x00   4  0x02 0x02 0x0000 0x00000000   800000040202000000000000      Caps Lock:  whichModState=locked; modifiers=Lock
 1   0x80  0  0x00   4  0x10 0x00 0x0001 0x00000000   800000041000010000000000      Num Lock:   whichModState=locked; modifiers=NumLock(vmod→Mod2)
 2   0x00  0  0x00   4  0x00 0x00 0x0080 0x00000000   000000040000800000000000      Scroll Lock:whichModState=locked; modifiers=ScrollLock(vmod)
11   0x80  0  0x00   4  0x01 0x01 0x0000 0x00000000   800000040101000000000000      Shift Lock: whichModState=locked; modifiers=Shift
12   0x80  8  0xfe   0  0x00 0x00 0x0000 0x00000000   8008fe000000000000000000      Group 2:    groups=0xfffffffe (wG=8 UseEffective, g=low byte 0xfe)
13   0x20  0  0x00   0  0x00 0x00 0x0000 0x00000010   200000000000000010000000      Mouse Keys: controls=MouseKeys (ctrls=0x10), flags=LEDDrivesKB
```

Field encodings:
- `whichMods`/`whichGroups` (XkbIM_*): UseBase=1, UseLatched=2, UseLocked=4,
  UseEffective=8, UseCompat=16. `whichModState=locked` → 4.
- `mods`/`realMods`/`vmods`: a real-mod name (Shift=0x01, Lock=0x02, Control=0x04,
  Mod1=0x08…Mod5=0x80) sets `realMods` and `mods`. A virtual-mod name (NumLock,
  ScrollLock…) sets the `vmods` bit AND `mods` gets the real mask the vmod is
  bound to (NumLock→Mod2 0x10; ScrollLock bound to nothing real → mods 0).
  Resolve vmod→real via the keymap (same path as `virtual_mods_from_keymap`).
- `groups`: a group list → low byte in `groups`; `whichGroups` defaults to
  UseEffective (8) when only `groups=` is given.
- `ctrls`: control-name mask (XkbMouseKeysMask=0x10).
- `flags` (XkbIM_*, NOT serialized by xkbcommon — by indicator name):
  Caps/Num/Shift Lock/Group 2 = `0x80` (NoExplicit); Scroll Lock = `0x00`;
  Mouse Keys = `0x20` (LEDDrivesKB).

## CompatMap golden vector (`us,de`) — DEFERRED follow-up

Header: `groupsRtrn=0x0f firstSIRtrn=0 nSIRtrn=124 nTotalSI=124`. Body =
124 × `xkbSymInterpretWireDesc` (16B: sym(4), mods(1), match(1), virtualMod(1),
flags(1), act{type(1),data[7]}) then group-compat: one `xkbModsWireDesc`
(mask(1),realMods(1),vmods(2)) per set bit in `groupsRtrn` (4 groups → 16B).

Action-type histogram (libxkbcommon's `xkb_keymap_get_as_string` for `us,de`
emits the SAME set — both compilers read `/usr/share/X11/xkb/compat/complete`):
```
SetMods 23  MovePtr 20  PtrBtn 13  LockControls 13  SwitchScreen 12  LockMods 9
LockPtrBtn 8  LatchMods 5  SetPtrDflt 10  LockGroup 4  Private 4  SetGroup 1
LatchGroup 1  Terminate 1
```
First 12 SIs (sym, mods, match, vmod, flags, action, data[7] hex):
```
0xfe02 0x01 132 0xff 0x00 LatchMods 03010100000000
0xffe6 0x03 2   0xff 0x00 LockMods  00010100000000
0xff7f 0xff 2   0x00 0x00 LockMods  00000000010000
0xfe03 0xff 130 0x02 0x00 SetMods   01000000040000   (ISO_Level3_Shift → set LevelThree vmod 0x04)
0xfe04 0xff 130 0x02 0x00 LatchMods 03000000040000
0xfe05 0xff 130 0x02 0x00 LockMods  00000000040000
0xffe9 0xff 2   0x01 0x00 SetMods   05000000000000   (Alt_L)
0xffea 0xff 2   0x01 0x00 SetMods   05000000000000   (Alt_R)
0xffe7 0xff 2   0x05 0x00 SetMods   05000000000000   (Meta_L)
0xffe8 0xff 2   0x05 0x00 SetMods   05000000000000   (Meta_R)
0xffeb 0xff 2   0x03 0x00 SetMods   05000000000000   (Super_L)
0xffec 0xff 2   0x03 0x00 SetMods   05000000000000   (Super_R)
```
group-compat trailing 16B: `00000000 80800000 80800000 80800000`
(group1 mods=0/0, groups 2–4 → mask=0x80 realMods=0x80 = Mod5).

Why deferred: 63 of 124 SIs are pointer/screen actions (MovePtr/PtrBtn/
SetPtrDflt/LockPtrBtn/SwitchScreen) from the mouse-keys/screen-switch interprets;
a faithful encoder needs all ~14 action types. The block is purely informational
in the reply — yserver's own key→action interpretation is done inside
libxkbcommon, and xkbcommon-x11 clients ignore server compat and recompile from
GetMap. Tracked as a clean pick-up: this golden vector + the text-parse path
(parse `interpret` actions → binary `xkbSymInterpretWireDesc`) are the full spec.
