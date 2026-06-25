# XKB Runtime Layout v2 — GetKbdByName + Group-Switch (corrected) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement task-by-task. Steps use checkbox (`- [ ]`) syntax.

> **Supersedes the trigger approach in `2026-06-24-xkb-runtime-layout.md`.** That plan triggered on a `_XKB_RULES_NAMES` ChangeProperty write. The captured wire (below) proves Cinnamon never writes that property — it uses `XkbGetKbdByName(23)` + `XkbLatchLockState(5)`. Parts A+B of the old plan are **already implemented and stay** (machinery is reused); this plan replaces the *trigger* and adds the group engine.

**Goal:** Make a real desktop layout switch take effect — switch Cinnamon to German and the physical **Y** key produces **z** — by handling the requests the client actually sends.

**Architecture (grounded in two captured xtraces in the repo root):**
- `cinnamon-xorg.xtrace` = real Xorg, where the switch WORKS. `cinnamon.xtrace` = the broken yserver run.
- Cinnamon loads a **multi-group** keymap via `XkbGetKbdByName(23)` with `symbols="pc+us+de:2+us:3+inet(evdev)"` (us=grp0, de=grp1, us=grp2; XKB groups are 1-based on the wire, 0-based internally) — `cinnamon-xorg.xtrace:6201`. It switches the active group via `XkbLatchLockState(5)` `lockGroup=1 groupLock=<g>` — `:36077`.
- Xorg's response: a valid `GetKbdByName` reply (`loaded=1`), **broadcast `XkbNewKeyboardNotify`** on load (`:6203+`), and **broadcast `XkbStateNotify`** on the group lock (`:36086+`). **No `MappingNotify`, no `MapNotify` anywhere.**
- **The load-bearing step:** Xorg stamps the active group into every KeyPress (`effective_group` 0 before the lock `:33933`, 1 after `:37115`). Same keycode; the client resolves keycode+group→keysym against the multi-group map it holds. yserver always stamps group 0 → Y stays y.

**Tech Stack:** Rust, `xkbcommon` 0.9, `Backend` trait, `fanout_event_to_clients`, XKB wire protocol. XKB major opcode on yserver = **136**; XKB event base = **85** (`xkb_info()`).

**External-vectors rule (load-bearing):** the fiddly wire (multi-group `KeySymMap`, `GetKbdByName` reply assembly, `XkbStateNotify`) is cross-checked against the **captured `cinnamon-xorg.xtrace`** + `/usr/include/X11/extensions/XKBproto.h` — never my arithmetic (`feedback_test_vectors_must_be_external`).

