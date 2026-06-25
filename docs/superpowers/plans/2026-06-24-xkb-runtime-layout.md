# XKB Runtime Keyboard-Layout Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make keyboard-layout changes actually take effect — the desktop-driven runtime switch (Cinnamon / libxklavier), the startup default, AltGr/4-level layouts (be/fr/de/us-intl), and multi-group live-switching (us,ru with a switch hotkey) — instead of being permanently locked to single-group `us`.

**Architecture:** yserver is built on libxkbcommon; it has no Xorg-style `XkbDescRec` to mutate from the `XkbSetMap` wire upload, so we do not parse that upload. Instead:
- **(A)** Recompile the keymap from RMLVO via `xkbcommon::Keymap::new_from_names` whenever the `_XKB_RULES_NAMES` root-window property is written (which both libxklavier and `setxkbmap` write when applying a layout), swap the live `KmsCore.xkb_keymap`/`xkb_state`, and emit the events Xorg emits (core `MappingNotify` to all clients + `XkbNewKeyboardNotify`/`XkbMapNotify` to XKB-subscribed clients) so running clients re-query `GetMap`/`GetNames`.
- **(B)** Resolve the startup RMLVO from `XKB_DEFAULT_*` env vars and the `-layout` argument instead of hardcoding `us`.
- **(C)** Publish proper key **types** in `GetMap` (`FOUR_LEVEL` etc. with the `LevelThree`/AltGr modifier→level mapping) so AltGr symbols resolve — required for `be`/`fr`/`de` even as a single layout.
- **(D)** Serialize **all groups** in `GetMap` (today only group 0), track the active group in `xkb_state`, stamp the group into key-event `state` bits, emit `XkbStateNotify` on group change, and honor `XkbLatchLockState` — so multi-layout switching works.

**This is the honest scope, not "full XKB."** The property hook is a **compatibility shim** for the RMLVO path used by libxklavier/`setxkbmap`/GNOME-stack tools — it is NOT general `XkbSetMap` support. A client that uploads a custom compiled keymap via `xkbcomp - $DISPLAY`, or changes the map without writing `_XKB_RULES_NAMES`, is still unsupported (documented limitation; `backend.rs:16722` still drops those minors). For the desktop layout-switching workflow the user reported, the RMLVO path is what fires.

**Tech Stack:** Rust, `xkbcommon` crate 0.9, the yserver `Backend` trait, `fanout_event_to_clients`, X11/XKB wire protocol.

**External-vectors rule (load-bearing):** Parts C and D touch fiddly `GetMap` wire layout (key-type modifier maps, multi-group `KeySymMap`). Per `feedback_test_vectors_must_be_external`, their expected bytes are NOT to be derived from my arithmetic — they are locked from a **captured real Xorg `GetMap` reply** for the target layouts (`be`, `us,ru`), obtained via `just xts-xorg-trace`-style capture. Task C0/D0 capture those vectors before any C/D code is written.

**Reference (canonical wire layouts, used as external test vectors):**
- `/usr/include/X11/extensions/XKBproto.h` — `xkbNewKeyboardNotify` (1028-1048), `xkbMapNotify` (1050-1077)
- `/usr/include/X11/extensions/XKB.h` — event codes: `XkbNewKeyboardNotify=0`, `XkbMapNotify=1`; masks: `XkbNewKeyboardNotifyMask=0x01`, `XkbMapNotifyMask=0x02`; `XkbNKN_KeycodesMask=0x01`
- `/home/jos/Projects/xserver/xkb/xkbEvents.c` — `XkbSendNewKeyboardNotify` (161), `XkbSendMapNotify` (265), `XkbSendLegacyMapNotify` (55)
- All X11 events are exactly 32 bytes. For XKB events byte 0 = `xkb_event_base` (85 for the KMS backend, from `xkb_info()`), byte 1 = `xkbType` (the minor code 0/1).

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `crates/yserver/src/kms/core.rs` | `KmsCore` keymap state | Add `xkb_rmlvo: XkbRmlvo` field + `XkbRmlvo` type; add `recompile_keymap`; replace hardcoded startup RMLVO with `resolve_startup_rmlvo` |
| `crates/yserver/src/kms/xkb.rs` | XKB replies | `reply_get_names` sources `symbolsName` from the active RMLVO |
| `crates/yserver-protocol/src/x11/mod.rs` | event encoders | Add `write_xkb_new_keyboard_notify`, `write_xkb_map_notify` |
| `crates/yserver-core/src/backend/trait_def.rs` | `Backend` trait | Add `set_keymap_rmlvo` (default no-op) |
| `crates/yserver/src/kms/v2/backend.rs` | KMS backend impl | Implement `set_keymap_rmlvo` delegating to `KmsCore::recompile_keymap`; thread `-layout` into `open`/`open_libseat` |
| `crates/yserver-core/src/core_loop/process_request.rs` | ChangeProperty + XKB handlers | Hook `_XKB_RULES_NAMES`; (D) `XkbLatchLockState` route |
| `crates/yserver/src/launch.rs` | argv parsing | Parse `-layout` into `LaunchOptions.layout` |
| `crates/yserver/src/lib.rs` | run path | Thread `opts.layout` to `build_kms_backend_v2` |
| `crates/yserver/src/kms/xkb.rs` (C/D) | `GetMap` | Derive real key types (`key_types_from_keymap`); serialize all groups |
| `crates/yserver-protocol/src/x11/mod.rs` (D) | event encoders | Add `write_xkb_state_notify` |
| `crates/yserver-core/src/core_loop/key_fanout.rs` (D) | key path | Detect group change → `XkbStateNotify` |
| `crates/yserver/tests/fixtures/getmap-{be,us-ru}.bin` (C/D) | golden vectors | Captured Xorg `GetMap` replies (external test vectors) |

---

# PART A — Runtime layout change (the reported bug)

### Task A1: `XkbRmlvo` type + keymap recompile on `KmsCore`

**Files:**
- Modify: `crates/yserver/src/kms/core.rs` (add type near the `XkbKeymap` wrappers ~line 58-75; add field to `KmsCore` ~line 1537; add method on the `impl KmsCore`; set field in `new` ~1670 and `for_tests` ~1732)

- [ ] **Step 1: Write the failing test**

Add to the existing test module in `crates/yserver/src/kms/core.rs` (or create one with `#[cfg(test)] mod tests { use super::*; ... }` if none exists in this file):

```rust
#[test]
fn recompile_keymap_changes_keysym() {
    // External ground truth: on a US layout, AE01 (keycode 10) level-0
    // is `1`; on the German (de) layout the same physical key is still
    // `1` but AD01..AD10 differ. Use a key that genuinely differs: the
    // physical Y key (keycode 29, "AD06") is `y` on us, `z` on de.
    let mut core = KmsCore::for_tests();
    let us_sym = core
        .xkb_state
        .0
        .key_get_one_sym(xkbcommon::xkb::Keycode::new(29));
    assert_eq!(us_sym, xkbcommon::xkb::keysyms::KEY_y.into());

    let changed = core.recompile_keymap(&XkbRmlvo {
        rules: "evdev".into(),
        model: "pc105".into(),
        layout: "de".into(),
        variant: String::new(),
        options: None,
    });
    assert!(changed.is_some(), "de keymap must compile");

    let de_sym = core
        .xkb_state
        .0
        .key_get_one_sym(xkbcommon::xkb::Keycode::new(29));
    assert_eq!(de_sym, xkbcommon::xkb::keysyms::KEY_z.into());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver --locked recompile_keymap_changes_keysym 2>&1 | tail -20`
Expected: FAIL — `XkbRmlvo` / `recompile_keymap` not found (compile error).

- [ ] **Step 3: Add the `XkbRmlvo` type**

Near the `XkbContext`/`XkbKeymap`/`XkbState` wrappers (`core.rs:58-75`):

```rust
/// The resolved-component keyboard layout (rules/model/layout/variant/
/// options) the server currently has compiled. Stored so GetNames can
/// report an honest `symbolsName` and so a layout change can be diffed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XkbRmlvo {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: Option<String>,
}

impl Default for XkbRmlvo {
    fn default() -> Self {
        Self {
            rules: "evdev".into(),
            model: "pc105".into(),
            layout: "us".into(),
            variant: String::new(),
            options: None,
        }
    }
}
```

- [ ] **Step 4: Add the `xkb_rmlvo` field to `KmsCore`**

In the `KmsCore` struct, next to `xkb_keymap`/`xkb_state` (`core.rs:1537-1539`):

```rust
    pub(crate) xkb_rmlvo: XkbRmlvo,
```

Initialize it in both `new` (after the keymap is built, ~line 1671) and `for_tests` (~1733) with `xkb_rmlvo: XkbRmlvo::default(),`. (In `new`, Part B Task B1 will replace `default()` with the resolved value.)

- [ ] **Step 5: Add `recompile_keymap`**

On `impl KmsCore` (place near the existing keymap construction code):

