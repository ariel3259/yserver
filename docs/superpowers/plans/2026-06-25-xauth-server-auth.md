# Server-side X11 authorization (`-auth` / MIT-MAGIC-COOKIE-1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Honor `-auth FILE` by validating each client's `MIT-MAGIC-COOKIE-1` against the cookies in that file, rejecting mismatches exactly as X.Org does, while leaving the no-`-auth` case open (today's behavior).

**Architecture:** A new `AuthState` (in `yserver-core::core_loop::auth`) is built once from `LaunchOptions.auth_file`, shared via `Arc` to the per-client setup threads, and queried by a single `check()` call inserted into the handshake. Cookie file decoding is shared with the existing host-X11 client reader via a new `yserver-core::xauth` module. A latent bug in `write_setup_failed` (missing the 2-byte `length` field) is fixed first, since the reject path depends on it.

**Tech Stack:** Rust, standard library only (no new runtime deps). `proptest` is the only existing dev-dep; tests here use hand-encoded fixtures and temp files, no new dev-deps.

**Spec:** `docs/superpowers/specs/2026-06-25-xauth-server-auth-design.md`

---

## File Structure

- **Create** `crates/yserver-core/src/xauth.rs` — shared low-level Xau (`.Xauthority`) record decoder + `MIT_MAGIC_COOKIE` constant. One responsibility: turn bytes into `Vec<XauthRecord>`. Selection (by family/number, or "all MIT") stays at the call sites.
- **Create** `crates/yserver-core/src/core_loop/auth.rs` — `AuthState`, the reload state machine, constant-time compare, and verdict logic. One responsibility: "given a client's auth name+data, Allow or Reject(reason)".
- **Modify** `crates/yserver-protocol/src/x11/mod.rs` — fix `write_setup_failed` (add the `length` field).
- **Modify** `crates/yserver-core/src/host_x11/pump.rs` — use `crate::xauth` instead of its own private decoder (no behavior change).
- **Modify** `crates/yserver-core/src/core_loop/setup_thread.rs` — thread `Arc<AuthState>` into `spawn`/`run_setup`; insert the `check()` call.
- **Modify** `crates/yserver-core/src/core_loop/run.rs` — `run_core` and `accept_pending` carry the `Arc<AuthState>`.
- **Modify** `crates/yserver-core/src/core_loop/mod.rs` — `pub mod auth;`.
- **Modify** `crates/yserver-core/src/lib.rs` — `pub mod xauth;`.
- **Modify** `crates/yserver/src/lib.rs` — build `AuthState` from `opts.auth_file`, pass to `run_core`.

---

## Task 1: Fix `write_setup_failed` (add the missing `length` field)

The X11 connection-setup *failed* reply prefix is 8 bytes: `success(1)`, `lengthReason(1)`, `protocol-major(2)`, `protocol-minor(2)`, `length(2)` where `length = (reason + pad) / 4` in 4-byte units. The current helper omits `length`, so the reply is two bytes short and a real client misparses it.

**Files:**
- Modify: `crates/yserver-protocol/src/x11/mod.rs:606-620`
- Test: `crates/yserver-protocol/src/x11/mod.rs` (new `#[cfg(test)] mod tests` at end of file, or append to an existing one if present)

- [ ] **Step 1: Write the failing test**

Append to `crates/yserver-protocol/src/x11/mod.rs`:

```rust
#[cfg(test)]
mod setup_failed_tests {
    use super::*;

    // Wire layout per the X11 core protocol "Connection Setup" (failed):
    //   1  success=0   1  lengthReason   2  major   2  minor
    //   2  length=(reason+pad)/4         n  reason  p  pad
    // reason="no" (n=2) → padded to 4 → length=1.

    #[test]
    fn write_setup_failed_little_endian_layout() {
        let mut out = Vec::new();
        write_setup_failed(&mut out, ClientByteOrder::LittleEndian, "no").unwrap();
        assert_eq!(
            out,
            vec![
                0x00, 0x02, // success=0, lengthReason=2
                0x0b, 0x00, // major=11 (LE)
                0x00, 0x00, // minor=0
                0x01, 0x00, // length=1 (LE)
                b'n', b'o', 0x00, 0x00, // reason + pad to 4
            ]
        );
    }

    #[test]
    fn write_setup_failed_big_endian_layout() {
        let mut out = Vec::new();
        write_setup_failed(&mut out, ClientByteOrder::BigEndian, "no").unwrap();
        assert_eq!(
            out,
            vec![
                0x00, 0x02, // success=0, lengthReason=2
                0x00, 0x0b, // major=11 (BE)
                0x00, 0x00, // minor=0
                0x00, 0x01, // length=1 (BE)
                b'n', b'o', 0x00, 0x00,
            ]
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p yserver-protocol setup_failed_tests -- --nocapture`
Expected: FAIL — current output is `[0x00,0x02,0x0b,0x00,0x00,0x00,b'n',b'o',...]` (no `length` bytes), so the `assert_eq!` mismatches.

- [ ] **Step 3: Fix `write_setup_failed`**

Replace `crates/yserver-protocol/src/x11/mod.rs:606-620` with:

```rust
pub fn write_setup_failed(
    writer: &mut impl Write,
    byte_order: ClientByteOrder,
    reason: &str,
) -> io::Result<()> {
    let reason_len = reason.len().min(u8::MAX as usize);
    let reason_bytes = &reason.as_bytes()[..reason_len];
    let length_units = (pad4(reason_len) / 4) as u16;

    let mut body = Vec::with_capacity(8 + pad4(reason_len));
    body.push(0); // success = Failed
    body.push(reason_len as u8); // lengthReason
    write_u16(byte_order, &mut body, 11); // protocol-major
    write_u16(byte_order, &mut body, 0); // protocol-minor
    write_u16(byte_order, &mut body, length_units); // length: 4-byte units of (padded) reason
    body.extend_from_slice(reason_bytes);
    pad_vec4(&mut body);
    writer.write_all(&body)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver-protocol setup_failed_tests`