**Authoritative group state (codex re-plan #4):** we do NOT push the locked group into `xkb_state` via `update_mask` (xkbcommon docs forbid it on a master state mixed with `update_key`). We keep a `KmsCore.locked_group: u8` as the source of truth for event stamping + `StateNotify`; `xkb_state` stays for key-driven modifier tracking.

**Phase-1 LIMITATION — keyboard-shortcut group switching is NOT handled (codex review).** Phase 1 stamps the group from `locked_group`, which is set only by an explicit `XkbLatchLockState(5)` request. A keymap compiled with `grp:alt_shift_toggle` (etc.) switches the group via a *key action* inside `xkb_state` (`update_key`), which we do NOT read for stamping — so that path would leave the wire group stale. **This is acceptable for Phase 1 because the captured Cinnamon switch uses `LatchLockState`, not a compiled key action** (`cinnamon-xorg.xtrace:36077`). Reconciling `xkb_state`'s key-action group with `locked_group` (so Alt+Shift-style switching works) is **Phase 2**.

---

## Salvage from A+B (already on the branch — keep)
- `XkbRmlvo`, `resolve_startup_rmlvo`, `KmsCore::recompile_keymap` — reused (multi-group recompile + startup default).
- `Backend::set_keymap_rmlvo` — reused; minor 23 calls it with a multi-layout `layout="us,de,us"`.
- `write_xkb_new_keyboard_notify` encoder — reused but **generalized** (Task 2c): it hardcodes `requestMajor/Minor=0` and `changed=0x0001`; the GetKbdByName path needs `requestMajor=136, requestMinor=23, changed=0x0003`.
- `reply_get_names` (symbolsName from RMLVO) — kept.
- The `_XKB_RULES_NAMES` property hook (`xkb_layout.rs`) — **demoted to a documented secondary shim** for `setxkbmap`-CLI; its `MappingNotify`/`MapNotify` fanout is no longer the primary path (Task 4b doc-only).
- `write_xkb_map_notify` — kept as spare; not emitted on the primary path.

## File Structure
| File | Change |
|------|--------|
| `crates/yserver/src/kms/core.rs` | add `locked_group: u8` to `KmsCore`; multi-group already supported by `recompile_keymap` |
| `crates/yserver/src/kms/xkb.rs` | `reply_get_map` → serialize all groups; add `reply_get_kbd_by_name` |
| `crates/yserver/src/kms/v2/backend.rs` | `serialize_modifiers` → add group bits; impls for new backend methods; xkb_proxy minor 23/5 wired |
| `crates/yserver-core/src/backend/trait_def.rs` | add `load_keymap_by_components`, `set_locked_group`, `current_group` |
| `crates/yserver-protocol/src/x11/mod.rs` | add `write_xkb_state_notify`; XI2 encoder group quartet; generalize NewKeyboardNotify |
| `crates/yserver-core/src/core_loop/process_request.rs` | `handle_xkb_request` → handle minor 23/5, broadcast NewKeyboard/State notify |
| `crates/yserver-core/src/core_loop/xkb_layout.rs` | demote property hook to shim (docs) |

---

# PHASE 1a — The group engine (multi-group GetMap + group stamping + LatchLockState→StateNotify)

> Provable WITHOUT GetKbdByName: boot a multi-group keymap via the existing `-layout` path (`yserver -layout "us,de"` or `XKB_DEFAULT_LAYOUT=us,de`), then a group lock must flip typed keys to German. This isolates the load-bearing group machinery from the fiddly GetKbdByName reply (Phase 1b).

### Task 1a-1: `reply_get_map` serializes all groups

**Files:** Modify `crates/yserver/src/kms/xkb.rs` (`reply_get_map` per-key loop ~299-346 — currently "layout 0 only").

- [ ] **Step 1: Failing functional+structural test** — compile `us,de` (`KmsCore::for_tests()` then `recompile_keymap` to layout `"us,de"`), build `reply_get_map`, parse a known key's `KeySymMap`. Assert: `groupInfo` group-count = 2; group-major sym layout places group-0 (`us`) and group-1 (`de`) syms at the right offsets; for keycode 29 (AD06) group-0 sym = `y` (0x79) and group-1 sym = `z` (0x7a). Structural: every serialized `ktIndex[group] < nTypes`; `nSyms == width*num_groups`. Cross-check the group-1 keysym against `cinnamon-xorg.xtrace:6202` (the `z` appears once per group).
- [ ] **Step 2: Run** `cargo test -p yserver --locked get_map_multigroup 2>&1 | tail -20` — FAIL (only group 0 today).
- [ ] **Step 3: Implement** — loop `group in 0..num_layouts_for_key(kc)` (clamp to 4 = `XkbNumKbdGroups`); `width = max` over groups of `num_levels_for_key(kc, group)`; `nSyms = width*num_groups`; fill `syms[group*width + level]` from `key_get_syms_by_level(kc, group, level)`; `groupInfo = num_groups`; `ktIndex[group]` by width (0/1 today — 4-level types are Phase 2). Modmap stays group-0 level-0.
- [ ] **Step 4: Run** — PASS; existing single-group `us` test still passes (num_groups=1 path unchanged).
- [ ] **Step 5: Commit** `feat(xkb): serialize all keyboard groups in GetMap`.

### Task 1a-2: authoritative `locked_group` + stamp it into key events

**Files:** Modify `crates/yserver/src/kms/core.rs` (add field), `crates/yserver/src/kms/v2/backend.rs` (`serialize_modifiers` ~5169), `crates/yserver-protocol/src/x11/mod.rs` (XI2 encoder ~2007).

- [ ] **Step 1: Add the field** — `pub(crate) locked_group: u8` on `KmsCore`, init `0` in `new` and `for_tests`.
- [ ] **Step 2: Failing test (core state group bits)** — set `core.locked_group = 1`, assert `serialize_modifiers()` has bits 13-14 == 1, i.e. `result & 0x6000 == 0x2000`. (Add a small test that constructs the backend via `for_tests` and sets the field.)
- [ ] **Step 3: Implement core-state group bits** — at the end of `serialize_modifiers`, before returning: `mask |= (u16::from(self.core.locked_group) & 0x3) << 13;` (X11 `XkbGroupForCoreState`). Source from `locked_group`, NOT `xkb_state.serialize_layout` (authoritative-group rule).
- [ ] **Step 4: Failing test (XI2 group quartet)** — call `encode_xi2_device_event` with `state = 0x2000` (group 1) and assert the **4 bytes** of the `xXIGroupInfo` quartet at the `mod.rs:2007` site decode as `[base=0, latched=0, locked=1, effective=1]`, not all zeros, AND that the total event length is unchanged (no byte-count drift into the trailing buttons mask).
- [ ] **Step 5: Implement XI2 group quartet** — `xXIGroupInfo` is **4×CARD8 = 4 bytes total** (base/latched/locked/effective group), NOT 4×u32 (verified `/usr/include/X11/extensions/XI2proto.h:261,267`). The existing `out.extend_from_slice(&[0; 4])` at mod.rs:2007 is **already the correct size** — replace only its CONTENTS: `let g = u8::try_from((state >> 13) & 0x3).unwrap_or(0); out.extend_from_slice(&[0, 0, g, g]);` (base=0, latched=0, locked=g, effective=g — matches `cinnamon-xorg.xtrace:37115`'s `locked=effective=1, base=latched=0`). Do NOT change the byte count or you corrupt the buttons mask and everything after it. (Confirm the field order base/latched/locked/effective against XI2proto.h `xXIGroupInfo` when implementing.)
- [ ] **Step 6: Run** both tests — PASS. `cargo build --locked`, `cargo +nightly fmt`, `cargo clippy -p yserver -p yserver-protocol`.
- [ ] **Step 7: Commit** `feat(xkb): stamp active group into core+XI2 key events`.

### Task 1a-3: `write_xkb_state_notify` encoder

**Files:** Modify `crates/yserver-protocol/src/x11/mod.rs` (new encoder near the other XKB events).

- [ ] **Step 1: Failing wire test** — assert full byte layout per `XKBproto.h xkbStateNotify`: byte0=`xkb_event_base`(85), byte1=`xkbType`=2, seq@2, time@4, deviceID@8, then `mods`/`baseMods`/`latchedMods`/`lockedMods` (CARD8), `group`(CARD8)@13, `baseGroup`(INT16)@14, `latchedGroup`(INT16)@16, `lockedGroup`(CARD8)@18, `compatState`@19, `grabMods`@20, `compatGrabMods`@21, `lookupMods`@22, `compatLookupMods`@23, `ptrBtnState`(CARD16)@24, `changed`(CARD16)@26, `keycode`@28, `eventType`@29, `requestMajor`@30, `requestMinor`@31. All 32 bytes. (Confirm offsets by reading the struct in XKBproto.h — the byte positions above are from the field order; verify CARD8-vs-INT16 widths shift them.)
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement `write_xkb_state_notify`** — params `(writer, byte_order, sequence, xkb_event_base, device_id, group: u8, changed: u16, request_major: u8, request_minor: u8)`. Set `group`, `lockedGroup`, `baseGroup`/`latchedGroup`=0, `changed`, `requestMajor`/`requestMinor`. For a `LatchLockState` group lock use `changed = XkbGroupStateMask(0x10) | XkbGroupLockMask(0x80) = 0x90` (**deliberate Phase-1 approximation, codex nice-to-have:** Xorg's state machinery reaches `0x1190` by also setting `XkbCompatStateMask(0x100)|XkbCompatLookupModsMask(0x1000)`; `0x90` is the group-relevant minimum that drives the client's group re-eval — note the `0x1190` trace value in a comment and revisit in Phase 2 if a client needs the compat bits), `request_major=136, request_minor=5`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(xkb): add XkbStateNotify wire encoder`.

### Task 1a-4: `LatchLockState(5)` → set `locked_group` + broadcast `StateNotify`

**Files:** Modify `crates/yserver-core/src/backend/trait_def.rs` (add `set_locked_group(&mut self, group: u8)` default no-op + `current_group(&self) -> u8` default 0), `crates/yserver/src/kms/v2/backend.rs` (impls: set/read `self.core.locked_group`), `crates/yserver-core/src/core_loop/process_request.rs` (`handle_xkb_request` minor-5 branch).

- [ ] **Step 1: Failing test (parse + apply)** — unit-test a `parse_latch_lock_group(body) -> Option<u8>` helper: body per `xkbLatchLockStateReq` (after 4-byte header): `deviceSpec@0..2, affectModLocks@2, modLocks@3, lockGroup(BOOL)@4, groupLock@5, ...`. Feed the exact bytes from `cinnamon-xorg.xtrace:36077` (`00 01 00 00 01 01 00 00 00 00 00 00`) → expect `Some(1)` (lockGroup=1 → lock to groupLock=1). Feed `lockGroup=0` → `None`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** — add `parse_latch_lock_group` (in `xkb_layout.rs` or a helper). In `handle_xkb_request`, add a `minor == 5` branch (alongside the existing `minor == 1`): if `parse_latch_lock_group(body)` is `Some(g)` and `g != backend.current_group()`, call `backend.set_locked_group(g)`, then broadcast `XkbStateNotify(group=g, changed=0x90, reqMajor=136, reqMinor=5)` to `StateNotify` subscribers (reuse the A5 `subscribers`/fanout pattern; xkb_event_base from `backend.xkb_info()`). The backend's `xkb_proxy` minor 5 stays `None` (no reply; LatchLockState is void). KMS `set_locked_group` sets `self.core.locked_group = g`; `current_group` returns it.
- [ ] **Step 4: Run** — PASS; `cargo build`.
- [ ] **Step 5: Commit** `feat(xkb): LatchLockState switches active group + XkbStateNotify`.

> **Subscriber caveat:** `handle_xkb_request` minor-1 records `selectAll & !clear` and ignores `affectWhich`/details. `libxkbcommon-x11` selects `StateNotify` via the detail path → may be dropped. If the HW smoke (1a-5) shows the indicator/typing not updating for a client that selected by detail, widen the minor-1 parser to also record events present in `affectWhich` with a non-zero detail (the D2b work from the old plan). Try the simple path first; widen only if the capture shows it's needed.

### Task 1a-5: HW smoke — group switch with a startup multi-group map

- [ ] On `air`: `yserver -layout "us,de"` (or `XKB_DEFAULT_LAYOUT=us,de`), open a text editor, type Y → expect `y` (group 0). Trigger a group lock to group 1 (e.g. `setxkbmap -layout us,de` is already loaded; use a tool that issues `XkbLatchLockState` — the Cinnamon applet, or `xkblayout-state set 1`, or a tiny xcb snippet). Type Y → expect **z**. Capture an xtrace; confirm yserver now emits `XkbStateNotify` and stamps `effective_group=1`. Record evidence.

---

# PHASE 1b — `GetKbdByName(23)`: load the multi-group map at runtime (the real Cinnamon trigger)

### Task 1b-1: parse the `symbols` component into a layout list

**Files:** `crates/yserver/src/kms/xkb.rs` or a new `kms/kcgst.rs` (pure parser + tests).

- [ ] **Step 1: Failing tests** — `parse_symbols_layouts("pc+us+de:2+us:3+inet(evdev)") -> "us,de,us"` (group-ordered layout list); skips non-layout extras `pc`, `inet(evdev)`, `grp:*`, `compose:*`; handles `layout(variant):N` → variant captured per group; `:N` group suffix orders the list (1-based → slot N). Add a case with a variant, e.g. `"pc+us+gr(polytonic):2+inet(evdev)"` → layouts `us,gr` variants `,polytonic`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement — FAIL CLOSED (codex must-fix).** split on `+`; for each segment strip a trailing `:N` (group index, 1-based) and an optional `(variant)`; the bare token before `(`/`:` is the layout candidate; drop the known non-layout extras (`pc`, `pc104`, `pc105`, anything starting `inet`/`grp`/`compose`/`lv`/`terminate`/`kpdl`/`capslock`/`level`). **Crucial:** a candidate that is neither a recognized 2-letter-ish layout code nor a known extra is AMBIGUOUS → **return `None` (abort the runtime load, keep the current keymap)** rather than guessing — a silently-wrong layout is worse than no switch. Order layouts by their `:N` (1-based slot; default sequential). Return `Some((layout_csv, variant_csv))` only when every segment was classified confidently. Document that this is a narrow KcCGST→RMLVO heuristic for the desktop-switch path, NOT general inversion (codex re-plan #3). Add a test for an unknown segment → `None`.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(xkb): parse XKB symbols component into a layout list`.

### Task 1b-2: backend `load_keymap_by_components` (recompile multi-group)

**Files:** `crates/yserver-core/src/backend/trait_def.rs` (trait method + result enum, default `Failed`), `crates/yserver/src/kms/v2/backend.rs` (impl).

> **Richer result (codex must-fix #4):** `recompile_keymap` returns `None` for BOTH "unchanged" and "compile-failed", and the symbols parse can also fail-closed. But `GetKbdByName` must report `loaded=TRUE` on a successful load **even if the map is unchanged** (Xorg `xkb.c:6104,6113`). So the three cases must be distinguishable — don't collapse them into `Option`.

- [ ] **Step 1: Failing test** — `backend.load_keymap_by_components("pc+us+de:2+us:3+inet(evdev)")` returns `KeymapLoad::Loaded { min, max, changed: true }`; the live keymap now has ≥2 groups for keycode 29 with group-1 sym `z`. A second identical call returns `KeymapLoad::Loaded { changed: false, .. }` (still a successful load). An unparseable symbols string returns `KeymapLoad::Failed` and leaves the keymap unchanged.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** — define `enum KeymapLoad { Failed, Loaded { min: u8, max: u8, changed: bool } }`. `load_keymap_by_components(&mut self, symbols: &str) -> KeymapLoad`: parse via 1b-1; on parse `None` (fail-closed) → `Failed` (keep current keymap). Otherwise build `XkbRmlvo{ rules:"evdev", model:"pc105", layout, variant, options:None }`; call `recompile_keymap` (returns `Some((min,max))` on a real change, `None` if unchanged-or-compile-fail — so distinguish: if the requested RMLVO equals the current one → `Loaded{changed:false}` with the current min/max; if compile failed → `Failed`; else `Loaded{changed:true}`). To tell "unchanged" from "compile-fail" cleanly, compare the requested `XkbRmlvo` to `self.core.xkb_rmlvo` before recompiling. Reset `self.core.locked_group = 0` on `changed:true`. Add the unparseable→`Failed` test.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(xkb): load multi-group keymap from GetKbdByName components`.

### Task 1b-3: `reply_get_kbd_by_name` (valid reply) + generalize NewKeyboardNotify + wire minor 23

**Files:** `crates/yserver/src/kms/xkb.rs` (new `reply_get_kbd_by_name`), `crates/yserver-protocol/src/x11/mod.rs` (generalize `write_xkb_new_keyboard_notify`), `crates/yserver/src/kms/v2/backend.rs` (xkb_proxy minor 23 builds the reply), `crates/yserver-core/src/core_loop/process_request.rs` (broadcast NewKeyboardNotify after a load).

> **Reply contract — corrected per codex must-fixes #1+#2 (rounds 1+2).** This is NOT "header + stripped section bodies." Match how Xorg's `ProcXkbGetKbdByName` builds it (`xserver xkb/xkb.c:6090-6272`, mask conversion `xkbfmisc.c:421-430`):
> - The request carries `want`/`need` component masks (captured Cinnamon: `want=0x00ff`, `need=0x00bf` — `cinnamon-xorg.xtrace:6201`). **Decode them correctly:** `need=0xbf` does NOT include Geometry; it DOES include `OtherNames`. The symbols component maps to BOTH `ClientSymbols` and `ServerSymbols` bits. Any nonzero `found` implies `OtherNames` (Xorg's conversion).
> - **Two distinct header fields (do not conflate — codex):** `loaded` is the BOOL load-success flag; **`found` is a COMPONENT MASK** of what was actually located/loaded (Xorg fills it from `XkbDDXLoadKeymapByNames`, `ddxLoad.c:396`, NOT from a boolean). `reported` is the component mask actually embedded in the reply body. `loaded != found != reported`.
> - The reply is the fixed `xkbGetKbdByNameReply` header (`XKBproto.h:911`) followed by **full nested reply blocks** — each block is the component's FULL standalone reply, header included: an embedded `GetMap` block is a complete `xkbGetMapReply` (its header is **40 bytes**, `XKBproto.h:322-355`, `xkb.c:1418/1458`), then its body; likewise `GetCompatMap`/`GetIndicatorMap`/`GetNames`/`GetGeometry` blocks. NOT bare section bodies, NOT 32-byte headers.
> - **Golden vector:** header fields + each nested block's framing/bytes are locked against `cinnamon-xorg.xtrace:6202` + `xkb.c`, not invented.

- [ ] **Step 1: Build embeddable nested reply blocks.** Each embedded block is the component's complete reply (full header + body), reused for both the standalone request AND embedding. For `GetMap`: factor `reply_get_map` (`xkb.rs:289`) so it can emit the full `xkbGetMapReply` (40-byte header + body) usable standalone or embedded — confirm the 40-byte header size against `XKBproto.h:322-355` (do NOT assume 32). Same for `GetNames` (`xkb.rs:565`). Reuse the existing empty `reply_get_compat_map`; add minimal `GetIndicatorMap` + `GetGeometry` blocks. Substantive work — "parameterized full nested reply blobs," not a light extraction.
- [ ] **Step 2: Failing test** — `reply_get_kbd_by_name(&keymap, &rmlvo, want, need, load: KeymapLoad, intern)` header per `xkbGetKbdByNameReply`: `loaded`(BOOL) from `KeymapLoad::Loaded`; **`found`(CARD16) = component mask of what we located** (the components we can supply intersected with want|need, with `OtherNames` set when nonzero, symbols→client+server bits); `reported`(CARD16) = mask actually embedded in the body; `minKeyCode/maxKeyCode` match the keymap; `length` matches the appended nested blocks. Assert each header field by offset, and assert `found`/`reported` are masks (not 0/1) and `loaded` is the boolean.
- [ ] **Step 3: Implement the reply** — compute `found`/`reported` via the `xkbfmisc.c:421-430` conversion (symbols→Client|Server, nonzero⇒OtherNames; `need=0xbf` ⇒ Types|CompatMap|Symbols(both)|IndicatorMap|KeyNames|OtherNames, NO Geometry). Embed the full nested reply block for each `reported` component. Cross-check the whole reply against `cinnamon-xorg.xtrace:6202`. (Do NOT use the earlier-mistaken narrow `0x25`.)
- [ ] **Step 4: Generalize `write_xkb_new_keyboard_notify`** — add `request_major: u8, request_minor: u8, changed: u16` params (default callers pass `0,0,0x0001` to preserve the property-shim behavior; the GetKbdByName path passes `136, 23, 0x0003`). Update the A5 caller + its tests accordingly (this is a salvage edit — verify the A5 test still asserts the old values via the new params).
- [ ] **Step 5: Wire minor 23 in `handle_xkb_request`** — on `minor == 23`: parse the request's `want`/`need`/`load` flag + the component-name strings; if `load`, call `backend.load_keymap_by_components(symbols) -> KeymapLoad`; build the `reply_get_kbd_by_name` reply passing the `KeymapLoad` (so `loaded`=`matches!(Loaded)`, `found`=the located-components mask, even when `changed:false`). Then **on `KeymapLoad::Loaded` (changed OR unchanged — Xorg notifies on any successful load) broadcast `XkbNewKeyboardNotify(reqMajor=136, reqMinor=23, changed)`** to all clients (per-device like Xorg; single core keyboard for now). On `KeymapLoad::Failed`, return a valid reply with `loaded=0` (keep current keymap), emit no notify. Replace the `reply_minimal(23)` stub.
  - **Deliberate Phase-1 approximation (codex nice-to-have):** `NewKeyboardNotify.changed` is hardcoded `0x0003` (Keycodes|Geometry) to match the captured Cinnamon load; Xorg sets the Geometry bit only conditionally. Note this as overfit-but-acceptable for Phase 1; refine to conditional in Phase 2 if a client misbehaves.
- [ ] **Step 6: Run** unit tests — PASS; `cargo build`.
- [ ] **Step 7: Commit** `feat(xkb): real GetKbdByName load + NewKeyboardNotify (drop stub)`.

### Task 1b-4: HW smoke — the real Cinnamon switch

- [ ] On `air`/Cinnamon: type Y (→`y`), switch to German via the Cinnamon UI, type Y (→ **z**), in an app opened before AND after the switch. Capture an xtrace and diff against `cinnamon-xorg.xtrace`: confirm yserver's `GetKbdByName` reply is non-stub (`loaded=1`), a `NewKeyboardNotify` is broadcast, the `LatchLockState` produces a `StateNotify`, and post-switch KeyPress carries `effective_group=1`. Record evidence (this is the acceptance gate for the whole feature).

---

# PHASE 2 — Deferred polish (after y→z works)
- **AltGr / 4-level key types** (old plan Part C): real `FOUR_LEVEL` types in GetMap+GetNames so be/fr/de AltGr symbols resolve. Needed for full single-layout `be`; not for the us↔de group switch.
- **Property-hook demotion**: update `xkb_layout.rs` docs to describe it as a `setxkbmap`-CLI compatibility shim; keep its recompile but stop describing MappingNotify/MapNotify as the runtime mechanism. (Doc-only; verify setxkbmap-CLI still benefits, else consider removing.)
- **XI2 group quartet fidelity** + server-side `xkb_state` group consistency (server-side keysym lookups for grabs use group 0 while clients see the locked group — note + revisit if a grab/accelerator bug appears).
- **`XkbSelectEvents` detail-mask parsing** (old plan D2b) if 1a-5/1b-4 show a subscriber miss.
- **GetNames `groupNames`** for multi-group (old plan D1b).

---

# Finalization
### F1: Full gate
- [ ] `cargo +nightly fmt`; `cargo clippy --workspace` (plain; new warnings only); `cargo test --workspace --locked` (all green).

### F2: HW acceptance
- [ ] Phase 1b-4 smoke green on `air`/Cinnamon (visible y→z), evidence in the PR.

### F3: Integration
- [ ] PR (master branch-protected — `project_master_protected`); body includes the smoke evidence + the `cinnamon-xorg.xtrace` diff. Draft PR text for the user's approval (never publish in their name unprompted).

---

## Self-Review notes
- **Root-cause grounded in wire**, not theory: the trigger is `GetKbdByName(23)`+`LatchLockState(5)`, proven by `cinnamon-xorg.xtrace` (works) vs `cinnamon.xtrace` (yserver stubs both). The old plan's `_XKB_RULES_NAMES` hook never fired for Cinnamon.
- **The load-bearing step is group-stamping** (1a-2); everything else (multi-group map, GetKbdByName, LatchLockState) exists to make group 1 reachable and switchable.
- **Authoritative `locked_group`** avoids the master-unsafe `update_mask`; `xkb_state` stays for key-driven mods. Server-side keysym lookups using group 0 is a known Phase-2 caveat.
- **Events:** only `NewKeyboardNotify`(load) + `StateNotify`(lock), matching Xorg; no MappingNotify/MapNotify on the primary path.
- **Phasing:** 1a is HW-provable with a startup multi-group map (no GetKbdByName), de-risking the group engine before the fiddly 1b reply assembly.
- **Open risks flagged inline:** StateNotify subscriber detail-mask gap (widen minor-1 parser if 1a-5 shows a miss); the `GetKbdByName` nested-block framing (locked from the capture during 1b-3).
- **Codex v2-review applied (round 1):** (1) XI2 group quartet is `4×u8` (4 bytes) not `4×u32` — existing `[0;4]` already correctly sized, contents only (Task 1a-2). (2) `GetKbdByName` reply rewritten to honor `want`/`need` + nested reply blocks (Task 1b-3). (3) symbols parser fail-closed (Tasks 1b-1/1b-2). (4) `grp:` shortcut switching deferred to Phase 2. Confirmed: Phase 1a provable standalone; offsets correct; no `serialize_modifiers` bypass.
- **Codex v2-review applied (round 2) — Phase 1b reply contract:** (1) `found` is a COMPONENT MASK (from `XkbDDXLoadKeymapByNames`), `loaded` is the BOOL — separated. (2) mask decode corrected: `need=0xbf` has OtherNames, NOT Geometry; symbols→Client|Server bits; nonzero found⇒OtherNames (`xkbfmisc.c:421-430`). (3) embedded `GetMap` block uses the FULL 40-byte `xkbGetMapReply` header, not 32. (4) richer `KeymapLoad{Failed, Loaded{min,max,changed}}` result so parse-fail / unchanged-success / changed-success are distinct (Xorg reports `loaded=TRUE` on unchanged). Two deliberate Phase-1 approximations noted: `StateNotify.changed=0x90` (Xorg `0x1190`) and `NewKeyboardNotify.changed=0x0003` (Xorg conditional Geometry). **Codex round-2 verdict: Phase 1a READY to implement; Phase 1b ready after these 4 (now applied).**
