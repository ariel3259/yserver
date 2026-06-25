# AltGr / 4-level XKB key types — golden vector (German, AltGr+e → €)

External golden vector for **Part C** of the XKB runtime-layout work
(`docs/superpowers/plans/2026-06-24-xkb-runtime-layout.md` Part C). Decoded from a
real-Xorg `xtrace` of a German keyboard pressing **AltGr+e** (`€` = keysym
`0x20ac`). The trace itself is gitignored/local; this is the durable record.
Cross-checked against `/usr/include/X11/extensions/XKBproto.h` (`xkbKeyTypeWireDesc`,
`xkbKTMapEntryWireDesc`, `xkbSymMapWireDesc`) and `xserver xkb/xkb.c:1413` (`XkbSendMap`).

## The gap Part C closes
yserver's multi-group `GetMap` (Phase 1a) already ships width-4 syms incl. `€` for
the `e` key — but it sets `ktIndex[group]` to a **2-level** type, so the client has
no modifier→level mapping to *reach* level 2/3. The fix: publish real `FOUR_LEVEL`
key types (with the `LevelThree`-vmod→level map entries) and assign them per-group.

## Ground truth (German keymap; the generic deviceID=9 map at trace line 4106 is NOT it — use the de map)

### `e` key (evdev keycode 26, AD03) KeySymMap
```
ktIndex = [2, 13, 2, 0]   groupInfo = 0x03 (3 groups)   width = 4   nSyms = 12
  group0 (type[2]  ALPHABETIC): 'e'(0x65) 'E'(0x45) 0 0
  group1 (type[13] FOUR_LEVEL): 'e'(0x65) 'E'(0x45) '€'(0x20ac) '€'(0x20ac)
  group2 (type[2]  ALPHABETIC): 'e'(0x65) 'E'(0x45) 0 0
```
€ is at **group 1, level 2**. The 4-level type rides in `ktIndex[1]`, NOT `ktIndex[0]`.

### `FOUR_LEVEL` key type used by `e` group 1 — `type[13]`
Wire `xkbKeyTypeWireDesc` = `mask:u8, realMods:u8, vmods:u16, numLevels:u8, nMapEntries:u8, preserve:u8, pad:u8`,
then `nMapEntries × {active:u8, mask:u8, level:u8, realMods:u8, vmods:u16, pad:u16}`,
then (preserve!=0) `nMapEntries × {mask:u8, realMods:u8, vmods:u16}`.
```
type[13]: mask=0x83 realMods=0x03 vmods=0x0004 numLevels=4 nMapEntries=6 preserve=1
  {realMods=0x01 vmods=0x0000} -> level 1   (Shift)
  {realMods=0x02 vmods=0x0000} -> level 1   (Lock)
  {realMods=0x00 vmods=0x0004} -> level 2   (LevelThree)            <-- the € path
  {realMods=0x01 vmods=0x0004} -> level 3   (Shift+LevelThree)
  {realMods=0x02 vmods=0x0004} -> level 2   (Lock+LevelThree)
  {realMods=0x03 vmods=0x0004} -> level 3   (Shift+Lock+LevelThree)
  preserve: entries 4&5 preserve Lock (0x02)
```
Plainer symbol-key FOUR_LEVEL (no Caps interaction) — `type[11]`:
```
type[11]: vmods=0x0004 numLevels=4 nMapEntries=3 preserve=0
  {realMods=0x01 vmods=0x0000} -> 1   {vmods=0x0004} -> 2   {realMods=0x01 vmods=0x0004} -> 3
```
NB: the high `0x80` bit seen in some `mask` fields is the vmod-mapped real bit
(`Mod5`), not part of `realMods`; the *binding* is carried by `realMods`+`vmods`.

### LevelThree (AltGr) modifier binding
- **ModifierMap:** keycode 92 (`ISO_Level3_Shift`, keysym `0xfe03`) → real **Mod5 (0x80)**.
- **VirtualModMap:** keycode 92 → vmods **0x0004** (LevelThree).
- So **real Mod5 (0x80) ⟷ virtual LevelThree (0x0004)**. (Full modmap: Shift=kc50/62→0x01, Lock=kc66→0x02, Control=kc37→0x04, Mod1=kc64→0x08, Mod2=kc77→0x10, Mod5=kc92→0x80.)

### AltGr+e KeyPress (XI2) the client actually received
```
KeyPress detail=0x1a (kc26): effective_mods=0x80 (Mod5)  effective_group=1
```

### Resolution chain to replicate / verify `key_get_mods_for_level` against
```
AltGr+e: effective_mods=0x80 (Mod5), group=1
 → group1 type = ktIndex[1] = type[13]
 → Mod5 ⟷ vmod LevelThree 0x0004
 → type[13] entry {realMods=0x00, vmods=0x0004} → level 2
 → group1 syms[level 2] = 0x20ac  ✓ €
```

### Type table size
`nTypes = 27` in the de keymap. The 4-level class is type[11..18]. Letters use a
FOUR_LEVEL(+alphabetic/semialphabetic) variant in their AltGr group; symbol keys use
the plain FOUR_LEVEL. Type-name atoms are present in GetNames but resolve to
pre-interned server atoms with no InternAtom round-trip in the capture, so the type
NAMES couldn't be byte-confirmed — only the structure (above) is the load-bearing vector.

## Implementation note for C1/C2
xkbcommon `key_get_mods_for_level(kc, group, level)` returns the real-mod mask(s) that
select each level — build the `FOUR_LEVEL` map entries from that, with `LevelThree`
expressed as vmod 0x0004 bound to Mod5 in the published VModMap. Verify the generated
type[13]-equivalent against the entries above. Assign `ktIndex[group]` per group by that
group's level count + mod structure (≥3 levels ⇒ a FOUR_LEVEL variant).
