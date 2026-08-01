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

> `data.len()` must be **≥ 1 and ≤ `n`** (longer *and* empty are both
> `Invalid`); missing trailing slots are treated as zero for the
> cardinality check.

`format != 8` stays a hard reject, and the cardinality rules are
unchanged: `OneHot` still requires exactly one non-zero slot (so a
short all-zero write is still rejected), `OneHotOrNone` still allows at
most one, `BitFlags` still accepts any pattern.

**Empty writes stay rejected** (review round, S3). `num_items = 0` is
reachable on the wire, and with only an upper bound it would become a
legal, meaning-bearing write: `[]` would normalise to all-zero and
reprogram the device from a value the client never expressed —
`ScrollMethod(None)` turns scrolling **off**, `SendEvents(0)` **re-enables
a device the user disabled**. It is also the one prefix carrying zero
information, it is not in the ecosystem write-set this design justifies
itself by, and X11 says an empty Replace makes the property *empty* —
which is the opposite of storing `n` bytes. All three were `BadValue`
before; they stay `BadValue`.

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

### Two prerequisites the relaxation depends on (review round)

The design's core claim — "`validate_value` is the gate, so every
decoder may assume the byte count" — is **not currently true**. Both
gaps are fixed as part of this change, because the relaxation is
unsound without them and both live in the exact function this change
edits.

**(1) `format` must be pinned to the descriptor (B1 — server crash).**
`dispatch_change_property` validates with the *request's* `format`, not
`desc.format`, and `validate_value`'s `Scalar` arm derives its expected
length from it (`expected = format / 8`). So
`XIChangeProperty(format=8, num_items=1)` against `libinput Accel Speed`
(a `Scalar` **format 32** descriptor) passes validation with one byte,
then `decode_change` runs `float32(&[0])` → `b[1]` → **index out of
bounds → panic**. There is no `catch_unwind` or panic hook anywhere in
the workspace and request handling is not per-client isolated, so any
client on the display can kill the server and every other client's
session with a single request. Same shape via `card32` for
`libinput Button Scrolling Button`, and it fires before the device-node
check, so it does not even need a real device attached.

Fix: right after the ReadOnly check, reject `format != desc.format`
with **BadMatch** (matching `xf86-input-libinput`, which returns
BadMatch for a format/size/type mismatch). Do the same for
`type_atom` vs the descriptor's declared type.

**(2) Prepend/Append must merge before validating (B2).** The original
plan scoped these out as "left exactly as-is". That is not an available
option: the relaxation makes a **short fragment** pass `validate_value`,
so `Append [1]` to `Accel Profile Enabled` now decodes as
`AccelProfile(Some(0))` and silently reprograms libinput to Adaptive,
then concatenates to a 4-item stored value. Worse, `XIDeleteProperty`
on a libinput descriptor is deliberately allowed, so delete-then-append
inserts a **1-item** property — which fails msd's
`nitems_ret >= 2` precondition and permanently breaks the very client
this change exists to fix, until a device re-add re-seeds. Both were
`BadValue` before this change.

Fix (also deleting the scope-out): compute the **merged** value first —
`Replace → data`, `Append → existing ++ data`, `Prepend → data ++ existing`
— then run `validate_value` → `normalize_value` → `decode_change` on
that merged value and commit the normalised result. This is what
xserver does (`XIChangeDeviceProperty` builds `new_value` and hands
*that* to the driver's `SetProperty` handler, not the fragment), and it
makes appending onto an already-full-width value fail `len > n` with the
property untouched — Xorg's behaviour.

The pipeline order becomes: BadAtom → descriptor → ReadOnly→BadAccess →
**`format`/type vs descriptor→BadMatch** → **merge by mode** →
`validate_value`→BadValue → **`normalize_value`** →
`decode_change`→BadValue → `apply_device_config` → commit the
**normalised merged** value via `apply_change_property` (Replace mode,
since the merge already happened).

### Scope decisions
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

Note the parity is on the *accept-set*, not on error codes: this
pipeline answers `BadValue` (with `error_value = format`) for every
size/cardinality failure, where `xf86-input-libinput` answers
`BadMatch`. The new format/type gate above uses BadMatch, but the
pre-existing BadValue answers are left alone — harmless for the
gdk-trapped writes at issue, and changing them is a separate
compatibility question. Recorded so the deviation is deliberate rather
than assumed-parity. One knock-on of the relaxation: a 1-byte all-zero
write to `libinput Click Method Enabled` moves from `BadValue` to
`BadMatch` (it decodes to `ClickMethod(None)`, which the libinput layer
reports Unsupported).

## Validation

Unit (in `libinput_props.rs` tests unless noted):

- `validate_value` accepts a 2-byte value for `OneHotOrNone { n: 3 }`
  and still rejects a 4-byte one.
- `validate_value` rejects an **empty** value for all three multi-slot
  kinds (S3).
- `OneHot { n: 3 }` with a short all-zero value is still `Invalid`
  (cardinality unchanged); with `[0, 1]` it passes.
- `OneHotOrNone` short `[1, 1]` is still `Invalid` (two slots set).
- `format != 8` still rejected regardless of length.
- `normalize_value` zero-pads `[0, 1]` → `[0, 1, 0]` for `n: 3`, leaves
  a full-width value borrowed (no copy), leaves `Scalar` untouched, and
  pads `OneHot` and `BitFlags` too (all three kinds, not just
  `OneHotOrNone`).
- **Composition** (the headline vector, currently asserted nowhere):
  `normalize_value` then `decode_change` on `[0, 1]` yields
  `AccelProfile(Some(1))` — *flat*, the one value MATE needs, which no
  existing test produces; on `[0, 0]` yields `AccelProfile(None)`; a
  3-byte `[0, 0, 1]` (custom) is still rejected.
- **Format gate (B1):** `XIChangeProperty` with `format = 8` and one
  byte against `libinput Accel Speed` (descriptor format 32) returns
  **BadMatch** and does **not** panic. Same for `format = 16`, and for
  `libinput Button Scrolling Button` via the `card32` decoder. These are
  regression tests for a reachable server crash — they must drive the
  real dispatch path, not `validate_value` in isolation.
- **Merge-then-validate (B2):** `Append` of `[1]` onto a full-width
  `Accel Profile Enabled` yields BadValue (merged length 4 > n) and
  leaves both the stored property and the libinput config untouched;
  `Append` of `[0, 1, 0]` onto a **deleted** (absent) property behaves
  exactly like Replace and stores 3 bytes; no fragment path can reach
  `apply_device_config` with a value the merge would not produce.
- Dispatch-level (in `process_request.rs` tests, mirroring the existing
  property-dispatch cases): a 2-item `XIChangeProperty` to
  `libinput Accel Profile Enabled` returns no error, reaches
  `apply_device_config` as `AccelProfile(Some(1))`, and leaves the
  stored property exactly 3 bytes long.
- The pre-existing per-descriptor width tests (e.g.
  `accel_profile_enabled_is_three_wide`) keep asserting the descriptor's
  `ValueKind` — they must not be weakened into asserting the write width.
  Note they do **not** pin the *seeded* read width, which comes from a
  separate hardcoded `encode_onehot(..., 3)` literal in the seeding path
  (N6). Now that the write path no longer enforces `len == n`, those two
  constants are fully independent, so add one assertion that a seeded
  `Accel Profile Enabled` is exactly 3 bytes — that is what msd's
  `nitems_ret >= 2` precondition actually depends on.

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
