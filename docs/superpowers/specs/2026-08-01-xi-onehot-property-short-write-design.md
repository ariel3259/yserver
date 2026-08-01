# XInput one-hot property short-write acceptance — design

**Status:** proposed, 2026-08-01
**Scope:** `crates/yserver-core/src/xinput/libinput_props.rs` +
the shared property-change dispatch in
`crates/yserver-core/src/core_loop/process_request.rs`.

## Problem — measured on MATE, this box

Setting "flat" mouse acceleration in MATE has no effect on yserver: the
pointer keeps libinput's adaptive profile no matter what the desktop is
configured to.

Verified live (display `:7`, device id 4,
"HP, Inc HyperX Alloy Origins 65 Mouse"):

- `gsettings org.mate.peripherals-mouse accel-profile` = `'flat'`.
- `mate-settings-daemon` is running and its mouse plugin
  (`/usr/lib64/mate-settings-daemon/libmouse.so`) references
  `libinput Accel Profile Enabled` / `... Profiles Available`.
- Forcing a re-apply (toggling the gsetting) leaves the device property
  at `1, 0, 0` — adaptive. The write fails **silently**.
- msd's *other* libinput writes land correctly
  (`Middle Emulation Enabled = 1`, `Left Handed Enabled = 0` both match
  gsettings), so its plumbing works; the failure is specific to this
  property.
- By hand:
  `xinput set-prop 4 "libinput Accel Profile Enabled" 0 1` → **BadValue**;
  the same write with three values (`0 1 0`) succeeds and audibly changes
  pointer behaviour.

## Root cause — confirmed from the client's source

`mate-settings-daemon`, `plugins/mouse/msd-mouse-manager.c`,
`set_accel_profile_libinput`:

```c
change_property (device, "libinput Accel Profile Enabled", XA_INTEGER,
                 8, values, 2);
```

It writes **exactly two** items — the historical
`xf86-input-libinput` width (adaptive, flat); the third "custom" slot
only exists with libinput ≥ 1.23.

yserver declares the property `ValueKind::OneHotOrNone { n: 3 }`
(`libinput_props.rs`, descriptor table) and `validate_value` rejects
**any** length other than exactly 3:

```rust
ValueKind::OneHotOrNone { n } => {
    if format != 8 || data.len() != usize::from(n) {
        return Err(DeviceConfigError::Invalid);   // → BadValue
    }
    ...
}
```

so msd's two-byte write is rejected before it can reach libinput.

Two supporting details from the same client source, both verified:

- msd's precondition read is
  `get_property(..., "libinput Accel Profiles Available", XA_INTEGER, 8, 2)`
  and `get_property` accepts `nitems_ret >= nitems`. Our 3-wide
  *Available* therefore passes; msd does **not** bail early, it really
  does reach the write. Widening/narrowing *Available* is not part of
  the problem.
- the write is wrapped in `gdk_x11_display_error_trap_push` /
  `..._pop_ignored`, which is why the `BadValue` produces no message
  anywhere. The silence is the client swallowing the error, not a
  missing log on our side.

This is **not MATE-specific**: GNOME's `gsd-mouse-manager` shares this
code lineage, so any GNOME-family settings daemon hits the same wall.

## Design

**Accept a short write and zero-fill it to the declared width.**

`validate_value` changes for the three multi-slot kinds — `OneHot { n }`,
`OneHotOrNone { n }`, `BitFlags { n }` — from

> `data.len()` must equal `n`

to

> `data.len()` must be **≤** `n` (a longer write is still `Invalid`);
> missing trailing slots are treated as zero for the cardinality check.

`format != 8` stays a hard reject, and the cardinality rules are
unchanged: `OneHot` still requires exactly one non-zero slot (so a
short all-zero write is still rejected), `OneHotOrNone` still allows at
most one, `BitFlags` still accepts any pattern.

**Normalisation happens once, immediately after validation**, via a new
pure helper in `libinput_props.rs`:

```rust
pub fn normalize_value(kind: ValueKind, data: &[u8]) -> Cow<'_, [u8]>
```

which returns `data` untouched for `Scalar` and for already-full-width
values, and a zero-padded `n`-byte copy for a short multi-slot write.
The dispatch pipeline then runs **decode and commit on the normalised
bytes**, so:

- every existing decoder (`onehot_index`, the `bitmask` fold, the
  `data.get(2)` custom-slot rejection) keeps seeing exactly `n` bytes and
  needs no change at all;
