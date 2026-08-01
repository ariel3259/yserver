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

- [x] **Step 1: Failing tests** in that file's test module, per spec
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
- [x] **Step 2:** Run to verify they fail.
- [x] **Step 3: Implement.** In `validate_value`, change the three
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
- [x] **Step 4:** Tests → PASS. `cargo clippy --all-targets -- -D warnings`.
- [x] **Step 5:** Commit:
  `feat(xinput): accept short one-hot property writes (zero-filled)`.

---

## Task 1b: Reject empty multi-slot writes (review round S3)

**Files:**
- Modify: `crates/yserver-core/src/xinput/libinput_props.rs`

Task 1 landed the `len <= n` relaxation but with no lower bound, which
makes `num_items = 0` a legal, meaning-bearing write (spec §Design,
"Empty writes stay rejected"): `[]` would normalise to all-zero and
reprogram the device — `ScrollMethod(None)` turns scrolling off,
`SendEvents(0)` re-enables a device the user disabled. All three were
`BadValue` before Task 1; restore that.

- [x] **Step 1: Failing tests** — `validate_value` rejects `&[]` for
  `OneHot { n }`, `OneHotOrNone { n }` and `BitFlags { n }`.
  (`OneHot` already rejects it via cardinality; assert it anyway so the
  rule is pinned per-kind.)
- [x] **Step 2:** Add `data.is_empty()` to the reject condition in the
  three multi-slot arms.
- [x] **Step 3:** Tests → PASS; clippy CI-exact. Commit:
  `fix(xinput): reject empty multi-slot property writes`.

---

