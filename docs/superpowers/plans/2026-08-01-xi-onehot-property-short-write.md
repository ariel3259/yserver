# XInput one-hot property short-write acceptance — implementation plan

**Goal:** implement
[`2026-08-01-xi-onehot-property-short-write-design.md`](../specs/2026-08-01-xi-onehot-property-short-write-design.md)
— accept a multi-slot XInput property write that is shorter than the
declared width, zero-filling it to that width, so
`mate-settings-daemon`'s two-item `libinput Accel Profile Enabled`
write stops being silently rejected and MATE's "flat" acceleration
setting actually reaches libinput.

**Branch:** `fix-accel-profile-prop-width` (off `master`; deliberately
NOT on `present-deferred-supersession`, which is a different subsystem
and is queued for its own squash-merge).

**Tech stack:** Rust; `yserver-core` (`xinput/libinput_props.rs`,
`core_loop/process_request.rs`).

---

## Task 1: Relax `validate_value` + add `normalize_value`

**Files:**
- Modify: `crates/yserver-core/src/xinput/libinput_props.rs`

- [ ] **Step 1: Failing tests** in that file's test module, per spec
  §Validation:
  - `validate_value(OneHotOrNone { n: 3 }, 8, &[0, 1])` → `Ok`.
  - `validate_value(OneHotOrNone { n: 3 }, 8, &[0, 1, 0, 0])` → `Invalid`
    (longer than `n` still rejected).
  - `validate_value(OneHot { n: 3 }, 8, &[0, 0])` → `Invalid`
    (cardinality 0 unchanged); `&[0, 1]` → `Ok`.
  - `validate_value(OneHotOrNone { n: 3 }, 8, &[1, 1])` → `Invalid`.
  - `format != 8` still rejected at any length.
  - `BitFlags { n: 3 }` accepts a 1-byte value.
  - `normalize_value(OneHotOrNone { n: 3 }, &[0, 1])` → `[0, 1, 0]`;
    a full-width input is returned **borrowed** (assert no allocation via
    `matches!(.., Cow::Borrowed(_))`); `Scalar` input untouched.
- [ ] **Step 2:** Run to verify they fail.
- [ ] **Step 3: Implement.** In `validate_value`, change the three
  multi-slot arms from `data.len() != usize::from(n)` to
  `data.len() > usize::from(n)`; cardinality counting is unchanged (it
  already folds over whatever bytes are present, and absent trailing
  slots are implicitly zero). Add:

```rust
/// Zero-pad a short multi-slot value to the descriptor's declared width
/// so every downstream consumer (decoders, the stored property) always
/// sees exactly `n` bytes. Caller MUST have run `validate_value` first.
pub fn normalize_value(kind: ValueKind, data: &[u8]) -> std::borrow::Cow<'_, [u8]>
```

  returning `Cow::Borrowed(data)` for `Scalar` and for already-full-width
  multi-slot values, and a zero-padded owned copy otherwise.
- [ ] **Step 4:** Tests → PASS. `cargo clippy --all-targets -- -D warnings`.
- [ ] **Step 5:** Commit:
  `feat(xinput): accept short one-hot property writes (zero-filled)`.

---

## Task 2: Wire normalisation into the dispatch pipeline

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs`
  (`dispatch_change_property`)

- [ ] **Step 1: Failing test** beside the existing property-dispatch
  cases: a 2-item `XIChangeProperty` (minor 57) to
  `libinput Accel Profile Enabled` with `[0, 1]` must (a) return no X
  error, (b) reach the backend as
  `DeviceConfigChange::AccelProfile(Some(1))`, and (c) leave the stored
  property **exactly 3 bytes** (`[0, 1, 0]`). Assert the same for the
  XI1 path (`XChangeDeviceProperty`, minor 37) since both arms share
  this helper.
- [ ] **Step 2:** Insert `normalize_value` between `validate_value` and
  `decode_change`, and pass the normalised bytes to **both**
  `decode_change` and the final `apply_change_property` commit — so the
  decoder and the stored value agree. Normalise on
  `XI_PROP_MODE_REPLACE` only; leave Prepend/Append on their current
  path untouched (spec §Scope decisions).
- [ ] **Step 3:** Confirm the pre-existing width tests (e.g.
  `accel_profile_enabled_is_three_wide`) still assert the **read**
  width and were not weakened — they must stay green unmodified. If one
  of them fails, that is a signal the read side changed by mistake:
  stop and report rather than editing the assertion.
- [ ] **Step 4:** `cargo test -p yserver-core`; clippy CI-exact.
- [ ] **Step 5:** Commit:
  `feat(xinput): normalize short property writes before decode and commit`.

---

## Task 3: Verification

- [ ] `cargo clippy --all-targets -- -D warnings` (workspace, CI-exact).
- [ ] `cargo test --workspace`.
- [ ] (No `cargo +nightly fmt` — no nightly toolchain on this box; CI's
  fmt check stays unverifiable locally, as on the Present branch.)

---

## Task 4: Hardware/manual gate (user)

- [ ] With MATE configured `accel-profile 'flat'`, restart the session
  (so msd re-applies from scratch, with no manual `xinput set-prop` in
  the way) and confirm
  `xinput list-props <dev> | grep "Accel Profile Enabled ("` reads
  `0, 1, 0`, and that pointer feel changes.
- [ ] Sanity: the other libinput settings MATE manages (middle
  emulation, left-handed, natural scrolling) still apply — the
  normalisation touches their shared code path.
- [ ] Squash-merge with user confirmation (AGENTS.md).

---

## Self-Review

- Spec §Design (accept `len <= n`, zero-fill, cardinality unchanged) →
  Task 1. §"Normalisation happens once" + pipeline order → Task 2.
  §Scope decisions (Replace-only, reads stay 3-wide, custom still
  rejected, general not special-cased) → Tasks 1–2 plus the Task 2
  Step 3 guard against weakening the read-width tests. §Validation
  bullets all appear as failing-test steps; the manual gate → Task 4.
- The custom-slot rejection is untouched precisely because
  normalisation runs *before* `decode_change`: a 2-byte write pads to
  `[x, y, 0]`, whose slot 2 is zero, so it can never be mistaken for a
  custom selection.