```rust
/// Recompile the keyboard map from a new RMLVO and swap it in.
///
/// Returns `Some((min_keycode, max_keycode))` of the new map on a
/// successful change, or `None` if compilation failed (the old map is
/// kept) or the RMLVO is byte-identical to the active one (no-op).
///
/// A fresh `xkb_state` is built, then every physically-held key in
/// `down_keys` is re-applied to it. This matters because the layout
/// switch is typically triggered by a *hotkey* (Super+Space, etc.) that
/// is still held at swap time — without re-applying, the new state's
/// modifier/group tracking diverges from the physical keyboard and the
/// next keys are stamped with stale modifiers (the stuck-modifier class
/// already seen on the VT path, `backend.rs:5267`).
///
/// LIMITATION (deliberate): *locked* state (Caps Lock, a locked group)
/// is reset by the rebuild — only physically-held keys are re-asserted.
/// We do NOT restore locked mods/group via `update_mask`: xkbcommon
/// documents `update_mask` as the lossy slave/wire entry point that
/// "must not be used to update the master state" and "should not be used
/// together" with `update_key` (`xkbcommon-0.9.0 src/xkb/mod.rs:1303-1319`).
/// Mixing it in would risk an incoherent master state for subsequent real
/// key events — worse than dropping a Caps-lock across a manual layout
/// switch. This matches the VT-acquire path's fresh-state behavior.
pub(crate) fn recompile_keymap(&mut self, rmlvo: &XkbRmlvo) -> Option<(u8, u8)> {
    if *rmlvo == self.xkb_rmlvo {
        return None;
    }
    let keymap = xkbcommon::xkb::Keymap::new_from_names(
        &self.xkb_context.0,
        &rmlvo.rules,
        &rmlvo.model,
        &rmlvo.layout,
        &rmlvo.variant,
        rmlvo.options.clone(),
        xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
    )?;
    let min_kc = u8::try_from(keymap.min_keycode().raw()).unwrap_or(8).max(8);
    let max_kc = u8::try_from(keymap.max_keycode().raw().min(255))
        .unwrap_or(255)
        .max(min_kc);
    let mut new_state = xkbcommon::xkb::State::new(&keymap);
    // Re-assert currently-held keys against the fresh state.
    for kc in &self.down_keys {
        new_state.update_key(
            xkbcommon::xkb::Keycode::new(u32::from(*kc)),
            xkbcommon::xkb::KeyDirection::Down,
        );
    }
    self.xkb_state = XkbState(new_state);
    self.xkb_keymap = XkbKeymap(keymap);
    self.xkb_rmlvo = rmlvo.clone();
    log::info!(
        "xkb: recompiled keymap -> rules={} model={} layout={} variant={:?} options={:?}",
        rmlvo.rules, rmlvo.model, rmlvo.layout, rmlvo.variant, rmlvo.options
    );
    Some((min_kc, max_kc))
}
```

> **Add a second test** in this file proving the held-key reconcile: seed `core.down_keys.insert(<Shift keycode>)`, recompile to `de`, then assert `core.xkb_state.0.mod_name_is_active("Shift", STATE_MODS_EFFECTIVE)` is true — the held Shift survives the swap.

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p yserver --locked recompile_keymap_changes_keysym 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/yserver/src/kms/core.rs
git commit -m "feat(xkb): add XkbRmlvo + KmsCore::recompile_keymap"
```

---

### Task A2: `reply_get_names` reports a layout-derived `symbolsName`

**Files:**
- Modify: `crates/yserver/src/kms/xkb.rs` (`reply_get_names` signature + body ~548-660)
- Modify: `crates/yserver/src/kms/v2/backend.rs` (call site in `xkb_proxy` ~16715)

- [ ] **Step 1: Write the failing test**

In `crates/yserver/src/kms/xkb.rs` test module (`#[cfg(test)] mod tests` at line 799):

```rust
#[test]
fn get_names_symbols_reflects_layout() {
    let core = KmsCore::for_tests();
    // Recompile to "de" so the stored RMLVO is German.
    let mut core = core;
    core.recompile_keymap(&crate::kms::core::XkbRmlvo {
        rules: "evdev".into(),
        model: "pc105".into(),
        layout: "de".into(),
        variant: String::new(),
        options: None,
    });
    // Capture the strings GetNames interns.
    let mut interned: Vec<String> = Vec::new();
    let _ = reply_get_names(&core.xkb_keymap.0, &core.xkb_rmlvo, &mut |s| {
        interned.push(s.to_string());
        1
    });
    assert!(
        interned.iter().any(|s| s == "pc+de+inet(evdev)"),
        "symbolsName must reflect the active layout, got {interned:?}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver --locked get_names_symbols_reflects_layout 2>&1 | tail -20`
Expected: FAIL — `reply_get_names` takes 2 args, not 3 (compile error).

- [ ] **Step 3: Add an RMLVO param + derive `symbolsName`**

Change the signature (`xkb.rs:548`):

```rust
pub(super) fn reply_get_names(
    keymap: &Keymap,
    rmlvo: &crate::kms::core::XkbRmlvo,
    intern_atom: &mut dyn FnMut(&str) -> u32,
) -> Vec<u8> {
```

Replace the hardcoded 4-name loop (`xkb.rs:651-660`). Build `symbolsName` from the active RMLVO. **Caveat (codex round-4): this is a best-effort approximation of the KcCGST `symbols` string, not the exact string `xkbcomp` would resolve** — it omits `options` and uses a simplified per-group join for a multi-layout RMLVO (`us,ru`). It is informational metadata (clients get real keysyms from `GetMap`), so the approximation is acceptable; do not describe it as authoritative. Handle the multi-layout case by joining each layout (with its variant) so the string isn't silently truncated to the first layout:

```rust
    // Per-layout segment: "<layout>" or "<layout>(<variant>)".
    let layouts: Vec<&str> = rmlvo.layout.split(',').collect();
    let variants: Vec<&str> = rmlvo.variant.split(',').collect();
    let mut segs = Vec::with_capacity(layouts.len());
    for (i, l) in layouts.iter().enumerate() {
        match variants.get(i) {
            Some(v) if !v.is_empty() => segs.push(format!("{l}({v})")),
            _ => segs.push((*l).to_string()),
        }
    }
    // Approximate KcCGST symbols string; informational only.
    let symbols_name = format!("pc+{}+inet(evdev)", segs.join("+"));
    let names: [&str; 4] = [
        "evdev+aliases(qwerty)", // keycodesName
        &symbols_name,           // symbolsName — derived from active RMLVO
        "complete",              // typesName
        "complete",              // compatName
    ];
    for name in names {
        let atom = intern_atom(name);
        r[off..off + 4].copy_from_slice(&atom.to_le_bytes());
        off += 4;
    }
```

- [ ] **Step 4: Update the `xkb_proxy` call site**

In `crates/yserver/src/kms/v2/backend.rs:16715`:

```rust
            17 => Some(xkb_replies::reply_get_names(
                &self.core.xkb_keymap.0,
                &self.core.xkb_rmlvo,
                intern_atom,
            )),
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p yserver --locked get_names_symbols_reflects_layout 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/xkb.rs crates/yserver/src/kms/v2/backend.rs
git commit -m "feat(xkb): GetNames reports symbolsName from active RMLVO"
```

---

### Task A3: XKB event encoders (`XkbNewKeyboardNotify`, `XkbMapNotify`)

**Files:**
- Modify: `crates/yserver-protocol/src/x11/mod.rs` (add two encoders near `write_mapping_notify_event` ~3450)
- Test: same file's test module (or add `#[cfg(test)] mod tests` if absent — grep for an existing `mod tests` in this file first and reuse it)

- [ ] **Step 1: Write the failing test**

Test vectors derive directly from `XKBproto.h` field offsets (external source). Use `xkb_event_base=85`, `xkbType=0`/`1`:

```rust
#[test]
fn xkb_new_keyboard_notify_wire_layout() {
    let mut buf = Vec::new();
    write_xkb_new_keyboard_notify(
        &mut buf,
        ClientByteOrder::LittleEndian,
        SequenceNumber(0x1234),
        85,   // xkb_event_base
        1,    // device_id
        8,    // min_keycode (new)
        255,  // max_keycode (new)
        8,    // old_min_keycode
        255,  // old_max_keycode
    )
    .unwrap();
    assert_eq!(buf.len(), 32);
    assert_eq!(buf[0], 85, "type = xkb_event_base + XkbEventCode(0)");
    assert_eq!(buf[1], 0, "xkbType = XkbNewKeyboardNotify");
    assert_eq!(&buf[2..4], &0x1234u16.to_le_bytes(), "sequenceNumber @2");
    assert_eq!(buf[8], 1, "deviceID @8");
    assert_eq!(buf[9], 1, "oldDeviceID @9");
    assert_eq!(buf[10], 8, "minKeyCode @10");
    assert_eq!(buf[11], 255, "maxKeyCode @11");
    assert_eq!(buf[12], 8, "oldMinKeyCode @12");
    assert_eq!(buf[13], 255, "oldMaxKeyCode @13");
    assert_eq!(&buf[16..18], &1u16.to_le_bytes(), "changed = XkbNKN_KeycodesMask @16");
}

#[test]
fn xkb_map_notify_wire_layout() {
    let mut buf = Vec::new();
    write_xkb_map_notify(
        &mut buf,
        ClientByteOrder::LittleEndian,
        SequenceNumber(0x1234),
        85, // xkb_event_base
        1,  // device_id
        8,  // min_keycode
        255, // max_keycode
        4,  // n_types (phase A fixed table)
    )
    .unwrap();
    assert_eq!(buf.len(), 32);
    assert_eq!(buf[0], 85);
    assert_eq!(buf[1], 1, "xkbType = XkbMapNotify");
    assert_eq!(&buf[2..4], &0x1234u16.to_le_bytes(), "sequenceNumber @2");
    assert_eq!(buf[8], 1, "deviceID @8");
    // changed mask @10 = KeyTypes(0x01)|KeySyms(0x02)|ModifierMap(0x04)
    // = 0x07. We do NOT claim VirtualMods(0x40): the vmod *bindings*
    // (Super/Alt/...) are layout-independent, so they don't change
    // us<->de, and the `virtualMods` field stays 0 — advertising a
    // change we leave undescribed is what codex flagged.
    assert_eq!(&buf[10..12], &0x0007u16.to_le_bytes(), "changed @10");
    assert_eq!(buf[12], 8, "minKeyCode @12");
    assert_eq!(buf[13], 255, "maxKeyCode @13");
    // Fields must be consistent with the claimed `changed`:
    assert_eq!(buf[15], 4, "nTypes @15 (KeyTypes claimed)");
    assert_eq!(buf[16], 8, "firstKeySym @16 (KeySyms claimed)");
    assert_eq!(buf[17], 248, "nKeySyms @17 = 255-8+1");
    assert_eq!(buf[24], 8, "firstModMapKey @24 (ModifierMap claimed)");
    assert_eq!(buf[25], 248, "nModMapKeys @25");
    assert_eq!(&buf[28..30], &0u16.to_le_bytes(), "virtualMods @28 = 0 (not claimed)");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver-protocol --locked xkb_ 2>&1 | tail -20`
Expected: FAIL — encoders not defined.

- [ ] **Step 3: Implement the encoders**

After `write_mapping_notify_event` (`x11/mod.rs:3467`):