## Task 2: Pin `format`/type to the descriptor (review round B1 — server crash)

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs`
  (`dispatch_change_property`, ~:19118-19145)

`validate_value` is called with the **request's** `format`, not
`desc.format`, and its `Scalar` arm derives the expected length from it
(`expected = format / 8`). So `XIChangeProperty(format=8, num_items=1)`
against `libinput Accel Speed` (`Scalar`, descriptor format **32**)
validates OK with one byte and then `decode_change` runs
`float32(&[0])` → `b[1]` → **index out of bounds → panic**. There is no
`catch_unwind` or panic hook in the workspace and request handling is
not per-client isolated: **any client on the display can kill the
server and every other session with one request.** Verified by reading
the code; do NOT reproduce it against a live desktop.

- [x] **Step 1: Failing tests** driving the **real dispatch path** (not
  `validate_value` in isolation, or they prove nothing): `format = 8,
  num_items = 1` to `libinput Accel Speed` → `BadMatch`, no panic;
  `format = 16` likewise; `format = 8` to
  `libinput Button Scrolling Button` (the `card32` decoder) → `BadMatch`.
  Cover both wire arms (XI2 minor 57 and XI1 minor 37).
- [x] **Step 2:** Immediately after the ReadOnly→BadAccess check, reject
  `format != desc.format` with `PropertyDispatchError::BadMatch`
  (matching `xf86-input-libinput`). Do the same for `type_atom` vs the
  descriptor's declared type — check how `XiValType` maps to a type atom
  in `crates/yserver-core/src/xinput/`; if there is no existing helper,
  add one rather than hardcoding atom numbers at the call site.
- [x] **Step 3:** Tests → PASS; clippy CI-exact. Commit:
  `fix(xinput): reject property writes whose format does not match the descriptor`
  (this one is a crash fix — keep it as its own commit so it can be
  cherry-picked to master independently of the accel work).

---

## Task 3: Merge-then-validate + normalisation in the dispatch pipeline

**Files:**
- Modify: `crates/yserver-core/src/core_loop/process_request.rs`
  (`dispatch_change_property`)

Supersedes the original "normalise on Replace only, leave Prepend/Append
as-is" step: leaving them as-is is not available, because Task 1's
relaxation makes a short *fragment* pass validation. See spec §"Two
prerequisites", item (2) — `Append [1]` would decode to
`AccelProfile(Some(0))` and silently reprogram libinput, and
delete-then-append would store a 1-item property that permanently
breaks msd's `nitems_ret >= 2` precondition.

- [x] **Step 1: Failing tests** beside the existing property-dispatch
  cases:
  - 2-item `XIChangeProperty` (minor 57) to
    `libinput Accel Profile Enabled` with `[0, 1]`: no X error, reaches
    the backend as `DeviceConfigChange::AccelProfile(Some(1))`, and
    leaves the stored property **exactly 3 bytes** `[0, 1, 0]`. Assert
    the stored `XiProperty.data` **directly**, not through a
    GetProperty re-read — a re-read can mask a raw-bytes commit.
    Same assertions on the XI1 arm (minor 37).
  - `Append [1]` onto a full-width `Accel Profile Enabled` → BadValue
    (merged length 4 > n), stored property and libinput config both
    untouched.
  - `Append [0, 1, 0]` onto a **deleted** (absent) property behaves
    exactly like Replace and stores 3 bytes.
- [x] **Step 2: Implement.** Compute the merged value by mode first
  (`Replace → data`, `Append → existing ++ data`,
  `Prepend → data ++ existing`), then run
  `validate_value` → `normalize_value` → `decode_change` on the merged
  value, and commit the normalised merged value.
  **Implementation trap (review S5):** `validate_value` and
  `decode_change` sit inside the `if let Some(name) … Some(desc)` block
  while the `apply_change_property` commit is *outside* it. A naive
  in-block `normalize_value` compiles, decodes normalised, and commits
  raw — exactly the "decoder and stored value disagree" failure the
  spec forbids. Hoist the value above the `if let`
  (`let mut value: Cow<[u8]> = Cow::Borrowed(data);` … commit `&value`),
  and commit it in Replace mode since the merge already happened.
- [x] **Step 3:** Confirm the pre-existing descriptor tests (e.g.
  `accel_profile_enabled_is_three_wide`) stay green **unmodified**. If
  one fails, the read side changed by mistake: stop and report rather
  than editing the assertion.
- [x] **Step 4:** Add the N6 assertion the existing tests are missing:
  a seeded `libinput Accel Profile Enabled` is exactly 3 bytes (the
  seeded width comes from a hardcoded `encode_onehot(..., 3)` in
  `crates/yserver-core/src/xinput/mod.rs`, independent of `desc.kind`'s
  `n`, and is what msd's `nitems_ret >= 2` precondition depends on).
- [x] **Step 5:** `cargo test -p yserver-core`; clippy CI-exact.
- [x] **Step 6:** Commit:
  `feat(xinput): merge-then-validate property writes and normalize before commit`.

---

## Task 3b: Close the Task 1 test gaps (review round S4)

**Files:**
- Modify: `crates/yserver-core/src/xinput/libinput_props.rs` (tests only)

Task 1's tests do not cover the spec's Validation list: the
`normalize_value` → `decode_change` **composition** is asserted nowhere,
and `decode_change_maps_each_binding` never produces
`AccelProfile(Some(1))` — *flat*, the single value this whole change
exists to deliver.

- [x] **Step 1:** Add: composition on `[0, 1]` → `AccelProfile(Some(1))`;
  composition on `[0, 0]` → `AccelProfile(None)`; `normalize_value`
  padding for `OneHot` and `BitFlags` (currently only `OneHotOrNone` and
  `Scalar` are covered).
- [x] **Step 2:** Tests → PASS; clippy. Commit:
  `test(xinput): pin the short-write to flat-profile composition`.

---

## Task 4: Verification

- [x] `cargo clippy --all-targets -- -D warnings` (workspace, CI-exact).
- [x] `cargo test --workspace`.
- [x] (No `cargo +nightly fmt` — no nightly toolchain on this box; CI's
  fmt check stays unverifiable locally, as on the Present branch.)

---

## Task 5: Hardware/manual gate (user)

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

- Spec §Design (accept `1 <= len <= n`, zero-fill, cardinality
  unchanged) → Tasks 1 + 1b. §"Two prerequisites" item (1), the format
  gate → Task 2; item (2), merge-then-validate → Task 3.
  §Scope decisions (reads stay 3-wide, custom still rejected, general
  not special-cased) → Tasks 1–3 plus the Task 3 Step 3 guard against
  weakening the descriptor tests and Step 4's seeded-width assertion.
  §Validation bullets all appear as failing-test steps; the manual gate
  → Task 5.
- The custom-slot rejection is untouched precisely because
  normalisation runs *before* `decode_change`: a 2-byte write pads to
  `[x, y, 0]`, whose slot 2 is zero, so it can never be mistaken for a
  custom selection. Post-merge this still holds — the merge happens
  before normalisation, so decode always sees exactly `n` bytes.
- **Task ordering is deliberate.** Task 1 (already landed) opened two
  holes that Tasks 1b/2/3 close; the branch is not shippable at any
  commit between them. Task 2 is kept as its own commit because it
  fixes a client-reachable server crash that predates this work and is
  worth cherry-picking to master on its own.
- **Review round:** this plan was revised after an Opus adversarial
  round on the spec + the Task 1 commit (findings B1, B2, S3, S4, S5,
  N6, N7 — all folded in above). The reviewer's "checked, clean" list
  covers zero-fill vs Replace semantics, custom-slot unreachability,
  cardinality with implicit-zero tails, the seeding path,
  `normalize_value` itself, generality across the descriptor table for
  non-empty short writes, and both wire arms sharing one pipeline — do
  not re-chase those.