Expected: PASS (both).

- [ ] **Step 5: Cross-check against the existing handshake test**

The `full_handshake_round_trip` test in `setup_thread.rs` already reads `head[6..8]` as the success-reply length; this fix makes the *failed* reply use the same 8-byte prefix shape. Run `cargo test -p yserver-core full_handshake_round_trip` and confirm PASS (unaffected — different code path, sanity only).

- [ ] **Step 6: Commit**

```bash
git add crates/yserver-protocol/src/x11/mod.rs
git commit -m "fix(setup): emit the 8-byte length field in write_setup_failed

The connection-setup failed prefix omitted the 2-byte length field at
bytes 6-7 (count of 4-byte units of padded reason). Latent because the
only caller (resource exhaustion) is effectively never hit; the -auth
reject path needs a correct reply."
```

---

## Task 2: Shared Xau record decoder (`xauth.rs`) + refactor `pump.rs`

Extract the record decoding that currently lives privately in `pump.rs` into a shared module so the server-side authorizer can reuse it. Behavior of the client reader must not change.

**Files:**
- Create: `crates/yserver-core/src/xauth.rs`
- Modify: `crates/yserver-core/src/lib.rs:13` (add module)
- Modify: `crates/yserver-core/src/host_x11/pump.rs:29` (remove local const), `:316-372` (use shared parser), `:522-535` (remove local helpers)
- Test: in `crates/yserver-core/src/xauth.rs`

- [ ] **Step 1: Write the failing test (new module file)**

Create `crates/yserver-core/src/xauth.rs`:

```rust
//! Shared decoder for the Xau (`~/.Xauthority`) binary record format.
//!
//! Each record is a sequence of big-endian length-prefixed fields:
//! `family(2)  addr_len(2) addr  num_len(2) num  name_len(2) name  data_len(2) data`.
//! Both the host-X11 client reader (selects one cookie by family/number)
//! and the server-side authorizer (loads every MIT cookie) decode through
//! here; record *selection* stays at the call site.

pub const MIT_MAGIC_COOKIE: &str = "MIT-MAGIC-COOKIE-1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XauthRecord {
    pub family: u16,
    pub address: Vec<u8>,
    pub number: Vec<u8>,
    pub name: Vec<u8>,
    pub data: Vec<u8>,
}

/// Decode records until one fails to parse (truncated/short tail),
/// keeping every fully-decoded record before that point. Mirrors
/// `XauReadAuth` looping until it returns NULL.
pub fn parse_records(bytes: &[u8]) -> Vec<XauthRecord> {
    let mut cursor = 0usize;
    let mut out = Vec::new();
    while cursor < bytes.len() {
        let Some(family) = read_be_u16(bytes, &mut cursor) else { break };
        let Some(address) = read_field(bytes, &mut cursor) else { break };
        let Some(number) = read_field(bytes, &mut cursor) else { break };
        let Some(name) = read_field(bytes, &mut cursor) else { break };
        let Some(data) = read_field(bytes, &mut cursor) else { break };
        out.push(XauthRecord { family, address, number, name, data });
    }
    out
}

fn read_be_u16(bytes: &[u8], cursor: &mut usize) -> Option<u16> {
    let end = *cursor + 2;
    let value = u16::from_be_bytes(bytes.get(*cursor..end)?.try_into().ok()?);
    *cursor = end;
    Some(value)
}

fn read_field(bytes: &[u8], cursor: &mut usize) -> Option<Vec<u8>> {
    let len = read_be_u16(bytes, cursor)? as usize;
    let end = *cursor + len;
    let value = bytes.get(*cursor..end)?.to_vec();
    *cursor = end;
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-encode one Xau record. Field framing per the format above.
    fn rec(family: u16, addr: &[u8], num: &[u8], name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&family.to_be_bytes());
        for f in [addr, num, name, data] {
            b.extend_from_slice(&(f.len() as u16).to_be_bytes());
            b.extend_from_slice(f);
        }
        b
    }

    #[test]
    fn parses_two_records() {
        // family 256 = FamilyLocal; address = hostname; number = display.
        let mut bytes = rec(256, b"bee", b"0", MIT_MAGIC_COOKIE.as_bytes(), &[1u8; 16]);
        bytes.extend(rec(256, b"bee", b"1", MIT_MAGIC_COOKIE.as_bytes(), &[2u8; 16]));
        let recs = parse_records(&bytes);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].number, b"0");
        assert_eq!(recs[0].data, vec![1u8; 16]);
        assert_eq!(recs[1].number, b"1");
        assert_eq!(recs[1].name, MIT_MAGIC_COOKIE.as_bytes());
    }

    #[test]
    fn stops_at_truncated_tail_keeping_prior_records() {
        let mut bytes = rec(256, b"bee", b"0", MIT_MAGIC_COOKIE.as_bytes(), &[7u8; 16]);
        bytes.extend_from_slice(&[0x01, 0x00, 0x00]); // a dangling, incomplete record
        let recs = parse_records(&bytes);
        assert_eq!(recs.len(), 1, "valid leading record kept, truncated tail dropped");
        assert_eq!(recs[0].data, vec![7u8; 16]);
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/yserver-core/src/lib.rs`, add after line 12 (`mod unix_fd;`), keeping alphabetical-ish grouping:

```rust
pub mod xauth;
```

- [ ] **Step 3: Run the new tests to verify they pass**

Run: `cargo test -p yserver-core xauth::tests`
Expected: PASS (both). (The module is new, so these are green immediately — they pin the format the later server loader relies on.)

- [ ] **Step 4: Externally verify the fixture format (one-time manual check)**

Run, to confirm the hand-encoded framing matches a real file:

```bash
xauth -f /tmp/xauth-check add :77 . $(mcookie) && xxd /tmp/xauth-check; rm -f /tmp/xauth-check
```