```rust
/// `XkbNewKeyboardNotify` (xkbType=0). Tells clients the keyboard map
/// may have changed and they must re-query GetMap/GetNames. Layout per
/// XKBproto.h `xkbNewKeyboardNotify` (1028-1048). `changed` is fixed at
/// `XkbNKN_KeycodesMask` (0x1). NB: our keycode *range* doesn't actually
/// change across layouts (evdev keeps 8..255) — we send this to mirror
/// Xorg's `GetKbdByName` full-reload path (xkb.c:6291), which sets
/// KeycodesMask regardless of whether the range changed; xkbcommon-x11
/// clients treat it as "rebuild the keymap from the device".
#[allow(clippy::too_many_arguments)]
pub fn write_xkb_new_keyboard_notify(
    writer: &mut impl Write,
    byte_order: ClientByteOrder,
    sequence: SequenceNumber,
    xkb_event_base: u8,
    device_id: u8,
    min_keycode: u8,
    max_keycode: u8,
    old_min_keycode: u8,
    old_max_keycode: u8,
) -> io::Result<()> {
    let mut buf = [0u8; 32];
    buf[0] = xkb_event_base; // type = base + XkbEventCode(0)
    buf[1] = 0; // xkbType = XkbNewKeyboardNotify
    let mut seq_buf = Vec::with_capacity(2);
    write_u16(byte_order, &mut seq_buf, sequence.0);
    buf[2..4].copy_from_slice(&seq_buf);
    // buf[4..8] time = 0
    buf[8] = device_id;
    buf[9] = device_id; // oldDeviceID — same device
    buf[10] = min_keycode;
    buf[11] = max_keycode;
    buf[12] = old_min_keycode;
    buf[13] = old_max_keycode;
    // buf[14] requestMajor = 0, buf[15] requestMinor = 0 (no XKB request)
    let mut changed_buf = Vec::with_capacity(2);
    write_u16(byte_order, &mut changed_buf, 0x0001); // XkbNKN_KeycodesMask
    buf[16..18].copy_from_slice(&changed_buf);
    writer.write_all(&buf)
}

/// `XkbMapNotify` (xkbType=1). Layout per XKBproto.h `xkbMapNotify`
/// (1050-1077). `changed` = KeyTypes|KeySyms|ModifierMap = 0x07; the
/// per-component first/count ranges below cover the full keymap. We do
/// not claim VirtualMods (layout-independent) and leave `virtualMods`
/// zero, so every advertised bit has matching populated fields.
pub fn write_xkb_map_notify(
    writer: &mut impl Write,
    byte_order: ClientByteOrder,
    sequence: SequenceNumber,
    xkb_event_base: u8,
    device_id: u8,
    min_keycode: u8,
    max_keycode: u8,
    n_types: u8, // current GetMap type count (4 in phase A; derived after C2)
) -> io::Result<()> {
    let mut buf = [0u8; 32];
    buf[0] = xkb_event_base;
    buf[1] = 1; // xkbType = XkbMapNotify
    let mut seq_buf = Vec::with_capacity(2);
    write_u16(byte_order, &mut seq_buf, sequence.0);
    buf[2..4].copy_from_slice(&seq_buf);
    // buf[4..8] time = 0
    buf[8] = device_id;
    // buf[9] ptrBtnActions = 0
    let mut changed_buf = Vec::with_capacity(2);
    write_u16(byte_order, &mut changed_buf, 0x0007); // KeyTypes|KeySyms|ModifierMap
    buf[10..12].copy_from_slice(&changed_buf);
    buf[12] = min_keycode;
    buf[13] = max_keycode;
    // firstType=0, nTypes — MUST match GetMap's published type count.
    // Pass it in (param `n_types`); for phase A the table is the fixed
    // 4 (ONE/TWO/ALPHABETIC/KEYPAD), but C2 makes GetMap publish the
    // derived count, so this is a parameter, NOT a hardcoded 4 (codex
    // round-5: a hardcoded 4 goes stale once C lands).
    buf[14] = 0;
    buf[15] = n_types;
    buf[16] = min_keycode; // firstKeySym
    buf[17] = max_keycode.saturating_sub(min_keycode).saturating_add(1); // nKeySyms
    // firstModMapKey/nModMapKeys cover the full range too
    buf[24] = min_keycode;
    buf[25] = max_keycode.saturating_sub(min_keycode).saturating_add(1);
    writer.write_all(&buf)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p yserver-protocol --locked xkb_ 2>&1 | tail -20`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/yserver-protocol/src/x11/mod.rs
