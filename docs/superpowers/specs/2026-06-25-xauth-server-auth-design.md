# Server-side X11 authorization (`-auth` / MIT-MAGIC-COOKIE-1)

**Date:** 2026-06-25
**Issue:** #57 (xauth support)
**Status:** Design approved, pending implementation plan

## Problem

yserver performs **no** connection authorization. `read_setup_request`
parses the client's `auth_protocol_name` / `auth_protocol_data`
(`yserver-protocol/src/x11/mod.rs:70-71`, `589-595`) and `run_setup`
ignores them entirely (`yserver-core/src/core_loop/setup_thread.rs:123-150`).
The only way a connection fails today is resource-ID exhaustion. Any
client that can reach the Unix socket is admitted.

`-auth FILE` is already accepted on the command line and stashed into
`LaunchOptions.auth_file` (`yserver/src/launch.rs:37-38, 69-70`),
explicitly marked "unused now." lightdm already passes
`-auth /var/run/lightdm/root/:N` (proven by the test at
`launch.rs:551-563`). The plumbing exists; only the *honoring* of the
file is missing.

## Goal

Implement Xorg-faithful, server-side `MIT-MAGIC-COOKIE-1` validation:
honor `-auth FILE`, validate each client's SetupRequest against the
cookies in that file, and reject mismatches with the same Setup-failed
reason strings Xorg uses. When no `-auth` is given, local access stays
open (today's behavior — backwards-compatible).

Cookie *generation* stays external (lightdm / startx / xinit), exactly
as in Xorg. The server only consumes `-auth`; it never writes
`~/.Xauthority`.

## Non-goals

- **xhost / host-based ACL** (`ChangeHosts` / `ListHosts` /
  `SetAccessControl` protocol requests, and the `si:localuser:`
  server-interpreted entries). Deferred to when TCP listening lands —
  on a Unix-socket-only server it has no practical use.
- **Server self-generation** of cookies / writing `~/.Xauthority`.
  Not how Xorg works.
- **Auth protocols other than `MIT-MAGIC-COOKIE-1`** (XDM-AUTHORIZATION-1,
  SUN-DES-1, etc.). Xorg supports them only for XDMCP, which we do not do.

## Grounding: how Xorg does it

From `/home/jos/Projects/xserver`:

- **Validation chain:** `ProcEstablishConnection` (`dix/dispatch.c:3776`)
  → `ClientAuthorized` (`os/connection.c:510`) → `CheckAuthorization`
  (`os/auth.c:156`) → per-protocol `Check`, i.e. `MitCheckCookie`
  (`os/mitauth.c:72`).
- **MIT check** (`os/mitauth.c:72-84`): linear scan of a linked list of
  registered cookies; match iff `data_length == auth->len` **and**
  `timingsafe_memcmp(data, auth->data, data_length) == 0`. Cookie is
  typically 16 bytes but the code compares whatever length was loaded.
- **Loading** (`os/auth.c:100-137`, `LoadAuthorization`): for every
  record `XauReadAuth` returns, if its name matches a registered
  protocol, register its `data`. **Family / address / display-number
  are not consulted** — that filtering is purely client-side
  (`XauGetBestAuthByAddr`). The server accepts any cookie present in the
  file.
- **Lazy reload** (`os/auth.c:166`): the auth file is re-`stat`ed and
  reloaded when it changes.
- **No `-auth` fallback** (`os/auth.c:197-198`): `LoadAuthorization`
  loads 0 entries → `EnableLocalAccess()` → local Unix clients admitted
  via host-based ACL. With `-auth` + ≥1 entry, `DisableLocalAccess()` is
  called, so local clients must present a valid cookie.
- **Reject** (`dix/dispatch.c:3684-3707`, `SendConnSetup` with a non-NULL
  reason): Setup reply with `success = 0` + the reason string.
- Reason strings: `"Invalid MIT-MAGIC-COOKIE-1 key"`
  (`os/mitauth.c:82`), `"Authorization required, but no authorization
  protocol specified\n"` (`os/auth.c:211`), `"Authorization protocol not
  supported by server\n"` (`os/auth.c:207`).

## Design

### Component: `core_loop::auth` (new module in `yserver-core`)

Owns all cookie logic, isolated from the setup handshake:

```rust
pub struct AuthState {
    file: Option<PathBuf>,        // from LaunchOptions.auth_file
    inner: Mutex<Inner>,
}

struct Inner {
    last_mtime: Option<SystemTime>, // last stat seen (Xorg `lastmod`)
    ever_loaded: bool,              // latches once ≥1 cookie loaded (Xorg `loaded`)
    local_open: bool,               // host-ACL local-access toggle (see state machine)
    cookies: Vec<Vec<u8>>,          // MIT-MAGIC-COOKIE-1 data blobs, ADDITIVE
}

pub enum AuthVerdict {
    Allow,
    Reject(&'static str),           // reason string sent to client
}

impl AuthState {
    pub fn new(file: Option<PathBuf>) -> Arc<Self>;
    pub fn check(&self, proto_name: &[u8], proto_data: &[u8]) -> AuthVerdict;
}
```

- Built once in `yserver::run` from `opts.auth_file`, wrapped in `Arc`,
  passed into `setup_thread::spawn`.
- Setup threads call **only** `check()`; they never see the file or the
  cookie list.
- Initial state when `file == Some`: `local_open = true`, `cookies`
  empty, `ever_loaded = false` (matches Xorg: nothing enforced until the
  first successful load). When `file == None`: permanently `local_open`.

**Concurrency.** Setup threads are spawned per client (`setup_thread.rs:58`)
and `check()` runs on each before the core rendezvous, so there is no
lock-ordering hazard with the core. The `Mutex<Inner>` is held across
`stat` + (conditional) reload + compare. Steady state is `stat` +
constant-time compare under the lock — microseconds; a reload (file read
+ parse) only happens on an `mtime` change, so concurrent handshakes
serialize only on the rare reload. The auth file lives on tmpfs
(`/var/run/lightdm/...`, `/tmp`), so a slow-FS stall is not a practical
concern. Poisoning is recovered with `lock().unwrap_or_else(|e|
e.into_inner())` — the existing pattern at `setup_thread.rs:66` — so a
panic mid-reload cannot turn auth into permanent panic-on-connect.

### Loading

Reuse **only the low-level Xau record decoder** from `host_x11/pump.rs:316`
(`read_be_u16_record` / `read_record_field`, parsing `family, address,
number, name, data` as length-prefixed big-endian). Factor that decoder
into one shared helper. The *selection semantics* stay separate and are
intentionally different: the client reader filters by name **and**
display number; the server loader collects the `data` of **every**
`MIT-MAGIC-COOKIE-1` record and **ignores family / address /
display-number** (confirmed faithful — `os/auth.c:122-130` never consults
them server-side; that filtering is purely client-side).

**Parse-until-malformed, additive cookies (Xorg-faithful).** Xorg's
`LoadAuthorization` reads records via `XauReadAuth` until it returns NULL
and **adds** each MIT cookie to a list that is *never cleared on reload*
— only `ResetAuthorization` (server reset) clears it (`os/auth.c:122-136`,
`216`; `os/mitauth.c:51-68`). We match this:

- The loader walks records until one fails to decode (truncated tail),
  keeping every valid MIT cookie decoded before that point.
- On reload, newly-decoded cookies are **appended** to `cookies`, not
  replaced (dedup-on-insert is allowed but not required — duplicates are
  harmless). A cookie, once accepted, stays valid until server exit.

**Lazy reload + the local-access state machine.** `check()` `stat`s
`file` and decides whether to reload, reproducing Xorg's trigger
(`os/auth.c:166-175`) precisely — `last_mtime: Option<SystemTime>` is our
`lastmod` (`None` ≡ Xorg's `0`):

- `stat` **succeeds** and `mtime` is **strictly newer** than `last_mtime`
  (or `last_mtime == None`) → set `last_mtime = Some(mtime)`, reload.
  Note **strictly newer** (`>`), not "differs": an mtime *rollback*
  (file swapped for an older one) does **not** reload, matching Xorg.
- `stat` **fails**: if `last_mtime.is_some()`, set `last_mtime = None` and
  reload **once** (the loss transition). While it keeps failing
  (`last_mtime == None`) no further reloads fire — not a reparse on every
  connection.

The reload outcome then drives `local_open` exactly as Xorg's
`loadauth`/`loaded` logic (`os/auth.c:176-198`):

| Reload outcome | `cookies` | `local_open` | `ever_loaded` |
|---|---|---|---|
| `file` opens, ≥1 MIT record decoded | append them | **false** (enforce) | set true |
| `file` opens, 0 MIT records (empty / none parseable) | unchanged | **true** (reopen, even if previously loaded — Xorg quirk) | unchanged |
| `file` won't open, **and** `ever_loaded` | unchanged | **unchanged** (stays enforcing) | unchanged |
| `file` won't open, never loaded | unchanged | **true** | unchanged |

The security-relevant guarantee this fixes (vs. the earlier draft): once
the server has enforced, an auth file that later **vanishes or becomes
unreadable** keeps enforcing — it does **not** silently reopen. The
empty-file-reopens row is a deliberately-replicated Xorg quirk; documented
here so it's a conscious choice, and harmless on our Unix-only transport
where the no-`-auth` baseline is already local-open.

### Prerequisite fix: `write_setup_failed` is currently malformed

`write_setup_failed` (`x11/mod.rs:606-620`) writes `success`,
`lengthReason`, `major`, `minor`, then jumps straight to the reason
bytes. The X11 connection-setup **failed** prefix is **8 bytes** and the
helper omits the 2-byte `length` field at bytes 6–7 — the count of
4-byte units of (padded) reason data that follows
(`xConnSetupPrefix.length`; Xorg sets it in `dix/dispatch.c:3693-3700`).
The current reply is therefore off by two bytes; a real client (Xlib)
would read the first two reason bytes as `length` and then consume the
wrong number of trailing bytes. It is latent only because the lone
caller today (resource exhaustion, `setup_thread.rs:144`) is effectively
never hit.

**This must be fixed before reuse.** Corrected layout, written in the
client's byte order:

```
byte 0     success = 0
byte 1     lengthReason = n
bytes 2-3  protocol-major (11)
bytes 4-5  protocol-minor (0)
bytes 6-7  length = (n + pad) / 4     <-- currently missing
bytes 8..  reason string, padded to 4
```

A wire-layout unit test (both byte orders) lands with this fix; it
doubles as regression cover for the auth reject path.

### Validation in `run_setup`

Slotted in **after** `read_setup_request` (`setup_thread.rs:127`) and
after byte-order validation (which `read_setup_request` already performs
by rejecting a bad order marker), and **before** the `SetupAllocate`
rendezvous:

```rust
if let AuthVerdict::Reject(reason) =
    auth.check(&setup.auth_protocol_name, &setup.auth_protocol_data)
{
    debug!("client {} auth rejected: {reason}", id.0);
    x11::write_setup_failed(&mut stream, setup.byte_order, reason)?;
    return Ok(());            // drop stream → connection closes (Xorg disconnects too)
}
```

**Check ordering.** Xorg validates in the order endianness → request
length → protocol version → authorization
(`dix/dispatch.c:3785-3805`). yserver currently validates only
endianness and does no length/version rejection, so auth-after-endianness
is correct today. If length/version checks are added later, they **must
precede** the auth check, or malformed clients would get an auth-error
reason instead of the proper setup-failed reason.

### Verdict logic

`check()` first reloads under the state machine above, then decides.
Evaluated in order; first match wins. This mirrors Xorg, where a cookie
match (`CheckAuthorization` → valid XID) **or** a host-ACL allow
(`ClientAuthorized`, `os/connection.c:536-560`) admits the client, and a
reason is sent only when both fail:

| Condition | Verdict |
|---|---|
| `proto_name == "MIT-MAGIC-COOKIE-1"` and `proto_data` matches any loaded cookie | `Allow` (cookie accepted) |
| `local_open == true` | `Allow` (host-ACL local-access; covers `file == None`, empty-file, and never-loaded) |
| `proto_name` empty | `Reject("Authorization required, but no authorization protocol specified\n")` |
| `proto_name == "MIT-MAGIC-COOKIE-1"` (and no match above) | `Reject("Invalid MIT-MAGIC-COOKIE-1 key")` |
| otherwise (non-empty, unrecognized protocol) | `Reject("Authorization protocol not supported by server\n")` |

Reject strings are **byte-for-byte** Xorg's, including the trailing `\n`
on two of them (`os/auth.c:207,211`) and its absence on the cookie one
(`os/mitauth.c:82`).

Cookie match = equal length **and** constant-time byte compare
(hand-rolled, no new dependency — mirrors Xorg's `timingsafe_memcmp`;
socket-only makes timing attacks moot but it costs nothing to match).

### lightdm lockout risk (operational)

lightdm already passes `-auth /var/run/lightdm/root/:N`. Once honored,
**every lightdm session enforces auth**. If the cookie lightdm wrote
does not validate, the greeter black-screens (a *valid* file with a
non-matching cookie is the dangerous case — note that a *missing* or
*empty* file degrades to local-open per the state machine, so those
cannot lock anyone out). Mitigations:

- The reject path logs (at `debug`/`info`) which reason fired and the
  presented protocol name, so a lockout is diagnosable from
  `yserver-*.log` rather than a silent black screen.
- HW smoke under lightdm on bee is a **release gate** for this change,
  not optional.

## Faithfulness & deliberate deviations

- **Host ACL modeled as a boolean.** Xorg admits a no-/bad-cookie client
  when the host ACL allows it (`os/connection.c:536-560`). We do not
  implement `xhost`/ACL (deferred — see Non-goals), so we collapse "host
  ACL would allow this local client" to the single `local_open` flag.
  On a Unix-socket-only server every peer is local, so this is
  behaviorally exact for our transport; it becomes an approximation only
  if/when a TCP listener is added, at which point real ACL replaces it.
- **Replicated Xorg quirks (intentional):** the cookie list is additive
  across reloads (revocation requires server restart), and an auth file
  rewritten to *empty* reopens local access. Both match `os/auth.c`
  exactly and are called out so they are not mistaken for oversights.

## Testing

- **`write_setup_failed` wire layout (`yserver-protocol`):** assert the
  full 8-byte prefix incl. the `length` field, in **both** byte orders,
  against a fixture cross-checked with an Xlib/x11rb capture (external
  ground truth, not self-derived).
- **Unit (`auth` module):**
  - Record decoder against a hand-built Xau byte fixture, cross-checked
    against a real `xauth`-generated file (external ground truth — not
    self-derived arithmetic).
  - Verdict logic: one test per row of the table above.
  - Constant-time-eq correctness (equal, unequal-same-length,
    unequal-length).
  - **State-machine transitions** (the part most likely to be wrong):
    - never-loaded + missing/empty file ⇒ `local_open`, `Allow`.
    - load ≥1 cookie ⇒ enforce; matching cookie `Allow`, bogus `Reject`.
    - after enforcing, file **vanishes/unreadable** ⇒ **still enforcing**
      (does *not* reopen); previously-loaded cookie still `Allow`.
    - after enforcing, file rewritten **empty** ⇒ reopens (`local_open`),
      *and* old cookie still `Allow` (additive list) — the Xorg quirk.
    - reload **adds** a second cookie ⇒ both old and new `Allow`.
    - mixed valid+malformed: file = one valid MIT record then a truncated
      tail ⇒ the valid cookie loads and the server **enforces** (not
      reopen).
  - **Reload trigger** (pins the `os/auth.c:166-175` semantics):
    - mtime **rollback** (replace with an older-mtime file) ⇒ **no
      reload**; prior cookies/state retained.
    - `stat` failure is **one-shot**: enforce, delete the file, then issue
      several `check()`s while it's gone ⇒ exactly one reload attempt
      (state stays enforcing); when the file reappears with a newer mtime,
      the next `check()` reloads.
- **Integration:** listener seeded with a known cookie; raw client sends
  matching / mismatching / empty / wrong-protocol auth; assert `Allow` vs
  the exact reject reason **on the wire** (Setup `success = 0`, correct
  big-endian/little-endian prefix, reason string), and that the server
  **closes the connection** after a reject.
- **HW smoke (gate):** lightdm session comes up on bee; a client with the
  correct `$XAUTHORITY` connects; a client with a bogus cookie is
  refused; `xhost`-free local session under `just startx` (which uses
  real xinit-generated cookie) works.

## Future work

- **xhost / host-based ACL** (`ChangeHosts`, `ListHosts`,
  `SetAccessControl`, `si:localuser:`) — add alongside TCP listener
  support, where it becomes meaningful.