- the stored property always reads back exactly `n` items, so a client
  that writes 2 does not silently shrink the property's advertised
  width for the next reader.

The pipeline order becomes: BadAtom → descriptor → ReadOnly→BadAccess →
`validate_value`→BadValue → **`normalize_value`** → `decode_change`→BadValue
→ `apply_device_config` → commit via `apply_change_property`.

### Scope decisions

- **Replace mode only.** Normalisation applies to
  `XI_PROP_MODE_REPLACE`. Prepend/Append on a fixed-width value property
  are already semantically odd (today they validate the incoming
  *fragment* and then concatenate, which can leave a property at a
  length other than `n`); that behaviour is left exactly as-is. Out of
  scope, noted so it is not mistaken for a regression introduced here.
- **Reads stay 3-wide.** `Accel Profile Enabled`,
  `... Enabled Default` and `Accel Profiles Available` keep
  `n: 3`. Clients reading with `>= 2` semantics (the whole GNOME/MATE
  lineage) are satisfied, and a modern client that wants the custom slot
  still sees it.
- **The custom slot stays rejected on write.** `decode_change`'s
  `data.get(2)` check is unchanged; the workspace libinput build cannot
  honour `AccelProfile::Custom`, so a 3-byte write selecting it remains
  `BadValue`.
- **General, not special-cased to accel.** The relaxation lives in
  `validate_value` and therefore covers every multi-slot descriptor. A
  narrower "only `Binding::AccelProfile`" carve-out would be a second
  code path to keep in sync for no benefit — the same client-vs-server
  width drift can hit any of these properties as libinput grows slots.

### Why deviating from Xorg here is correct

Xorg's driver is strict about size too, but its strictness is paired
with a width that matches what that same driver advertises. Ours
advertises 3 because we model libinput ≥ 1.23's profile list, while the
installed client ecosystem still writes 2. Accepting a **prefix** is a
strict superset of Xorg's accept-set: every write that works on Xorg
works here unchanged, and the writes we newly accept are exactly the
ones the ecosystem actually emits. Per AGENTS.md ("clients are tested
for 40+ years on Xorg") the goal is that those clients work — which a
byte-for-byte copy of Xorg's length check does not achieve on our
declared width.

## Validation

Unit (in `libinput_props.rs` tests unless noted):

- `validate_value` accepts a 2-byte value for `OneHotOrNone { n: 3 }`
  and still rejects a 4-byte one.
- `OneHot { n: 3 }` with a short all-zero value is still `Invalid`
  (cardinality unchanged); with `[0, 1]` it passes.
- `OneHotOrNone` short `[1, 1]` is still `Invalid` (two slots set).
- `format != 8` still rejected regardless of length.
- `normalize_value` zero-pads `[0, 1]` → `[0, 1, 0]` for `n: 3`, leaves
  a full-width value borrowed (no copy), and leaves `Scalar` untouched.
- `decode_change` on the **normalised** `[0, 1, 0]` yields
  `AccelProfile(Some(1))` (flat); `[0, 0]` normalised yields
  `AccelProfile(None)`; a 3-byte `[0, 0, 1]` (custom) is still rejected.
- Dispatch-level (in `process_request.rs` tests, mirroring the existing
  property-dispatch cases): a 2-item `XIChangeProperty` to
  `libinput Accel Profile Enabled` returns no error, reaches
  `apply_device_config` as `AccelProfile(Some(1))`, and leaves the
  stored property exactly 3 bytes long.
- The pre-existing per-descriptor width tests (e.g.
  `accel_profile_enabled_is_three_wide`) keep asserting the **read**
  width — they must not be weakened into asserting the write width.

Hardware/manual (the real gate): with MATE set to `accel-profile 'flat'`,
restart the session and confirm
`xinput list-props <dev> | grep "Accel Profile Enabled ("` reads
`0, 1, 0` without any manual `xinput set-prop`, and that pointer feel
changes accordingly.

## Out of scope

- Persisting the setting across server restarts: yserver has no
  `xorg.conf`-style `InputClass` config at all, so a client-applied
  property is runtime-only. Separate feature if wanted.
- Core `ChangePointerControl` / `xset m`: stored and reported by
  `GetPointerControl` but never applied to motion. That is exact parity
  with Xorg + libinput, where libinput owns acceleration. Not a bug,
  not touched here.
- Prepend/Append semantics on fixed-width value properties (see above).