git commit -m "feat(xkb): add XkbNewKeyboardNotify + XkbMapNotify wire encoders"
```

---

### Task A4: `Backend::set_keymap_rmlvo` trait method + KMS impl

**Files:**
- Modify: `crates/yserver-core/src/backend/trait_def.rs` (add method near `on_window_property_changed` ~554)
- Modify: `crates/yserver/src/kms/v2/backend.rs` (impl on `KmsBackendV2`)

- [ ] **Step 1: Write the failing test**

In the `crates/yserver/src/kms/v2/backend.rs` test module (uses `KmsBackendV2::for_tests()`, see existing tests ~17953):

```rust
#[test]
fn backend_set_keymap_rmlvo_reports_range() {
    let mut backend = KmsBackendV2::for_tests();
    let range = backend.set_keymap_rmlvo("evdev", "pc105", "de", "", None);
    assert_eq!(range, Some((8, 255)), "de map compiles, evdev keycode range");
    // Re-applying the same RMLVO is a no-op (None = no change).
    let again = backend.set_keymap_rmlvo("evdev", "pc105", "de", "", None);
    assert_eq!(again, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver --locked backend_set_keymap_rmlvo_reports_range 2>&1 | tail -20`
Expected: FAIL — method not found.

- [ ] **Step 3: Add the trait method (default no-op)**

In `crates/yserver-core/src/backend/trait_def.rs`, after `on_window_property_changed` (~564):

```rust
/// Recompile the keyboard map from RMLVO names and swap it in.
/// Returns `Some((min_keycode, max_keycode))` of the new map on a
/// successful change, `None` if compilation failed or the RMLVO was
/// unchanged. Backends without a real keymap return `None`.
fn set_keymap_rmlvo(
    &mut self,
    _rules: &str,
    _model: &str,
    _layout: &str,
    _variant: &str,
    _options: Option<&str>,
) -> Option<(u8, u8)> {
    None
}
```

- [ ] **Step 4: Implement on `KmsBackendV2`**

In `crates/yserver/src/kms/v2/backend.rs`, in the `impl Backend for KmsBackendV2` block (near `xkb_proxy`):

```rust
fn set_keymap_rmlvo(
    &mut self,
    rules: &str,
    model: &str,
    layout: &str,
    variant: &str,
    options: Option<&str>,
) -> Option<(u8, u8)> {
    self.core.recompile_keymap(&crate::kms::core::XkbRmlvo {
        rules: rules.to_string(),
        model: model.to_string(),
        layout: layout.to_string(),
        variant: variant.to_string(),
        options: options.map(str::to_string),
    })
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p yserver --locked backend_set_keymap_rmlvo_reports_range 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver-core/src/backend/trait_def.rs crates/yserver/src/kms/v2/backend.rs
git commit -m "feat(xkb): Backend::set_keymap_rmlvo + KMS impl"
```

---

### Task A5: `_XKB_RULES_NAMES` property hook → recompile + fan out events

**Files:**
- Create: `crates/yserver-core/src/core_loop/xkb_layout.rs` (RMLVO parse helper + event fan-out)
- Modify: `crates/yserver-core/src/core_loop/mod.rs` (add `mod xkb_layout;` — confirm module declaration style in this file)
- Modify: `crates/yserver-core/src/core_loop/process_request.rs` (call the hook in the ChangeProperty handler ~22843, after the property is stored)

- [ ] **Step 1: Write the failing test (pure RMLVO parser)**

Create `crates/yserver-core/src/core_loop/xkb_layout.rs` with only the test first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rules_names_full() {
        // _XKB_RULES_NAMES is NUL-separated: rules,model,layout,variant,options
        let bytes = b"evdev\0pc105\0be\0\0\0";
        let r = parse_rules_names(bytes).expect("parses");
        assert_eq!(r.rules, "evdev");
        assert_eq!(r.model, "pc105");
        assert_eq!(r.layout, "be");
        assert_eq!(r.variant, "");
        assert_eq!(r.options, None);
    }

    #[test]
    fn parse_rules_names_with_variant_and_options() {
        let bytes = b"evdev\0pc105\0us\0intl\0ctrl:nocaps\0";
        let r = parse_rules_names(bytes).expect("parses");
        assert_eq!(r.layout, "us");
        assert_eq!(r.variant, "intl");
        assert_eq!(r.options.as_deref(), Some("ctrl:nocaps"));
    }

    #[test]
    fn parse_rules_names_too_few_fields_is_none() {
        assert!(parse_rules_names(b"evdev\0pc105\0").is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver-core --locked parse_rules_names 2>&1 | tail -20`
Expected: FAIL — module/function not found (add `mod xkb_layout;` to `core_loop/mod.rs` first; the test still fails on the missing `parse_rules_names`).

- [ ] **Step 3: Implement the parser + a plain RMLVO struct**

Top of `crates/yserver-core/src/core_loop/xkb_layout.rs` (this crate cannot see `yserver`'s `XkbRmlvo`, so use a local plain struct of owned strings that the backend call destructures):

```rust
use crate::core_loop::fanout::fanout_event_to_clients;
use crate::server::ServerState;
use crate::backend::Backend;
use yserver_protocol::{ClientByteOrder, SequenceNumber};
use yserver_protocol::x11;

/// Parsed `_XKB_RULES_NAMES` property value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesNames {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: Option<String>,
}

/// Parse the NUL-separated `_XKB_RULES_NAMES` value
/// (`rules\0model\0layout\0variant\0options`). Returns `None` if fewer
/// than the 3 load-bearing fields (rules, model, layout) are present.
pub fn parse_rules_names(bytes: &[u8]) -> Option<RulesNames> {
    let s = std::str::from_utf8(bytes).ok()?;
    let mut it = s.split('\0');
    let rules = it.next()?.to_string();
    let model = it.next()?.to_string();
    let layout = it.next()?.to_string();
    if layout.is_empty() {
        return None;
    }
    let variant = it.next().unwrap_or("").to_string();
    let options = match it.next() {
        Some(o) if !o.is_empty() => Some(o.to_string()),
        _ => None,
    };
    Some(RulesNames { rules, model, layout, variant, options })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p yserver-core --locked parse_rules_names 2>&1 | tail -20`
Expected: PASS (all three).

- [ ] **Step 5: Add the recompile-and-notify entry point**

Append to `xkb_layout.rs`. This is called from the ChangeProperty handler with `&mut state` and `&mut backend`:

```rust
/// Apply a `_XKB_RULES_NAMES` change: recompile the keymap in the
/// backend, then notify clients (core MappingNotify to all; XKB
/// New-Keyboard / Map notify to subscribed clients) so already-running
/// clients re-query the new layout. No-op if the RMLVO is unchanged or
/// fails to compile.
pub fn apply_rules_names_change(
    state: &mut ServerState,
    backend: &mut dyn Backend,
    value: &[u8],
) {
    let Some(names) = parse_rules_names(value) else {
        return;
    };
    let Some((min_kc, max_kc)) = backend.set_keymap_rmlvo(
        &names.rules,
        &names.model,
        &names.layout,
        &names.variant,
        names.options.as_deref(),
    ) else {
        return; // unchanged or failed to compile
    };
    let xkb_event_base = backend.xkb_info().map_or(0, |(_maj, ev, _err)| ev);
    let count = max_kc.saturating_sub(min_kc).saturating_add(1);

    // 1. Core MappingNotify(Keyboard) + MappingNotify(Modifier) to ALL
    //    clients — what Xorg's XkbSendLegacyMapNotify emits on a keymap
    //    reload (xkbEvents.c:55). Drives XRefreshKeyboardMapping in
    //    Xlib/non-XKB clients.
    let all: Vec<crate::server::ClientId> =
        state.clients.keys().map(|id| crate::server::ClientId(*id)).collect();
    let _ = fanout_event_to_clients(state, &all, |buf, seq, order| {
        let _ = x11::write_mapping_notify_event(buf, order, seq, 1, min_kc, count);
    });
    let _ = fanout_event_to_clients(state, &all, |buf, seq, order| {
        let _ = x11::write_mapping_notify_event(buf, order, seq, 0, 0, 0);
    });

    // 2. XKB events only to clients that selected them (XkbSelectEvents).
    let nkn: Vec<crate::server::ClientId> = subscribers(state, 0x0001); // XkbNewKeyboardNotifyMask
    let _ = fanout_event_to_clients(state, &nkn, |buf, seq, order| {
        let _ = x11::write_xkb_new_keyboard_notify(
            buf, order, seq, xkb_event_base, 1, min_kc, max_kc, min_kc, max_kc,
        );
    });
    let mapn: Vec<crate::server::ClientId> = subscribers(state, 0x0002); // XkbMapNotifyMask
    let _ = fanout_event_to_clients(state, &mapn, |buf, seq, order| {
        // n_types = 4 in phase A; C2 changes this to the backend's derived
        // type count once GetMap publishes the real table (codex round-5).
        let _ = x11::write_xkb_map_notify(buf, order, seq, xkb_event_base, 1, min_kc, max_kc, 4);
    });
    log::info!(
        "xkb: applied layout '{}' (variant '{}'); notified {} clients",
        names.layout, names.variant, all.len()
    );
}

/// Clients whose XkbSelectEvents top mask (any device-spec) includes `bit`.
/// NOTE: in phase A the stored value is a `u16`; D2b changes it to the
/// `XkbEventMasks` struct, after which this reads `m.top & bit` (and D3
/// filters StateNotify separately on `m.state_detail`). Adjust the field
/// access when D2b lands.
fn subscribers(state: &ServerState, bit: u16) -> Vec<crate::server::ClientId> {
    let mut out: Vec<crate::server::ClientId> = state
        .xkb_select_event_masks
        .iter()
        .filter(|((_cid, _dev), mask)| **mask & bit != 0) // becomes `mask.top & bit` after D2b
        .map(|((cid, _dev), _)| crate::server::ClientId(*cid))
        .collect();
    out.sort_by_key(|c| c.0);
    out.dedup_by_key(|c| c.0);
    out
}
```

> Note: confirm the exact import paths for `ClientId`, `fanout_event_to_clients`, `x11`, `ClientByteOrder`, `SequenceNumber` against the crate (the gathered references show `fanout_event_to_clients` in `core_loop::fanout`, `ClientId` and `xkb_select_event_masks` in `server`, and `write_mapping_notify_event` under the protocol crate's `x11`). Adjust `use` lines to compile; do not change behavior.

- [ ] **Step 6: Wire the hook into ChangeProperty**

In `crates/yserver-core/src/core_loop/process_request.rs`, in the ChangeProperty handler right after the property is stored (`set_window_property`, ~22843) and before/near `backend.on_window_property_changed` (22844), add:

```rust
        if req.window == crate::resources::ROOT_WINDOW
            && state.atoms.name(req.property) == Some("_XKB_RULES_NAMES")
        {
            // Read the *stored* value, not `req.data`: ChangeProperty
            // Append/Prepend mode means the effective property is the
            // accumulated bytes, not just this request's payload. Clone
            // so we drop the immutable borrow before the &mut call.
            if let Some(value) = state
                .resources
                .window_property(req.window, req.property)
                .map(|p| p.data.clone())
            {
                crate::core_loop::xkb_layout::apply_rules_names_change(
                    state, backend, &value,
                );
            }
        }
```

> Confirm the accessor: the gathered references show `state.resources.window_property(window, atom) -> Option<&PropertyValue>` and `PropertyValue` carries a `data: Vec<u8>` (it's the same value type `set_window_property` stores). Match the real field/method names when implementing.

**`XkbSelectEvents` detail-mask limitation (codex finding #2).** yserver's current SelectEvents handler (`process_request.rs:14679`) records only `selectAll & ~clear` — the top-level event bits — and ignores the per-event *detail* masks Xorg stores as `newKeyboardNotifyMask`/`mapNotifyMask` (`xkb.c:237`). For the common clients this is fine: GTK/xkbcommon-x11 select these events via the select-all path, so the bit is set and `subscribers()` finds them; and plain Xlib clients are covered by the core `MappingNotify` we send to *all* clients regardless. The gap is a client that selects MapNotify via detail mask only (affectMap/map) without select-all — it won't be in our list. Document this in the `subscribers()` doc comment as a known limitation; do not expand the SelectEvents parser in this task (out of scope, and no observed client needs it).

- [ ] **Step 7: Run the full crate tests + build**

Run: `cargo build --locked 2>&1 | tail -20 && cargo test -p yserver-core --locked xkb 2>&1 | tail -20`
Expected: builds clean; xkb_layout tests PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/yserver-core/src/core_loop/xkb_layout.rs crates/yserver-core/src/core_loop/mod.rs crates/yserver-core/src/core_loop/process_request.rs
git commit -m "feat(xkb): recompile + notify clients on _XKB_RULES_NAMES change"
```

---

### Task A6: Hardware verification (bee / Cinnamon) — user-driven smoke

> Per `feedback_commit_after_testing` + `feedback_hw_recipes_user_only`: interactive layout behavior needs a visible HW smoke, run by the user (one agent per checkout). This task is a checklist, not code.

- [ ] **Step 1: Capture the Xorg-side reference trace** (fulfils "do what Xorg does")

On a real Xorg + Cinnamon session, capture what a layout switch emits, to confirm our event set matches:
```bash
# under Xorg+Cinnamon:
xtrace -o /tmp/xorg-layout-switch.trace -- <client>   # or attach via `just xts-xorg-trace`-style tooling
# then switch layout (us -> be) in Cinnamon's Keyboard > Layouts and stop xtrace.
```
Confirm the trace shows the `_XKB_RULES_NAMES` property write + the MappingNotify / XkbNewKeyboardNotify / XkbMapNotify events. If Xorg emits an event we don't, add it; if a client only reacts to one we omit, add it.

- [ ] **Step 2: Run yserver on bee under Cinnamon**

```bash
just startx        # or the bee/Cinnamon HW recipe; rebuilds automatically
```

- [ ] **Step 3: Switch layout in Cinnamon**

Open Cinnamon Settings → Keyboard → Layouts, add `Belgian (be)` (or `German`), switch to it. Open a text field (e.g. a terminal or text editor) and type the keys that differ between `us` and the chosen layout (e.g. `q`/`a` swap on AZERTY, `y`/`z` swap on German).

- [ ] **Step 4: Verify**

- Expected: typed characters match the *new* layout, both in apps launched *before* the switch (proves the notify path) and apps launched *after* (proves GetMap re-query).
- Check `yserver-*-cinnamon.log` for the `xkb: recompiled keymap` and `xkb: applied layout` INFO lines.
- If wrong: grep the fresh HW trace per `feedback_logs_before_input_fixes` before theorizing.

- [ ] **Step 5: Record the result** in the PR description (visible-smoke evidence per `feedback_tests_are_not_visible_evidence`).

---

# PART B — Startup default (env + `-layout`)

### Task B1: `resolve_startup_rmlvo` from `XKB_DEFAULT_*` env

**Files:**
- Modify: `crates/yserver/src/kms/core.rs` (add helper; use it in `new` ~1649-1671)

- [ ] **Step 1: Write the failing test (pure resolver)**

```rust
#[test]
fn resolve_rmlvo_prefers_inputs_over_defaults() {
    // Pure function: explicit values win; missing fall back to evdev/pc105/us.
    let r = resolve_rmlvo_from(
        None,                 // rules env
        None,                 // model env
        Some("be".into()),    // layout env
        None,                 // variant env
        None,                 // options env
        None,                 // -layout arg
    );
    assert_eq!(r.rules, "evdev");
    assert_eq!(r.model, "pc105");
    assert_eq!(r.layout, "be");

    // -layout arg overrides env layout (Xorg cmdline precedence).
    let r2 = resolve_rmlvo_from(
        None, None, Some("be".into()), None, None, Some("de".into()),
    );
    assert_eq!(r2.layout, "de");

    // Nothing set -> us default.
    let r3 = resolve_rmlvo_from(None, None, None, None, None, None);
    assert_eq!(r3.layout, "us");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver --locked resolve_rmlvo 2>&1 | tail -20`
Expected: FAIL — function not found.

- [ ] **Step 3: Implement the pure resolver + env wrapper**

In `core.rs`:

```rust
/// Pure RMLVO resolution: explicit `layout_arg` (from `-layout`) wins,
/// then the env-provided value, else the Xorg-style default. Rules/model
/// default to evdev/pc105 (the evdev ruleset everything on Linux uses).
pub(crate) fn resolve_rmlvo_from(
    rules_env: Option<String>,
    model_env: Option<String>,
    layout_env: Option<String>,
    variant_env: Option<String>,
    options_env: Option<String>,
    layout_arg: Option<String>,
) -> XkbRmlvo {
    XkbRmlvo {
        rules: rules_env.filter(|s| !s.is_empty()).unwrap_or_else(|| "evdev".into()),
        model: model_env.filter(|s| !s.is_empty()).unwrap_or_else(|| "pc105".into()),
        layout: layout_arg
            .filter(|s| !s.is_empty())
            .or(layout_env.filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "us".into()),
        variant: variant_env.unwrap_or_default(),
        options: options_env.filter(|s| !s.is_empty()),
    }
}

/// Read `XKB_DEFAULT_*` from the environment + the `-layout` arg.
pub(crate) fn resolve_startup_rmlvo(layout_arg: Option<String>) -> XkbRmlvo {
    resolve_rmlvo_from(
        std::env::var("XKB_DEFAULT_RULES").ok(),
        std::env::var("XKB_DEFAULT_MODEL").ok(),
        std::env::var("XKB_DEFAULT_LAYOUT").ok(),
        std::env::var("XKB_DEFAULT_VARIANT").ok(),
        std::env::var("XKB_DEFAULT_OPTIONS").ok(),
        layout_arg,
    )
}
```

- [ ] **Step 4: Use it in `KmsCore::new`**

`KmsCore::new` gains a `layout_arg: Option<String>` param. Replace the hardcoded `new_from_names` (`core.rs:1649-1669`) with:

```rust
    pub(crate) fn new(fb_w: u16, fb_h: u16, layout_arg: Option<String>) -> io::Result<Self> {
        let xkb_context = XkbContext(xkbcommon::xkb::Context::new(
            xkbcommon::xkb::CONTEXT_NO_FLAGS,
        ));
        let rmlvo = resolve_startup_rmlvo(layout_arg);
        let keymap = xkbcommon::xkb::Keymap::new_from_names(
            &xkb_context.0,
            &rmlvo.rules,
            &rmlvo.model,
            &rmlvo.layout,
            &rmlvo.variant,
            rmlvo.options.clone(),
            xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .or_else(|| {
            // Fall back to a guaranteed-valid us map if the requested
            // RMLVO doesn't compile (bad -layout / bad env).
            xkbcommon::xkb::Keymap::new_from_names(
                &xkb_context.0, "evdev", "pc105", "us", "", None,
                xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        })
        .ok_or_else(|| io::Error::other("failed to create xkb keymap"))?;
```

Then set `xkb_rmlvo: rmlvo,` (instead of `XkbRmlvo::default()`) in the returned struct. **Caveat:** if the fallback path was taken, the stored `rmlvo` won't match the compiled `us` map — guard by re-defaulting on fallback. Simplest correct form: build `rmlvo`, try compile; on `None`, set `rmlvo = XkbRmlvo::default()` and compile `us`. Restructure so the stored `xkb_rmlvo` always matches the compiled keymap.

- [ ] **Step 5: Fix the `KmsCore::new` call sites**

`backend.rs:979` and `backend.rs:1130` (the two real constructors). For now pass the layout through (Task B2 supplies it); to keep this task compiling independently, pass `None` here and let B2 replace it:

```rust
        let mut core = KmsCore::new(fb_w, fb_h, None)?;
```

(`for_tests` is unchanged — it doesn't call `new`.)

- [ ] **Step 6: Run tests + build**

Run: `cargo test -p yserver --locked resolve_rmlvo 2>&1 | tail -20 && cargo build --locked 2>&1 | tail -10`
Expected: PASS + clean build.

- [ ] **Step 7: Commit**

```bash
git add crates/yserver/src/kms/core.rs crates/yserver/src/kms/v2/backend.rs
git commit -m "feat(xkb): resolve startup layout from XKB_DEFAULT_* env"
```

---

### Task B2: Thread `-layout` from argv to the backend

**Files:**
- Modify: `crates/yserver/src/launch.rs` (add `layout` field; parse `-layout`)
- Modify: `crates/yserver/src/lib.rs` (thread `opts.layout` into `build_kms_backend_v2`)
- Modify: `crates/yserver/src/kms/v2/backend.rs` (`open`, `open_libseat`, `open_with_commit` gain a `layout: Option<String>` param)

- [ ] **Step 1: Write the failing test (argv parse)**

In `launch.rs` test module (or add one):

```rust
#[test]
fn parse_layout_arg() {
    let opts = parse_args(["-layout".to_string(), "be".to_string()]).unwrap();
    assert_eq!(opts.layout.as_deref(), Some("be"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver --locked parse_layout_arg 2>&1 | tail -20`
Expected: FAIL — `layout` field missing.

- [ ] **Step 3: Add the field + parse `-layout`**

In `LaunchOptions` (`launch.rs:28-42`) add:

```rust
    /// `-layout NAME` — XKB layout for the startup keymap (Xorg-style).
    pub layout: Option<String>,
```

Change the `-layout` arm: it is currently bundled into the value-taking no-op `matches!(...)` (`launch.rs:75-83`). Pull `-layout` out into its own arm before that block:

```rust
        } else if arg == "-layout" {
            o.layout = Some(next_value(&mut it, "-layout")?);
        } else if matches!(arg.as_str(), "-nolisten" | "-config" | "-background") {
            // Known value-taking no-ops.
            if it.next().is_none() {
                log::warn!("yserver: {arg} given without a value; ignoring");
            }
```

- [ ] **Step 4: Thread through the backend constructors**

In `crates/yserver/src/kms/v2/backend.rs` — note there are **two** `KmsCore::new` call sites (Direct mode and libseat mode), so both entry chains need the param (codex round-4):
- Direct: `open(device_path, console_guard)` → `open(device_path, console_guard, layout: Option<String>)`; pass `layout` to `open_with_commit`, which passes it to `KmsCore::new(fb_w, fb_h, layout)?` (line 979).
- libseat: `open_libseat(...)` → `open_libseat_with_commit(...)` is where the libseat-mode `KmsCore::new(fb_w, fb_h, ...)?` actually lives (~`backend.rs:1098`/`1130`). Thread `layout: Option<String>` through `open_libseat` → `open_libseat_with_commit` → that `KmsCore::new` call. Grep `KmsCore::new` first and confirm both sites get the argument.

In `crates/yserver/src/lib.rs`:
- `build_kms_backend_v2(seat, &device_path, console_guard)` (line 256, def ~524) gains a `layout: Option<String>` param, threaded to both `KmsBackendV2::open_libseat(...)` (540) and `KmsBackendV2::open(device_path, console_guard, layout)` (552).
- At the call site (line 256): `build_kms_backend_v2(seat, &device_path, console_guard, opts.layout.clone())?`.

> `opts` is `run`'s `LaunchOptions` param (lib.rs:43). `opts.layout` is in scope. Clone because `opts` may be used later.

- [ ] **Step 5: Run test + build**

Run: `cargo test -p yserver --locked parse_layout_arg 2>&1 | tail -20 && cargo build --locked 2>&1 | tail -10`
Expected: PASS + clean build.

- [ ] **Step 6: Manual sanity (vng, optional)**

`yserver -layout be` then run `xkbcomp -version`-style check or `setxkbmap -query` — `layout: be` should report. (Full HW smoke is Task A6.)

- [ ] **Step 7: Commit**

```bash
git add crates/yserver/src/launch.rs crates/yserver/src/lib.rs crates/yserver/src/kms/v2/backend.rs
git commit -m "feat(xkb): honor -layout startup argument"
```

---

# PART C — Proper key types in GetMap (AltGr / 4-level)

> **Why this is required, not optional:** real layouts like `be`/`fr`/`de`/`us(intl)` put AltGr symbols on levels 3–4. Today `reply_get_map` (`xkb.rs:289`) tags any key with `width >= 2` as `TWO_LEVEL` (`type_index = 1`), whose published type only defines `Shift → level 1`. A 4-level key therefore ships 4 syms but a type that can only reach level 1 — so AltGr/`LevelThree` symbols never resolve on the client. We must publish real key **types** whose modifier→level maps are derived from the live keymap.
>
> **External-vector discipline:** the type-section wire bytes are cross-checked against a captured Xorg `GetMap` reply (Task C0). The *modifier→level* data itself is read from xkbcommon via `key_get_mods_for_level` (binding confirmed at `xkbcommon-0.9.0/src/xkb/mod.rs:1123`) — not invented.

### Task C0: Capture the golden Xorg `GetMap` vector

**Files:** Create `crates/yserver/tests/fixtures/getmap-be.bin` (+ a short README noting provenance)

- [ ] On a real Xorg, set the target layout and dump the XKB `GetMap` reply for the keyboard. Capture via an `xcb`/`xkbcommon-x11` probe or `xtrace` of `setxkbmap -layout be` + a client `XkbGetMap`. Record: exact `nTypes`, each `xkbKeyTypeWireDesc` (mask/numLevels/nMapEntries + map entries `{realMods, virtualMods, level}`), and the per-key `ktIndex[]`.
- [ ] Save the raw reply bytes as the fixture; note the Xorg version + `setxkbmap -query` output in the README (provenance per `feedback_test_vectors_must_be_external`).

> This fixture is the source of truth for C2's assertions. Without it, do not hand-author expected type bytes.

### Task C1: Derive key types from xkbcommon

**Files:** Modify `crates/yserver/src/kms/xkb.rs` (new `key_types_from_keymap` helper + `KeyTypeDesc` struct)

- [ ] **Step 1: Write the failing test** — for the `de`/`be` keymap, assert the helper produces a `FOUR_LEVEL`-shaped type (numLevels 4, map entries for `Shift`, `LevelThree`(real-mod), `Shift+LevelThree`) for a known AltGr key (e.g. keycode for `AD01`), reading expected level→mods from `key_get_mods_for_level`.

```rust
#[test]
fn key_types_include_four_level_for_altgr() {
    let mut core = KmsCore::for_tests();
    core.recompile_keymap(&crate::kms::core::XkbRmlvo {
        rules: "evdev".into(), model: "pc105".into(),
        layout: "de".into(), variant: String::new(), options: None,
    });
    let types = key_types_from_keymap(&core.xkb_keymap.0);
    assert!(
        types.iter().any(|t| t.num_levels == 4 && t.map_entries.len() >= 3),
        "a FOUR_LEVEL type must be derived for AltGr keys, got {types:?}"
    );
}
```

- [ ] **Step 2: Run** `cargo test -p yserver --locked key_types_include_four_level` — expect FAIL (helper missing).
- [ ] **Step 3: Implement `key_types_from_keymap`.** For each key+group, use `keymap.num_levels_for_key` and `keymap.key_get_mods_for_level(kc, group, level, &mut masks)` to read the real-mod mask(s) that select each level. Build a `KeyTypeDesc { num_levels, map_entries: Vec<{ real_mods: u8, level: u8 }> }`, dedup identical signatures into a type table (always seed indices 0..=3 = ONE_LEVEL/TWO_LEVEL/ALPHABETIC/KEYPAD to preserve the existing `XkbNumRequiredTypes >= 4` invariant noted at `xkb.rs:354-359`), and record each (key,group)→type index. Map the `LevelThree` real-mod from `virtual_mods_from_keymap` (already in this file) so the FOUR_LEVEL entry uses the correct real modifier bit.
- [ ] **Step 4: Run** the test — expect PASS.
- [ ] **Step 5: Commit** `feat(xkb): derive key types (incl. FOUR_LEVEL) from keymap`.

### Task C2: Serialize the derived types in `reply_get_map`

**Files:** Modify `crates/yserver/src/kms/xkb.rs` (`reply_get_map`, KeyTypes section ~353-366 + per-key `type_index` ~328)

> **Assertion strategy (revised per re-review):** do NOT byte-match the whole KeyTypes section against `fixtures/getmap-be.bin`. Xorg's `complete` ruleset emits a large type table in its own order; ours, derived from xkbcommon, is functionally equivalent but will differ in ordering/count — a raw byte-equality test is brittle and would fail on an equally-correct reply. Instead assert **functional resolution**: parse our own reply and verify that, for representative keys, each modifier combination maps to the keysym the layout actually produces. Use the captured Xorg fixture only for targeted spot-checks of specific keys, not whole-section equality.

- [ ] **Step 1: Functional + structural test** — for `be`, build `reply_get_map`, parse the KeyTypes + KeySymMap sections, and assert: (functional) a known AltGr key resolves correctly at every level — for the key carrying `€`, applying the `LevelThree` real-mod selects the level whose keysym is `EuroSign` (`0x20ac`); cross-check 2–3 keys' level-0/level-1 against the captured Xorg fixture's same keycodes. (structural, codex re-review #3) every serialized per-key `type_index < nTypes`, and `nTypes` matches the emitted KeyTypes section header count. Spot-check, not whole-section byte equality.
- [ ] **Step 2: Run** — expect FAIL (4-stub table can't reach level ≥ 2).
- [ ] **Step 3: Implement** — replace the fixed 4-type stub with the C1 type table; set each key's `type_index` to the C1-recorded index instead of the `width >= 2 ? 1 : 0` heuristic; recompute the KeyTypes section size from the real table.
- [ ] **Step 4: Run** — expect PASS. Re-run existing `reply_get_map` tests (single-group `us` must still pass).
- [ ] **Step 5: Commit** `feat(xkb): publish real key types in GetMap`.

### Task C2b: Update `reply_get_names` in lockstep with the type table

> **Codex finding (re-review):** `reply_get_names` (`xkb.rs:548`) hard-codes exactly 4 type-name atoms (`ONE_LEVEL/TWO_LEVEL/ALPHABETIC/KEYPAD`), `nTypes = 4` (`xkb.rs:624`), and 5 KT-level names (`xkb.rs:632,677`). Once C2 makes `reply_get_map` publish N derived types, `GetNames` MUST agree — `nTypes`, the per-type name atoms, `nKTLevels`, and the `nLevelsPerType`/level-name arrays must all match `reply_get_map`, or a client reading both replies (xkbcommon-x11 reads GetMap *and* GetNames) sees an inconsistent keymap and may reject it.

**Files:** Modify `crates/yserver/src/kms/xkb.rs` (`reply_get_names` type-name section ~599-604, 624, 632, 661-681)

- [ ] **Step 1: Failing test** — for `be`, assert `reply_get_names`'s `nTypes` byte equals `reply_get_map`'s `nTypes`, and that the KT-level count (`nKTLevels`) equals the sum of `nLevelsPerType` over the derived table. (Both replies must be driven from the same C1 type table.)
- [ ] **Step 2: Run** — expect FAIL (GetNames still says 4).
- [ ] **Step 3: Implement** — source `nTypes`, `nLevelsPerType[]`, the type-name atoms, and the KT-level-name atoms from the **same** C1 `key_types_from_keymap` table `reply_get_map` uses (factor the table once, share it). Assign each derived type a canonical name (`FOUR_LEVEL`, etc.) by its `(num_levels, map signature)`; fall back to a synthesized `"type<N>"` atom for any non-canonical shape so every type slot still carries a real interned atom (the BadAtom-exit hazard at `xkb.rs:637-642`).
- [ ] **Step 4: Run** — expect PASS; the GetMap/GetNames consistency test is green and existing `us` GetNames tests still pass.
- [ ] **Step 5: Commit** `feat(xkb): GetNames type/level names track the derived type table`.

### Task C3: HW smoke — AltGr on a single non-US layout

- [ ] On bee, `setxkbmap be` (or via Cinnamon), open a text field, type AltGr-level keys (e.g. AltGr+`e` → €, the `@`/`#`/`{`/`}` row on AZERTY). Confirm the AltGr symbols appear. Record evidence.

---

# PART D — Multi-group layouts + group switching

> Cinnamon configured with e.g. `us,ru` compiles a 2-group keymap and switches the active **group**. yserver must (1) serialize all groups in `GetMap`, (2) report the active group in key-event `state` bits 13–14, (3) emit `XkbStateNotify` on group change. Group *switching itself is free* for the **keyboard-driven** path: xkbcommon advances/locks the effective group inside `xkb_state` when keys with `grp:` actions (e.g. `grp:alt_shift_toggle` — Cinnamon's default) are fed through `update_key` (already called at `backend.rs:5243`); `serialize_layout(STATE_LAYOUT_EFFECTIVE)` reads the result. The key path (`key_fanout.rs`) holds `&mut state` and `&mut dyn Backend`, so it fans out `XkbStateNotify` directly when the group changes — the only backend addition is a read-only `current_xkb_group()` accessor (D3), no event-signalling channel.
>
> **Descoped (codex re-review #2): explicit `XkbLatchLockState` (applet-click group lock).** The 0.9 xkbcommon binding exposes **no master-safe lock-group API** — the only layout-lock entry point is `update_mask`, which its own docs forbid for a master state (the same reason Part A keeps `update_key`). Honoring an applet-click `XkbLatchLockState` would require either an unsafe `update_mask` on the master or a parallel group-offset that desyncs server-side keysym resolution — both are wrong. So minor 5 stays a void no-op (as today; honest, not a stub), and applet-click switching is a **documented follow-up**. The keyboard hotkey path (Cinnamon's default switch mechanism) is fully covered.
>
> **Event selection (codex re-review #1): `StateNotify` needs detail masks.** `XkbStateNotify` is selected by clients via `XkbSelectEventDetails` with a per-event `stateNotifyMask`, **not** via top-level `selectAll`. yserver's current SelectEvents handler (`process_request.rs:14679`) records only `selectAll & ~clear` and discards the detail payload, so a layout-indicator client that selects `StateNotify` by detail would be dropped and never update. Task D2b fixes the parser before D3 relies on it.
>
> **External-vector discipline:** the multi-group `KeySymMap` wire bytes are cross-checked against a captured Xorg `GetMap` for `us,ru` (Task D0), and the `XkbStateNotify` layout is taken from `/usr/include/X11/extensions/XKBproto.h` (`xkbStateNotify`) — not invented.

### Task D0: Capture golden Xorg vectors

**Files:** Create `crates/yserver/tests/fixtures/getmap-us-ru.bin` + note `xkbStateNotify` offsets from XKBproto.h in the test.

- [ ] Capture Xorg `GetMap` for `setxkbmap -layout us,ru`: record per-key `groupInfo` (num groups), `width`, `nSyms`, and the group-major sym layout. Save as fixture with provenance.
- [ ] Record the `xkbStateNotify` struct field offsets (type, xkbType=2, deviceID, mods, baseMods, latchedMods, lockedMods, group, baseGroup, latchedGroup, lockedGroup, ...) from XKBproto.h for D3.

### Task D1: Multi-group serialization in `reply_get_map`

**Files:** Modify `crates/yserver/src/kms/xkb.rs` (`reply_get_map` per-key loop ~299-346, drop the "layout 0 only" restriction)

- [ ] **Step 1: Functional + structural test** — for `us,ru`, build `reply_get_map`, parse a known key's `KeySymMap`, and assert:
  - *Functional:* group-0 (`us`) and group-1 (`ru`, Cyrillic) keysyms land at the right group-major offsets (`[g0 levels..., g1 levels...]`); group-1's keysym is the expected Cyrillic letter (ground truth from the `ru` layout).
  - *Structural (codex re-review #3):* for that key, `groupInfo`'s group count and `width` and `nSyms` match the captured Xorg fixture's same keycode; and **every** serialized `ktIndex[group] < nTypes` (advertised type-table size). This catches a malformed header that a keysym-only functional test would pass. Do NOT byte-match `ktIndex[]` values against Xorg (they reference our type table, not Xorg's) — only the count/width/nSyms shape + the in-range invariant.
- [ ] **Step 2: Run** — expect FAIL (only group 0 emitted today).
- [ ] **Step 3: Implement** — loop `group in 0..num_layouts_for_key(kc)` (clamp to 4 = `XkbNumKbdGroups`); `width = max` over groups of `num_levels_for_key(kc, group)`; `nSyms = width * num_groups`; fill `syms[group*width + level]`; `groupInfo = num_groups`; `ktIndex[group]` from the C1 per-(key,group) index. Update modmap capture to use group 0 level 0 (unchanged convention).
- [ ] **Step 4: Run** — PASS against vector; existing single-group `us` test still passes (num_groups=1 path unchanged).
- [ ] **Step 5: Commit** `feat(xkb): serialize all keyboard groups in GetMap`.

### Task D1b: `GetNames` group names track the group count

> Same GetMap/GetNames coupling class as C2b (codex re-review answer): once D1 advertises `num_groups > 1`, `reply_get_names` must populate the `GroupNames` section + set the matching `which` bit (`xkb.rs:578-580`) and the `groupNames` byte at `xkb.rs:625`, else a client comparing the two replies sees a group-count mismatch.

**Files:** Modify `crates/yserver/src/kms/xkb.rs` (`reply_get_names`: `which` mask ~578, groupNames byte ~625, add a GroupNames atom section)

- [ ] **Step 1: Failing test** — for `us,ru`, assert `reply_get_names` advertises 2 group names (`GroupNames` bit set in `which`, two interned group-name atoms, e.g. `"English (US)"`/`"Russian"` or the layout codes) consistent with `reply_get_map`'s `num_groups`.
- [ ] **Step 2: Run** — FAIL (groupNames absent today).
- [ ] **Step 3: Implement** — derive the group count from the keymap (`num_layouts`), set the `GroupNames` `which` bit, write the `groupNames` field, and append exactly `num_layouts` interned atoms — one per group, from `keymap.layout_get_name(i)` (exposed at `xkbcommon-0.9.0 src/xkb/mod.rs:897`). Recompute the section size.
- [ ] **Step 4: Run** — PASS. **Pinned behavior (codex round-4):** always emit exactly `num_layouts` group names — so single-group `us` emits **1** group name (not 0). Update the existing single-group GetNames test to expect 1.
- [ ] **Step 5: Commit** `feat(xkb): GetNames advertises group names for multi-group`.

### Task D2: Report the active group in key-event state bits

**Files:** Modify `crates/yserver/src/kms/v2/backend.rs` (`serialize_modifiers` ~5156)

- [ ] **Step 1: Failing test** — drive `xkb_state` into group 1 (feed a group-lock, or call `update_key` on a `grp:` key), assert `serialize_modifiers()` has bits 13–14 = `1` (`(group & 0x3) << 13`, X11 `XkbGroupForCoreState`).
- [ ] **Step 2: Run** — FAIL (group bits never set).
- [ ] **Step 3: Implement** — after the mod bits, OR in the effective group: `let grp = self.core.xkb_state.0.serialize_layout(xkbcommon::xkb::STATE_LAYOUT_EFFECTIVE); mask |= ((grp as u16) & 0x3) << 13;` (confirm the binding method name; it wraps `xkb_state_serialize_layout`).
- [ ] **Step 4: Run** — PASS. Rename the method comment to note it now carries group bits too.
- [ ] **Step 5: Commit** `feat(xkb): encode active group in key-event state`.

### Task D2b: Parse `XkbSelectEvents` detail masks (prerequisite for D3)

> **Codex re-review must-fix #1.** Today `handle_xkb_request` minor 1 (`process_request.rs:14679`) stores only `selectAll & ~clear` keyed by `(client, device_spec)`. That is enough for the `A5` New-Keyboard/Map paths (clients select those via select-all), but **`StateNotify` is selected by detail mask**, so the current parser drops indicator clients. We must parse the request the way Xorg's `ProcXkbSelectEvents` does (`xkb.c:222-300`): `affectWhich`, `clear`, `selectAll`, then per affected event a `{affect, detail}` pair, and record the resulting per-event detail mask.

> **Codex round-4 correction:** it is NOT enough to record that the top-level `StateNotify` bit was selected and then deliver on that bit. Xorg stores a per-event **detail mask** (`stateNotifyMask`) and delivers a `StateNotify` only when `stateNotifyMask & changed != 0` (`xkb.c:328,350`; `xkbEvents.c:240`). So we must store the actual `state_detail` mask and D3 must filter delivery by it — folding into the top `u16` (the discarded round-3 idea) loses exactly the bits the filter needs.

**Files:** Modify `crates/yserver-core/src/server.rs` (the `xkb_select_event_masks` value type ~747) and `crates/yserver-core/src/core_loop/process_request.rs` (minor-1 parse ~14679-14692)

- [ ] **Step 1: Failing test** — feed a synthetic `XkbSelectEvents` body that selects `StateNotify` (event index 2) via the **detail** path (`affectWhich` bit 2 set, `selectAll` bit 2 NOT set, trailing per-event section carries an `affect`/`detail` pair with `GroupState` set), and assert: (a) the stored value's top mask has the StateNotify bit, AND (b) the stored `state_detail` mask equals the selected detail bits. Add a second case selecting NewKeyboard/Map via `selectAll` and assert those still record (regression guard).
- [ ] **Step 2: Run** `cargo test -p yserver-core --locked xkb_select_events_detail` — FAIL.
- [ ] **Step 3: Implement** — change the stored value from `u16` to a struct, e.g. `XkbEventMasks { top: u16, state_detail: u16, map_detail: u16 }` (keyed `(client, device_spec)` as before). Parse faithfully to `ProcXkbSelectEvents` (`xkb.c:222-300`) — **note the MapNotify special-case (codex round-5):** MapNotify is NOT in the trailing stream. Xorg reads `map_detail` from the **fixed header** `affectMap`/`map` fields (`xkb.c:237-239`: `mapNotifyMask &= ~affectMap; mapNotifyMask |= affectMap & map`), then iterates the trailing `{affect, detail}` payload only for events in `affectWhich & ~XkbMapNotifyMask` (`xkb.c:262`). So: set `map_detail` from `affectMap`/`map` (header); walk the trailing per-event stream for the **non-MapNotify** selected events and store `state_detail` for event index 2 (StateNotify detail is 2 bytes); set `top` from `selectAll & ~clear` plus any event with a present detail. `subscribers(top_bit)` keeps working for A5; D3 reads `state_detail`.
- [ ] **Step 4: Run** — PASS; re-run existing XKB tests (select-all path unchanged).
- [ ] **Step 5: Commit** `feat(xkb): store XkbSelectEvents per-event detail masks`.

### Task D3: `XkbStateNotify` on keyboard-driven group change

**Files:**
- Modify `crates/yserver-protocol/src/x11/mod.rs` (add `write_xkb_state_notify`, layout from XKBproto.h `xkbStateNotify`)
- Modify `crates/yserver-core/src/server.rs` (add `last_xkb_group: u8` to `ServerState`)
- Add a backend accessor `fn current_xkb_group(&self) -> u8` (`trait_def.rs` default `0`; KMS impl returns `serialize_layout(STATE_LAYOUT_EFFECTIVE)`)
- Modify `crates/yserver-core/src/core_loop/key_fanout.rs` (after the key is processed, compare current vs last group, fan out filtered `XkbStateNotify`)

> Scope: the **keyboard-driven** group switch (Cinnamon's default `grp:` hotkey). Explicit `XkbLatchLockState` (applet click) is descoped (see Part D intro) — minor 5 stays a void no-op.
>
> **Plumbing seam (codex round-4 must-fix).** `update_key` is called inside `KmsBackendV2::cook_host_key` (`backend.rs:5243`, ~`10024`/`10033`), *before* `key_fanout.rs` runs, and its return is discarded — so the plan can NOT "use update_key's return value in the key path." Instead: `key_fanout.rs` already holds `&mut state` and a `&mut dyn Backend`. After delivering the key event, read `backend.current_xkb_group()`, compare to `state.last_xkb_group`; if changed, fan out `XkbStateNotify` and update `state.last_xkb_group`. No new backend→core signal channel needed; the group lives in `xkb_state`, which the backend already owns and can expose read-only.

- [ ] **Step 1: Failing wire test** — `write_xkb_state_notify` byte-layout test with vectors from `XKBproto.h xkbStateNotify` (full struct, offsets confirmed in D0): byte0 = event base, byte1 = `xkbType` = 2, `sequenceNumber`, `time`, `deviceID`, then `mods`/`baseMods`/`latchedMods`/`lockedMods` (CARD8 each), `group`(CARD8)/`baseGroup`(INT16)/`latchedGroup`(INT16)/`lockedGroup`(CARD8), `compatState`, `grabMods`/`compatGrabMods`, `lookupMods`/`compatLookupMods`, `ptrBtnState`(CARD16), `changed`(CARD16), **and the trailing cause bytes `keycode`(CARD8), `eventType`(CARD8), `requestMajor`(CARD8), `requestMinor`(CARD8)** (`XKBproto.h:1100-1103` — codex round-5: these were missing). All 32 bytes. Assert each field offset, including the four cause bytes.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement the encoder** — fill every state field from the live `xkb_state`: effective/base/latched/locked **mods** via `serialize_mods(STATE_MODS_*)` mapped to the X11 real-mod bits (reuse `serialize_modifiers`' mapping), and effective/base/latched/locked **group** via `serialize_layout(STATE_LAYOUT_*)` (base/latched/locked are distinct — do NOT write "active group everywhere"). Fill the **cause** bytes from the triggering key (Xorg sets these from the cause at `xkbEvents.c:804-807`): for a keyboard-driven group change, `keycode` = the key that triggered it, `eventType` = `KeyPress` (2), `requestMajor`/`requestMinor` = 0 (not a request-caused change). Pass these into `write_xkb_state_notify` as params.
- [ ] **Step 4: StateComponent → XKB `changed` mapping (codex round-4 must-fix)** — the xkbcommon `StateComponent` returned by `update_key` and the XKB-protocol `changed` mask are different vocabularies. Define the mapping explicitly in a small helper next to `write_xkb_state_notify`, covering at least: `XKB_STATE_LAYOUT_EFFECTIVE`→`XkbGroupStateMask`, `XKB_STATE_LAYOUT_LOCKED`→`XkbGroupLockMask`, `XKB_STATE_MODS_EFFECTIVE`→`XkbModifierStateMask`, `XKB_STATE_MODS_LOCKED`→`XkbModifierLockMask`, `XKB_STATE_MODS_LATCHED`→`XkbModifierLatchMask` (`XKB.h:191-210`). For D3 we emit on a group change with `changed = XkbGroupStateMask` (plus `XkbGroupLockMask` if the lock component changed).
- [ ] **Step 5: Wire group-change detection** — in `key_fanout.rs` after the event is delivered: `let g = backend.current_xkb_group(); if g != state.last_xkb_group { fan out XkbStateNotify(changed=XkbGroupStateMask, keycode=<this event's keycode>) to StateNotify subscribers whose state_detail & XkbGroupStateMask != 0; state.last_xkb_group = g; }`. Subscribers come from D2b's stored masks; filter on `state_detail`, not just the top bit.
- [ ] **Step 6: `last_xkb_group` re-sync (codex round-5 must-fix).** The edge detector compares against `state.last_xkb_group`; if `xkb_state` is rebuilt *outside* the key path that counter goes stale → a spurious or missed `StateNotify` on the next key. Add an explicit re-sync (`state.last_xkb_group = backend.current_xkb_group()`) at **every** non-key `xkb_state` rebuild/reset: A1 `recompile_keymap` (the new map resets group to 0), and the VT-acquire reset (`backend.rs:~10593`). Since `recompile_keymap` lives in the backend (no `state` there), expose the post-rebuild group and have the A5 `apply_rules_names_change` core-loop path (which holds `state`) set `state.last_xkb_group` after a successful recompile; for VT-acquire, re-sync wherever the core loop observes the acquire.
- [ ] **Step 7: Freeze/replay limitation.** `key_event_fanout_to_state` early-returns for frozen-keyboard queued events (`key_fanout.rs:131,142`) and replays later through helpers with no backend handle (`key_fanout.rs:148,213`). D3's detection only runs on the live path, so a group change on a replayed key won't emit `StateNotify`. Document this as a known limitation (rare: group-switch keys aren't typically grabbed/frozen); do not plumb a backend handle into the replay helpers in this task.
- [ ] **Step 8: Run** unit tests — PASS.
- [ ] **Step 9: Commit** `feat(xkb): XkbStateNotify on keyboard-driven group change`.

### Task D4: HW smoke — multi-group switching

- [ ] On bee, configure `us,ru` (or `us,be`) in Cinnamon with a **keyboard** switch hotkey (`grp:alt_shift_toggle`). Verify: typing in a text field reflects the active group both for apps started before and after a switch; the layout indicator updates on hotkey switch (proves the `XkbStateNotify` detail-mask path); the switch hotkey's held modifiers don't stick (the C-side `down_keys` reconcile). Note: switching via **clicking the panel applet** is the descoped `XkbLatchLockState` path and is expected NOT to work yet — confirm it's the only gap. Record evidence + compare against the Xorg reference trace.

---

# Finalization

### Task F1: Full gate

- [ ] `cargo +nightly fmt` (repo uses nightly rustfmt features per AGENTS.md / the global Rust prefs)
- [ ] `cargo clippy --workspace 2>&1 | tail -30` (plain clippy per `feedback_clippy_pedantic_default`; fix new warnings only)
- [ ] `cargo test --workspace --locked 2>&1 | tail -30` (all green)
- [ ] Commit any fmt/clippy fixups.

### Task F2: Codex spec/plan review

- [ ] Per project CLAUDE.md ("use codex command/skill for reviews of specs/plans") this plan is reviewed by codex *before* implementation begins (see the conversation; address findings, then execute).

### Task F3: Integration

- [ ] After all HW smokes pass on bee (A6 Cinnamon switch, C3 AltGr, D4 multi-group), open a PR (master is branch-protected — `project_master_protected`). PR body includes the smoke evidence + the Xorg reference-trace/golden-vector comparisons.

---

## Self-Review notes

- **Spec coverage:** runtime RMLVO change (A1–A5) + startup default (B1–B2) + AltGr/4-level types (C0–C3) + multi-group switching (D0–D4) + verification gates — covers single-layout replacement, AltGr layouts (be/fr/de), and multi-group (us,ru) live-switch.
- **Codex review round 1 applied:** MapNotify `changed` corrected to `0x07` with consistent fields (was inconsistent `0x47`); `down_keys` re-applied on state swap (held switch-hotkey modifiers); hook reads the stored property value not raw `req.data`; NewKeyboardNotify framed as the Xorg `GetKbdByName` mirror (not "re-read everything"); property hook documented as a compatibility shim, not general `XkbSetMap`; `XkbSelectEvents` detail-mask gap documented.
- **Codex review round 2 applied (after C/D added):** (1) `reply_get_names` now updated in lockstep with the type table — Task C2b — so GetMap/GetNames agree on `nTypes`/levels (the one concrete catch). (2) C2/D1 assertions switched from brittle whole-section byte-equality to functional modifier→keysym resolution (Xorg fixture used for keysym spot-checks only) — Xorg's type table is equivalent-but-differently-ordered. (3) State-swap keeps `update_key`/`down_keys` and documents locked-state reset as deliberate (xkbcommon docs forbid `update_mask` on a master state). Round-2 verdict text was truncated by a codex link-formatting glitch; the substantive finding (#1) and mechanism checks (serialize_modifiers wired to event `state` at `backend.rs:5246`, `update_key` returns group-change `StateComponent`, `xkbStateNotify` 32-byte layout, `ProcXkbLatchLockState`) were recovered from its exec trail and verified against the source.
- **Codex review round 3 applied (verdict "not ready as-is" → addressed):** (1) NEW Task **D2b** parses `XkbSelectEvents` detail masks so `StateNotify`-by-detail clients (layout indicators) are captured — was a functional D bug. (2) D3 **drops `XkbLatchLockState`** group-lock (no master-safe xkbcommon API; minor 5 stays an honest void no-op) and is rescoped to the keyboard-driven `grp:` switch using `update_key`'s returned `StateComponent`; the backend-interface concern disappears since the key path holds `&mut state`. (3) C2/D1 gained a minimal **structural header assertion** (`groupInfo`/`width`/`nSyms` spot-check vs the Xorg fixture + `ktIndex < nTypes`) so malformed `KeySymMap` headers can't slip past the functional tests. (4) NEW Task **D1b** keeps `GetNames` `groupNames` consistent with multi-group `GetMap`. Codex confirmed round-2 fixes #1–#3 correct; D4 notes applet-click switching as the one documented gap.
- **Codex review round 4 applied (verified the 3 must-fixes; 2 were STILL-OPEN):** #3 (structural assert) confirmed CLOSED. #1 reopened: D2b now stores the actual per-event **detail mask** (struct `XkbEventMasks`), and D3 filters delivery by `state_detail & changed` (not the top bit) — matching Xorg's `xkbEvents.c:240`. #2 reopened on plumbing: `update_key` runs in `KmsBackendV2::cook_host_key` before `key_fanout.rs`, so D3 now detects group change via a `backend.current_xkb_group()` accessor compared to `state.last_xkb_group` in the key path (no discarded-return-value dependency). New round-4 items folded in: D3 encoder now fills all base/latched/locked/effective mod+group fields + a `changed` mask, with an explicit xkbcommon-`StateComponent`→XKB-`changed` mapping; A2 `symbolsName` reframed as a best-effort approximation (handles multi-layout/options caveat); D1b group-name count pinned to `num_layouts` (1 for single-group); stale self-review note fixed; `cargo +nightly fmt`; B2 notes both `KmsCore::new` sites (Direct + `open_libseat_with_commit`). A1/A4/A5/B1/C confirmed sound.
- **Codex review round 5 applied (must-fix #1 CLOSED, #2 STILL-OPEN→fixed, +4 new):** #1 (StateNotify detail) confirmed CLOSED. Fixed: (a) D2b MapNotify detail now read from the `affectMap`/`map` header, not the trailing stream (`xkb.c:237-239`); (b) D3 `xkbStateNotify` encoder now includes the trailing cause bytes `keycode`/`eventType`/`requestMajor`/`requestMinor` (`XKBproto.h:1100-1103`); (c) explicit `StateComponent`→`changed` mapping table; (d) `write_xkb_map_notify` takes `n_types` (was hardcoded 4 — went stale once C2 lands); (e) D3 adds `last_xkb_group` re-sync at every non-key `xkb_state` rebuild (recompile, VT-acquire); (f) freeze/replay-path StateNotify documented as a known limitation; (g) stale text fixed (`subscribers` reads `.top` after D2b; Part D intro notes the `current_xkb_group()` accessor). Codex verdict: **A+B ready to implement now; C after the C0 capture; D after captures + these fixes.** Findings have converged from architectural (rounds 1–3) to wire-detail (rounds 4–5).
- **Phasing safety (codex Q4):** shipping A+B before C is a *strict improvement*, not a regression — today a non-`us` layout is ignored entirely (you get `us`); after A+B a `be` switch yields correct base+Shift typing (AltGr dead until C). GetMap/GetNames stay consistent within A (both 4 types); C updates both together (C2 + C2b).
- **External vectors:** C/D wire-exact sections are gated on captured Xorg `GetMap` fixtures (C0/D0) + XKBproto.h offsets — never my arithmetic (`feedback_test_vectors_must_be_external`). C1's modifier→level data comes from xkbcommon `key_get_mods_for_level` (binding confirmed).
- **Type consistency:** `XkbRmlvo` (yserver crate) vs `RulesNames` (yserver-core crate) are deliberately separate (crates can't share the type); `set_keymap_rmlvo` takes `&str`s as the bridge and returns `Option<(u8,u8)>` consistently across trait/impl/caller.
- **Known limitations (documented, not stubbed):** custom `xkbcomp - $DISPLAY` keymap uploads and layout changes that don't write `_XKB_RULES_NAMES` remain unsupported; explicit `XkbLatchLockState` (panel-applet group lock) is descoped (no master-safe xkbcommon lock-group API) — keyboard-hotkey group switching is covered. (`XkbSelectEvents` detail masks **are** now modeled — Task D2b.)
- **Open confirmation during impl:** accessor/field names (`PropertyValue.data`, `window_property`), `use` paths in `xkb_layout.rs`, and the `serialize_layout`/`STATE_LAYOUT_EFFECTIVE` binding names must be matched to the real code — flagged inline.