Confirm the leading bytes are `01 00` (family 256 = FamilyLocal, big-endian) followed by a 2-byte big-endian length and the hostname. If `xauth`/`mcookie` are unavailable, skip — the format is from the documented Xau layout. This is a verification step, no code change.

- [ ] **Step 5: Refactor `pump.rs` to use the shared decoder**

In `crates/yserver-core/src/host_x11/pump.rs`:

1. Delete the local constant at line 29: `const MIT_MAGIC_COOKIE: &str = "MIT-MAGIC-COOKIE-1";`
2. Delete the local helpers `read_be_u16_record` (≈line 522) and `read_record_field` (≈line 529).
3. Replace the body of `XAuthority::load` (the `while cursor < bytes.len() { ... }` loop, ≈lines 324-367) with:

```rust
        let bytes = fs::read(path)?;
        let display_number = display_number.to_string();
        let mut fallback = None;

        for rec in crate::xauth::parse_records(&bytes) {
            if rec.name == crate::xauth::MIT_MAGIC_COOKIE.as_bytes()
                && rec.number == display_number.as_bytes()
            {
                let auth = Self { name: rec.name, data: rec.data };
                if rec.address.is_empty() {
                    return Ok(Some(auth)); // exact (wildcard-address) match wins
                }
                fallback = Some(auth);
            }
        }

        Ok(fallback)
```

Keep the `XAuthority` struct and the `path`-resolution lines above this unchanged. Fix up any now-unused imports (e.g. if `ErrorKind` was only used by removed code, leave it if still referenced elsewhere).

- [ ] **Step 6: Verify the client reader still builds and its behavior is unchanged**

Run: `cargo build -p yserver-core --locked` then `cargo test -p yserver-core host_x11`
Expected: builds clean; any existing pump/host_x11 tests PASS. (If there are no pump tests, the build + clippy in the final task covers the refactor.)

- [ ] **Step 7: Commit**

```bash
git add crates/yserver-core/src/xauth.rs crates/yserver-core/src/lib.rs crates/yserver-core/src/host_x11/pump.rs
git commit -m "refactor(xauth): extract shared Xau record decoder

New yserver-core::xauth module owns the .Xauthority record framing; the
host-X11 client reader now decodes through it. Server-side authorizer
will reuse parse_records. No behavior change for the client reader."
```

---

## Task 3: `auth` module skeleton + reload-trigger logic (`should_reload`)

**Files:**
- Create: `crates/yserver-core/src/core_loop/auth.rs`
- Modify: `crates/yserver-core/src/core_loop/mod.rs:21` (add module)
- Test: in `crates/yserver-core/src/core_loop/auth.rs`

- [ ] **Step 1: Write the module with types + `should_reload`, and failing tests**

Create `crates/yserver-core/src/core_loop/auth.rs`:

```rust
//! Server-side X11 connection authorization (`-auth` / MIT-MAGIC-COOKIE-1).
//!
//! Faithful to X.Org's `os/auth.c` + `os/mitauth.c`: honor an auth file,
//! validate each client's SetupRequest cookie, reject mismatches. No auth
//! file (or zero cookies loaded) keeps local access open. See
//! docs/superpowers/specs/2026-06-25-xauth-server-auth-design.md.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use crate::xauth::{self, MIT_MAGIC_COOKIE};

// Reject reason strings — byte-for-byte X.Org. Note the trailing newline
// on two of them and its absence on the cookie one.
//   os/auth.c:211, os/auth.c:207, os/mitauth.c:82
const REASON_NO_PROTO: &str =
    "Authorization required, but no authorization protocol specified\n";
const REASON_BAD_PROTO: &str = "Authorization protocol not supported by server\n";
const REASON_BAD_COOKIE: &str = "Invalid MIT-MAGIC-COOKIE-1 key";

#[derive(Debug, PartialEq, Eq)]
pub enum AuthVerdict {
    Allow,
    Reject(&'static str),
}

pub struct AuthState {
    file: Option<PathBuf>,
    inner: Mutex<Inner>,
}

struct Inner {
    /// Last successful stat mtime; `None` ≡ X.Org `lastmod == 0`.
    last_mtime: Option<SystemTime>,
    /// Latches true once ≥1 cookie has loaded (X.Org `loaded`).
    ever_loaded: bool,
    /// Host-ACL local-access toggle (collapsed to a bool on unix-only).
    local_open: bool,
    /// MIT-MAGIC-COOKIE-1 data blobs. ADDITIVE — never cleared.
    cookies: Vec<Vec<u8>>,
}

/// Reproduce X.Org's reload trigger (os/auth.c:166-175), updating
/// `last_mtime`. `stat_mtime == None` means the stat failed. Returns
/// true iff a load should run now.
fn should_reload(stat_mtime: Option<SystemTime>, last_mtime: &mut Option<SystemTime>) -> bool {
    match stat_mtime {
        Some(m) => match *last_mtime {
            Some(lm) if m > lm => {
                *last_mtime = Some(m);
                true
            }
            Some(_) => false, // not strictly newer (incl. rollback) → no reload
            None => {
                *last_mtime = Some(m);
                true
            }
        },
        None => {
            // stat failed: one-shot transition (lastmod → 0), then quiet.
            if last_mtime.is_some() {
                *last_mtime = None;
                true
            } else {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn first_successful_stat_loads() {
        let mut last = None;
        assert!(should_reload(Some(t(100)), &mut last));
        assert_eq!(last, Some(t(100)));
    }

    #[test]
    fn newer_mtime_reloads_same_mtime_does_not() {
        let mut last = Some(t(100));
        assert!(!should_reload(Some(t(100)), &mut last)); // equal → no
        assert!(should_reload(Some(t(200)), &mut last)); // newer → yes
        assert_eq!(last, Some(t(200)));
    }

    #[test]
    fn mtime_rollback_does_not_reload() {
        let mut last = Some(t(200));
        assert!(!should_reload(Some(t(100)), &mut last)); // older → no reload
        assert_eq!(last, Some(t(200)), "last_mtime unchanged on rollback");
    }

    #[test]
    fn stat_failure_is_one_shot() {
        let mut last = Some(t(200));
        assert!(should_reload(None, &mut last), "loss transition reloads once");
        assert_eq!(last, None);
        assert!(!should_reload(None, &mut last), "stays quiet while missing");
        // File reappears with a newer mtime → reload.
        assert!(should_reload(Some(t(300)), &mut last));
        assert_eq!(last, Some(t(300)));
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/yserver-core/src/core_loop/mod.rs`, add (alphabetical position, before `barriers` or wherever fits) :

```rust
pub mod auth;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p yserver-core core_loop::auth::tests::`
Expected: PASS (all four). They pin the reload trigger directly against synthetic mtimes — no filesystem flakiness.

(Note: `AuthState`/`Inner` are defined but not yet used; the compiler may warn about dead fields until Task 5. That's expected mid-plan — do not silence with `#[allow]`; Task 5 wires them.)

- [ ] **Step 4: Commit**

```bash
git add crates/yserver-core/src/core_loop/auth.rs crates/yserver-core/src/core_loop/mod.rs
git commit -m "feat(auth): AuthState types + Xorg-faithful reload trigger

should_reload reproduces os/auth.c:166-175: reload only on strictly
newer mtime (rollback ignored) and a one-shot stat-loss transition."
```

---

## Task 4: Load-outcome state machine (`apply_load_outcome`)

**Files:**
- Modify: `crates/yserver-core/src/core_loop/auth.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `auth.rs`:

```rust
    fn enforcing_with(cookies: Vec<Vec<u8>>) -> Inner {
        Inner { last_mtime: Some(t(1)), ever_loaded: true, local_open: false, cookies }
    }
    fn fresh_open() -> Inner {
        Inner { last_mtime: None, ever_loaded: false, local_open: true, cookies: Vec::new() }
    }

    #[test]
    fn loaded_enforces_and_appends() {
        let mut i = fresh_open();
        i.apply_load_outcome(LoadOutcome::Loaded(vec![vec![1u8; 16]]));
        assert!(!i.local_open);
        assert!(i.ever_loaded);
        assert_eq!(i.cookies, vec![vec![1u8; 16]]);
        // A second load APPENDS (additive — Xorg never clears on reload).
        i.apply_load_outcome(LoadOutcome::Loaded(vec![vec![2u8; 16]]));
        assert_eq!(i.cookies, vec![vec![1u8; 16], vec![2u8; 16]]);
    }

    #[test]
    fn empty_file_reopens_even_after_load() {
        let mut i = enforcing_with(vec![vec![1u8; 16]]);
        i.apply_load_outcome(LoadOutcome::OpenedEmpty);
        assert!(i.local_open, "empty file reopens local access (Xorg quirk)");
        assert_eq!(i.cookies, vec![vec![1u8; 16]], "cookies retained (additive)");
    }

    #[test]
    fn open_failed_after_load_keeps_enforcing() {
        let mut i = enforcing_with(vec![vec![1u8; 16]]);
        i.apply_load_outcome(LoadOutcome::OpenFailed);
        assert!(!i.local_open, "stays enforcing — does NOT reopen");
        assert_eq!(i.cookies, vec![vec![1u8; 16]]);
    }

    #[test]
    fn open_failed_before_any_load_opens() {
        let mut i = fresh_open();
        i.local_open = true; // initial open state
        i.apply_load_outcome(LoadOutcome::OpenFailed);
        assert!(i.local_open, "never loaded → open");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver-core core_loop::auth::tests::loaded_enforces_and_appends`
Expected: FAIL to compile — `LoadOutcome` and `apply_load_outcome` don't exist yet.

- [ ] **Step 3: Implement `LoadOutcome` + `apply_load_outcome`**

Add to `auth.rs` (above the `tests` module):

```rust
enum LoadOutcome {
    /// File opened, ≥1 MIT cookie decoded.
    Loaded(Vec<Vec<u8>>),
    /// File opened but no MIT cookies (empty or none parseable).
    OpenedEmpty,
    /// File could not be opened/read.
    OpenFailed,
}

impl Inner {
    fn apply_load_outcome(&mut self, outcome: LoadOutcome) {
        match outcome {
            LoadOutcome::Loaded(mut cookies) => {
                self.cookies.append(&mut cookies); // additive — os/auth.c never clears
                self.local_open = false; // DisableLocalAccess
                self.ever_loaded = true;
            }
            LoadOutcome::OpenedEmpty => {
                // EnableLocalAccess, even if previously loaded (os/auth.c:197 quirk).
                self.local_open = true;
            }
            LoadOutcome::OpenFailed => {
                // loadauth == -1: open only if never loaded; else unchanged.
                if !self.ever_loaded {
                    self.local_open = true;
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver-core core_loop::auth::tests::`
Expected: PASS (all, including Task 3's).

- [ ] **Step 5: Commit**

```bash
git add crates/yserver-core/src/core_loop/auth.rs
git commit -m "feat(auth): load-outcome state machine (additive + reopen quirk)

apply_load_outcome mirrors os/auth.c:176-198: >=1 cookie enforces and
latches loaded; empty file reopens even after a prior load; open-failure
after a load stays enforcing."
```

---

## Task 5: Constant-time compare, verdict, and `check()` glue

**Files:**
- Modify: `crates/yserver-core/src/core_loop/auth.rs`

- [ ] **Step 1: Write the failing tests (verdict + ct_eq + check)**

Add to the `tests` module in `auth.rs`:

```rust
    #[test]
    fn ct_eq_matches_only_identical_bytes() {
        assert!(ct_eq(&[1, 2, 3], &[1, 2, 3]));
        assert!(!ct_eq(&[1, 2, 3], &[1, 2, 4])); // same len, differ
        assert!(!ct_eq(&[1, 2, 3], &[1, 2])); // differing len
        assert!(ct_eq(&[], &[]));
    }

    #[test]
    fn verdict_cookie_match_allows() {
        let i = enforcing_with(vec![vec![9u8; 16]]);
        assert_eq!(i.verdict(MIT_MAGIC_COOKIE.as_bytes(), &[9u8; 16]), AuthVerdict::Allow);
    }

    #[test]
    fn verdict_enforcing_rejects() {
        let i = enforcing_with(vec![vec![9u8; 16]]);
        assert_eq!(
            i.verdict(MIT_MAGIC_COOKIE.as_bytes(), &[0u8; 16]),
            AuthVerdict::Reject(REASON_BAD_COOKIE)
        );
        assert_eq!(i.verdict(b"", b""), AuthVerdict::Reject(REASON_NO_PROTO));
        assert_eq!(
            i.verdict(b"XDM-AUTHORIZATION-1", &[0u8; 8]),
            AuthVerdict::Reject(REASON_BAD_PROTO)
        );
    }

    #[test]
    fn verdict_local_open_allows_anything() {
        let mut i = fresh_open();
        i.local_open = true;
        assert_eq!(i.verdict(b"", b""), AuthVerdict::Allow);
        assert_eq!(i.verdict(MIT_MAGIC_COOKIE.as_bytes(), &[0u8; 16]), AuthVerdict::Allow);
    }

    // ---- check() over a real temp file ----

    fn xauth_bytes(number: &[u8], cookie: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&256u16.to_be_bytes()); // FamilyLocal
        for f in [b"host".as_slice(), number, MIT_MAGIC_COOKIE.as_bytes(), cookie] {
            b.extend_from_slice(&(f.len() as u16).to_be_bytes());
            b.extend_from_slice(f);
        }
        b
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("yserver-auth-test-{}-{tag}", std::process::id()))
    }

    #[test]
    fn check_enforces_against_file_cookie() {
        let cookie = [0xABu8; 16];
        let path = temp_path("enforce");
        fs::write(&path, xauth_bytes(b"7", &cookie)).unwrap();
        let auth = AuthState::new(Some(path.clone()));

        assert_eq!(auth.check(MIT_MAGIC_COOKIE.as_bytes(), &cookie), AuthVerdict::Allow);
        assert_eq!(
            auth.check(MIT_MAGIC_COOKIE.as_bytes(), &[0u8; 16]),
            AuthVerdict::Reject(REASON_BAD_COOKIE)
        );
        assert_eq!(auth.check(b"", b""), AuthVerdict::Reject(REASON_NO_PROTO));

        // File vanishes after a successful load → stays enforcing; the
        // already-loaded cookie still validates (additive list).
        fs::remove_file(&path).unwrap();
        assert_eq!(auth.check(MIT_MAGIC_COOKIE.as_bytes(), &cookie), AuthVerdict::Allow);
        assert_eq!(
            auth.check(MIT_MAGIC_COOKIE.as_bytes(), &[0u8; 16]),
            AuthVerdict::Reject(REASON_BAD_COOKIE),
            "missing-after-load does not reopen"
        );
    }

    #[test]
    fn check_no_auth_file_is_open() {
        let auth = AuthState::new(None);
        assert_eq!(auth.check(b"", b""), AuthVerdict::Allow);
    }

    #[test]
    fn check_empty_file_is_open() {
        let path = temp_path("empty");
        fs::write(&path, b"").unwrap();
        let auth = AuthState::new(Some(path.clone()));
        assert_eq!(auth.check(b"", b""), AuthVerdict::Allow);
        let _ = fs::remove_file(&path);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p yserver-core core_loop::auth::tests::ct_eq_matches_only_identical_bytes`
Expected: FAIL to compile — `ct_eq`, `Inner::verdict`, `AuthState::new`, `AuthState::check` not yet defined.

- [ ] **Step 3: Implement `ct_eq`, `verdict`, `load_outcome`, and `AuthState` methods**

Add to `auth.rs` (above the `tests` module):

```rust
/// Constant-time byte equality (mirrors X.Org's timingsafe_memcmp).
/// Length is compared first (X.Org does too); the byte loop is branchless.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn load_outcome(path: &Path) -> LoadOutcome {
    match fs::read(path) {
        Ok(bytes) => {
            let cookies: Vec<Vec<u8>> = xauth::parse_records(&bytes)
                .into_iter()
                .filter(|r| r.name == MIT_MAGIC_COOKIE.as_bytes())
                .map(|r| r.data)
                .collect();
            if cookies.is_empty() {
                LoadOutcome::OpenedEmpty
            } else {
                LoadOutcome::Loaded(cookies)
            }
        }
        Err(_) => LoadOutcome::OpenFailed,
    }
}

impl Inner {
    fn verdict(&self, name: &[u8], data: &[u8]) -> AuthVerdict {
        // Cookie match OR local-open admits (mirrors CheckAuthorization +
        // host-ACL fallthrough, os/connection.c:536-560).
        if name == MIT_MAGIC_COOKIE.as_bytes() && self.cookies.iter().any(|c| ct_eq(c, data)) {
            return AuthVerdict::Allow;
        }
        if self.local_open {
            return AuthVerdict::Allow;
        }
        if name.is_empty() {
            return AuthVerdict::Reject(REASON_NO_PROTO);
        }
        if name == MIT_MAGIC_COOKIE.as_bytes() {
            return AuthVerdict::Reject(REASON_BAD_COOKIE);
        }
        AuthVerdict::Reject(REASON_BAD_PROTO)
    }
}

impl AuthState {
    /// Build from `LaunchOptions.auth_file`. `None` ⇒ permanently open.
    pub fn new(file: Option<PathBuf>) -> Arc<Self> {
        let local_open = file.is_none();
        Arc::new(Self {
            file,
            inner: Mutex::new(Inner {
                last_mtime: None,
                ever_loaded: false,
                local_open,
                cookies: Vec::new(),
            }),
        })
    }

    /// Authorize one client. Reloads lazily on file change, then decides.
    pub fn check(&self, proto_name: &[u8], proto_data: &[u8]) -> AuthVerdict {
        let Some(path) = self.file.as_deref() else {
            return AuthVerdict::Allow;
        };
        // Poisoning recovery — same pattern as setup_thread.rs:66.
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let stat_mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok());
        if should_reload(stat_mtime, &mut inner.last_mtime) {
            let outcome = load_outcome(path);
            inner.apply_load_outcome(outcome);
        }
        inner.verdict(proto_name, proto_data)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p yserver-core core_loop::auth::`
Expected: PASS (all). `AuthState`/`Inner` fields are now read, so dead-field warnings clear.

- [ ] **Step 5: Commit**

```bash
git add crates/yserver-core/src/core_loop/auth.rs
git commit -m "feat(auth): constant-time verdict + check() glue

Cookie-match-or-local-open verdict with byte-for-byte Xorg reject
strings; check() does lazy stat+reload under a poison-recovering lock."
```

---

## Task 6: Thread `AuthState` through the setup path

**Files:**
- Modify: `crates/yserver-core/src/core_loop/setup_thread.rs:58-95` (spawn), `:123-150` (run_setup), `:284` (existing test)
- Modify: `crates/yserver-core/src/core_loop/run.rs:334-342` (run_core sig), `:511` (accept_pending call), `:1432-1452` (accept_pending sig + spawn call)

- [ ] **Step 1: Update `spawn` and `run_setup` to take `Arc<AuthState>` and call `check`**

In `setup_thread.rs`:

1. Add to the `use` block: `use crate::core_loop::auth::{AuthState, AuthVerdict};` and `use std::sync::Arc;` (Arc is likely already imported — check the existing `use std::{... sync::{Arc, Mutex} ...}` at the top; if present, don't duplicate).

2. Change `spawn` signature (line 58) and pass `auth` into the thread:

```rust
pub fn spawn(
    id: ClientId,
    stream: UnixStream,
    sender: CoreSender,
    registry: SetupRegistry,
    auth: Arc<AuthState>,
) -> io::Result<()> {
```

and in the thread closure body, change the `run_setup` call (line 78):

```rust
            if let Err(e) = run_setup(id, stream, &sender, &auth) {
```

3. Change `run_setup` signature (line 123) and insert the check right after `read_setup_request` (after line 131's `debug!`):

```rust
fn run_setup(
    id: ClientId,
    mut stream: UnixStream,
    sender: &CoreSender,
    auth: &AuthState,
) -> io::Result<()> {
    stream.set_read_timeout(Some(SETUP_TIMEOUT))?;
    stream.set_write_timeout(Some(SETUP_TIMEOUT))?;

    let setup = x11::read_setup_request(&mut stream)?;
    debug!(
        "client {} setup: byte_order={:?} protocol {}.{}",
        id.0, setup.byte_order, setup.protocol_major, setup.protocol_minor
    );

    if let AuthVerdict::Reject(reason) =
        auth.check(&setup.auth_protocol_name, &setup.auth_protocol_data)
    {
        debug!(
            "client {} auth rejected ({reason:?}); presented proto {:?}",
            id.0,
            String::from_utf8_lossy(&setup.auth_protocol_name)
        );
        x11::write_setup_failed(&mut stream, setup.byte_order, reason)?;
        return Ok(()); // drop stream → connection closes (Xorg disconnects too)
    }

    // ... existing SetupAllocate rendezvous continues unchanged ...
```

Leave everything from the `let (response_tx, response_rx) = bounded::<...>` line onward unchanged.

- [ ] **Step 2: Update ALL existing handshake-test `spawn` calls to pass an open `AuthState`**

`spawn` is called in **four** existing tests — `setup_thread.rs:284`, `:361`, `:416`, `:444`. Update **every** one (changing only line 284 leaves three compile errors). Each becomes:

```rust
        spawn(id, server_side, sender, registry.clone(), AuthState::new(None)).unwrap();
```

Verify with `grep -n "spawn(id" crates/yserver-core/src/core_loop/setup_thread.rs` — all four must carry the new fifth argument. (The test module's `use super::*;` brings `AuthState` into scope via the file-level import added in Step 1.)

- [ ] **Step 3: Update `run_core` and `accept_pending` to carry the `Arc<AuthState>`**

In `run.rs`:

1. Add to imports: `use crate::core_loop::auth::AuthState;` and ensure `use std::sync::Arc;` is present.

2. `run_core` signature (line 334) — add a parameter:

```rust
pub fn run_core(
    mut poll: Poll,
    rx: CoreReceiver,
    sender: CoreSender,
    state: &mut ServerState,
    backend: &mut dyn Backend,
    listener: Option<UnixListener>,
    client_id_allocator: &ClientIdAllocator,
    auth: Arc<AuthState>,
) -> io::Result<()> {
```

3. `accept_pending` call site (line 511) — pass `&auth`:

```rust
                        accept_pending(listener, client_id_allocator, &sender, &setup_registry, &auth);
```

4. `accept_pending` signature (line 1434) and its `spawn` call (line 1445):

```rust
fn accept_pending(
    listener: &UnixListener,
    client_id_allocator: &ClientIdAllocator,
    sender: &CoreSender,
    registry: &SetupRegistry,
    auth: &Arc<AuthState>,
) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let id = client_id_allocator.allocate();
                if let Err(err) = setup_thread::spawn(
                    id,
                    stream,
                    sender.clone_handle(),
                    registry.clone(),
                    auth.clone(),
                ) {
                    error!("setup thread spawn failed for client {}: {err}", id.0);
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(err) => {
                warn!("accept failed: {err}");
                break;
            }
        }
    }
}
```

- [ ] **Step 4: Fix EVERY other `run_core` caller (incl. the ynest production path)**

Run: `grep -rn "run_core(" crates/ | grep -v "fn run_core\|pub use"`

There are five call sites total. Append `, AuthState::new(None)` (open auth) as the final argument to each:

- **`crates/yserver-core/src/nested.rs:430`** — the **non-test** ynest path. Add `, AuthState::new(None)` to the call, and add an import. nested.rs uses fully-qualified `crate::` paths, so write the argument as `crate::core_loop::auth::AuthState::new(None)` (no new `use` needed). ynest is a nested/host-X11 server with no `-auth` concept, so open is correct. (ynest is compile-only per project policy — this keeps `yserver-core` building; do not invest further there.)
- **`crates/yserver-core/src/core_loop/run.rs:1580`, `:1631`, `:1857`** — test callers in `run.rs`. Append `, AuthState::new(None)`. `AuthState` is in scope via the file-level `use crate::core_loop::auth::AuthState;` added in Step 3.

(The `yserver/src/lib.rs:465` caller is handled in Task 7 with the real auth file.)

- [ ] **Step 5: Build to verify wiring compiles**

Run: `cargo build -p yserver-core --locked`
Expected: clean build. Fix any missed call site the compiler flags.

- [ ] **Step 6: Run the affected tests**

Run: `cargo test -p yserver-core core_loop::`
Expected: PASS, including `full_handshake_round_trip` (now passing `AuthState::new(None)`).

- [ ] **Step 7: Commit**

```bash
git add crates/yserver-core/src/core_loop/setup_thread.rs crates/yserver-core/src/core_loop/run.rs
git commit -m "feat(auth): enforce -auth in the client setup handshake

run_core carries an Arc<AuthState> down to each setup thread, which
calls check() after read_setup_request and writes a setup-failed reply
on reject before closing the connection."
```

---

## Task 7: Build `AuthState` from `opts.auth_file` in `yserver::run`

**Files:**
- Modify: `crates/yserver/src/lib.rs:463-473` (run_core call)

- [ ] **Step 1: Construct and pass the `AuthState`**

In `crates/yserver/src/lib.rs`, immediately before the `run_core` call (line 465), add:

```rust
    let auth = core_loop::auth::AuthState::new(opts.auth_file.clone());
    if opts.auth_file.is_some() {
        log::info!("yserver: authorization enabled via -auth {:?}", opts.auth_file);
    } else {
        log::info!("yserver: no -auth file; local access open (Xorg default)");
    }
```

Then add `auth` as the final argument to `run_core`:

```rust
    let result = core_loop::run_core(
        poll,
        rx,
        sender,
        &mut state,
        &mut backend,
        Some(listener),
        &alloc,
        auth,
    );
```

- [ ] **Step 2: Build the binary**

Run: `cargo build -p yserver --locked`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add crates/yserver/src/lib.rs
git commit -m "feat(auth): wire -auth file into the server run loop

Build AuthState from LaunchOptions.auth_file and hand it to run_core;
log whether authorization is enabled."
```

---

## Task 8: Integration test — allow vs reject on the wire

Verify the end-to-end handshake: a matching cookie reaches the core (SetupAllocate observed), a bogus cookie is refused on the wire (setup-failed reply, no SetupAllocate, connection closed).

**Files:**
- Modify: `crates/yserver-core/src/core_loop/setup_thread.rs` (tests module)

- [ ] **Step 1: Add wire helpers + the two integration tests**

In the `tests` module of `setup_thread.rs`, add:

```rust
    use crate::core_loop::auth::AuthState;
    use crate::xauth::MIT_MAGIC_COOKIE;
    use std::path::PathBuf;

    /// Hand-encode a SetupRequest with a MIT-MAGIC-COOKIE-1 auth payload.
    fn write_setup_with_cookie(s: &mut UnixStream, cookie: &[u8]) -> io::Result<()> {
        let name = MIT_MAGIC_COOKIE.as_bytes();
        let mut buf = Vec::new();
        buf.push(b'l'); // little-endian
        buf.push(0);
        buf.extend_from_slice(&11u16.to_le_bytes()); // protocol major
        buf.extend_from_slice(&0u16.to_le_bytes()); // protocol minor
        buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
        buf.extend_from_slice(&(cookie.len() as u16).to_le_bytes());
        buf.extend_from_slice(&[0, 0]); // 2 pad bytes (header is 12)
        // name + pad4
        buf.extend_from_slice(name);
        while buf.len() % 4 != 0 { buf.push(0); }
        // data + pad4
        buf.extend_from_slice(cookie);
        while buf.len() % 4 != 0 { buf.push(0); }
        s.write_all(&buf)
    }

    fn xauth_file(tag: &str, cookie: &[u8]) -> PathBuf {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&256u16.to_be_bytes()); // FamilyLocal
        for f in [b"host".as_slice(), b"0", MIT_MAGIC_COOKIE.as_bytes(), cookie] {
            bytes.extend_from_slice(&(f.len() as u16).to_be_bytes());
            bytes.extend_from_slice(f);
        }
        let path = std::env::temp_dir()
            .join(format!("yserver-setup-auth-{}-{tag}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn matching_cookie_reaches_core() {
        let cookie = [0x5Au8; 16];
        let path = xauth_file("ok", &cookie);
        let (poll, sender, rx) = channel().unwrap();
        let _ = poll;
        let registry = make_registry();
        let (server_side, mut client_side) = UnixStream::pair().unwrap();
        spawn(ClientId(1), server_side, sender, registry.clone(), AuthState::new(Some(path.clone()))).unwrap();

        write_setup_with_cookie(&mut client_side, &cookie).unwrap();

        // Allowed → core receives SetupAllocate.
        match wait_for_message(&rx, Duration::from_secs(2)).unwrap() {
            Message::SetupAllocate { .. } => {}
            other => panic!("expected SetupAllocate, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bogus_cookie_is_refused_on_the_wire() {
        let path = xauth_file("bad", &[0x11u8; 16]);
        let (poll, sender, rx) = channel().unwrap();
        let _ = poll;
        let registry = make_registry();
        let (server_side, mut client_side) = UnixStream::pair().unwrap();
        spawn(ClientId(2), server_side, sender, registry.clone(), AuthState::new(Some(path.clone()))).unwrap();

        write_setup_with_cookie(&mut client_side, &[0x22u8; 16]).unwrap(); // wrong cookie

        // Rejected → first reply byte is 0 (Failed), and a reason follows.
        let head = read_n_with_timeout(&mut client_side, 8, Duration::from_secs(2)).unwrap();
        assert_eq!(head[0], 0, "setup-failed reply");
        let reason_len = head[1] as usize;
        let length_units = u16::from_le_bytes([head[6], head[7]]) as usize;
        assert_eq!(length_units, (reason_len + 3) / 4, "length field present & correct");
        let reason = read_n_with_timeout(&mut client_side, length_units * 4, Duration::from_secs(2)).unwrap();
        assert_eq!(&reason[..reason_len], b"Invalid MIT-MAGIC-COOKIE-1 key");

        // Core must NOT have received a SetupAllocate.
        assert!(
            wait_for_message(&rx, Duration::from_millis(200)).is_none(),
            "rejected client must not reach the core"
        );

        // Connection is closed by the server after reject (read → 0 bytes / EOF).
        let mut tail = [0u8; 1];
        client_side.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let n = client_side.read(&mut tail).unwrap_or(0);
        assert_eq!(n, 0, "server closed the connection after reject");
        let _ = std::fs::remove_file(&path);
    }
```

(`wait_for_message` is the existing test helper at `setup_thread.rs:478`, signature `fn wait_for_message(rx, timeout) -> Option<Message>` — so `.is_none()` on a short timeout is the correct "no message arrived" assertion.)

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p yserver-core core_loop::setup_thread::tests`
Expected: PASS — both new tests plus the existing `full_handshake_round_trip`.

- [ ] **Step 3: Commit**

```bash
git add crates/yserver-core/src/core_loop/setup_thread.rs
git commit -m "test(auth): end-to-end allow/reject over the setup handshake

Matching cookie reaches the core; a bogus cookie gets a well-formed
setup-failed reply (length field + exact reason), never reaches the
core, and the connection is closed."
```

---

## Task 9: Workspace verification (fmt / clippy / test)

**Files:** none (verification only)

- [ ] **Step 1: Format (nightly — AGENTS.md)**

Run: `cargo +nightly fmt`
Then: `git diff --exit-code` — if fmt changed anything, review and `git commit -am "style: cargo +nightly fmt"`.

- [ ] **Step 2: Clippy exactly as CI runs it (AGENTS.md)**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings (CI fails on any). `--all-targets` lints the new test code too. Fix anything that points at the new/modified files — do **not** `#[allow]` past it.

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace`
Expected: PASS. Pay attention to any `run_core(` caller you might have missed (Task 6 Step 4).

- [ ] **Step 4: Commit any fixups**

```bash
git add -A
git commit -m "chore(auth): fmt/clippy cleanup"   # only if needed
```

---

## Task 10: Hardware smoke gate (manual — release gate, not automated)

Per the spec, honoring `-auth` means lightdm sessions begin enforcing auth (lightdm already passes `-auth /var/run/lightdm/root/:N`). This MUST be validated on real hardware before merge. Per project conventions: HW runs go through the established tmux `send-keys` procedure, **one agent per checkout**, and recipes always rebuild.

- [ ] **Step 1: Build release**

Run: `cargo build --release -p yserver`

- [ ] **Step 2: lightdm greeter comes up (cookie validates)**

Bring up a session under lightdm on `bee` via the standard HW procedure. **Pass:** greeter renders and you can log in — i.e. the cookie lightdm wrote validated. **Fail (lockout):** black greeter → grep `yserver-*.log` for `auth rejected` to see which reason fired; the presented-proto debug line is load-bearing here.

- [ ] **Step 3: Authorized local client connects**

In the session: `xeyes` (or any X client) with the session's `$XAUTHORITY` set. **Pass:** it connects and displays.

- [ ] **Step 4: Unauthorized client is refused**

Run a client with a deliberately wrong cookie, e.g.:

```bash
XAUTHORITY=/tmp/bogus-xauth xauth -f /tmp/bogus-xauth add :0 . $(mcookie)
DISPLAY=:0 XAUTHORITY=/tmp/bogus-xauth xeyes
```

**Pass:** the client fails with an authorization error (e.g. "No protocol specified" / "Invalid MIT-MAGIC-COOKIE-1 key"), and `yserver-*.log` shows the reject.

- [ ] **Step 5: `just startx` path still works**

Run the `just startx` VT entry point (real xinit generates + passes a cookie). **Pass:** MATE/desktop comes up clean. **Fail:** check whether `just startx` actually passes `-auth`; if it relies on a custom launcher that does not generate a cookie, that launcher needs a follow-up (out of scope here — the server side is correct; note it and file a follow-up issue).

- [ ] **Step 6: Record results**

Note pass/fail per step in the PR description. Only merge once Steps 2–5 pass on HW.

---

## Notes for the implementer

- **No new dependencies.** Constant-time compare is hand-rolled; temp files use `std::env::temp_dir()`.
- **Do not silence mid-plan dead-code warnings** with `#[allow]` — Task 3 introduces `AuthState`/`Inner` before Task 5 uses them; the warnings clear when wired. (If a `<new-diagnostics>` reminder shows stale warnings during edits, verify with `cargo build --locked`, not the reminder.)
- **Faithfulness anchors:** state machine ↔ `os/auth.c:166-198`; reject strings ↔ `os/auth.c:207,211` + `os/mitauth.c:82`; additive cookies ↔ `os/auth.c:122-136`,`216`.
