# Install Contract + Installed Documentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the yserver tree installable by a distro packager — a `DESTDIR`/`PREFIX`-aware `just install` that compiles nothing, plus man pages, a canonical setup guide, a lightdm example and a `tmpfiles.d` snippet.

**Architecture:** Three separated concerns. `just man` compiles scdoc sources to roff in `target/man/`. `cargo build --release` produces binaries. `just install` only *copies* pre-existing artefacts into `$DESTDIR$PREFIX`, preflighting every input before the first write. A shell smoke test (`tools/install-smoke.sh`) drives the install recipe in CI.

**Tech Stack:** `just` 1.57 (Justfile), `scdoc` for man pages, POSIX `install(1)` (no GNU-only `-D`), Rust `build.rs` for the version stamp, GitHub Actions for CI.

**Spec:** `docs/superpowers/specs/2026-07-28-packaging-install-contract-design.md`

---

## Configuration interface — read this before writing any recipe

`PREFIX`, `DESTDIR`, `TARGETDIR` and `TMPFILESDIR` are **top-level `just`
variables**, not recipe parameters. This is load-bearing and was
verified empirically, not assumed:

```
# Justfile
PREFIX := env_var_or_default("PREFIX", "/usr/local")
```

Recipe parameters in `just` are **positional**. Had these been declared
as `install PREFIX="/usr/local" DESTDIR="":`, then

```sh
just install DESTDIR=/tmp/stage PREFIX=/usr
```

would bind `PREFIX` to the literal string `DESTDIR=/tmp/stage` and
`DESTDIR` to `PREFIX=/usr`. Confirmed by running it:

```
PREFIX=[DESTDIR=/tmp/stage] DESTDIR=[PREFIX=/usr] TARGETDIR=[]
```

With top-level variables plus `env_var_or_default`, all three of these
work, and command-line assignment beats the environment:

| Form | Use |
| --- | --- |
| `DESTDIR=$pkgdir PREFIX=/usr just install` | environment — make-like, what packagers reach for first |
| `just PREFIX=/usr DESTDIR=$pkgdir install` | `just` assignment, **before** the recipe name |
| `just --set PREFIX /usr install` | explicit |

**Every invocation in this plan uses the environment form.** Do not write
`just install PREFIX=...` — it silently does the wrong thing rather than
erroring.

**Ground rules for every task:**
- Before committing: `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --locked` (Rust tasks only — tasks 2 and 3).
- Commit after each task. Do not amend.
- Branch is `feat/install-contract-docs`, already holding the spec commits. It is squash-merged, so intermediate forward references between commits never reach `master`.
- Install `shellcheck` and `scdoc` first — task 1 and task 5 need them and neither is present by default: `sudo pacman -S --needed shellcheck scdoc`.

---

## File Structure

| File | Status | Responsibility |
| --- | --- | --- |
| `starty` | modify | portable across supported shells; FreeBSD consoles; leak-free cleanup |
| `crates/yserver/build.rs` | modify | honour a pre-set `YSERVER_GIT_COMMIT` |
| `crates/yserver/src/lib.rs` | modify | fix two stale signal comments |
| `crates/yserver/src/input_thread.rs` | modify | two stale comments, one wrong log line, one stale test doc |
| `crates/yserver-core/src/core_loop/message.rs` | modify | two stale doc comments |
| `crates/yserver-core/src/backend/trait_def.rs` | modify | two stale doc comments |
| `examples/lightdm-99-yserver.conf.in` | create | lightdm drop-in template, `@PREFIX@` |
| `examples/yserver.tmpfiles` | create | `/tmp/.X11-unix` mode declaration |
| `docs/man/yserver.1.scd` | create | `yserver(1)` source |
| `docs/man/starty.1.scd` | create | `starty(1)` source |
| `docs/setup.md` | create | canonical setup guide; installed to `share/doc` |
| `README.md` | modify | replace setup prose with a pointer |
| `Justfile` | modify | variables + `man`, `install`, `install-local`, `install-smoke` |
| `tools/install-smoke.sh` | create | the install contract's test |
| `.github/workflows/ci.yml` | modify | run the contract checks |
| `Cargo.toml` | modify | `repository` URL |
| `crates/yserver/Cargo.toml` | modify | drop the `ynest` bin, `autobins = false` |
| `crates/yserver/src/bin/ynest.rs` | delete | ynest is removed, not merely uninstalled |

Task order builds inputs before consumers: launcher and Rust fixes, then
the static files the install recipe copies, then the recipes and their
test, then CI.

---

### Task 1: `starty` — portability and leak-free cleanup

Two separate defects. The TTY guard only matches Linux VT names, so the
launcher we ship refuses to run on FreeBSD. And `cleanup()` runs
unguarded commands under `set -eu`, so it can abort partway and leak the
cookie. Spec section 2a.

**Files:**
- Modify: `starty` — header comment, TTY guard (~:46-48), trap/cleanup (~:98-105), socket wait (~:107-116), session env (~:119-124)

- [ ] **Step 1: Establish the real red — do not rely on shellcheck for it**

`shellcheck` does **not** flag `seq` or `env -u` as non-POSIX, so a clean
`shellcheck` run proves nothing about this task. The genuine failing
behaviours are the guard and the cleanup. Prove both:

```bash
sh -n starty && echo "syntax OK"
grep -n 'dev/tty\[0-9\]\|seq 30\|env -u' starty
```
Expected: `syntax OK`, and three hits showing the Linux-only guard, the
`seq` call and the `env -u` prefix.

Now prove the cleanup bug directly, in isolation — this is the one that
loses a cookie:

```bash
sh -c 'set -eu
cleanup() { kill -TERM 999999 2>/dev/null; echo "REACHED-CLEANUP-END"; }
trap cleanup EXIT
exit 0'
```
Expected: **no** `REACHED-CLEANUP-END` output. Under `set -e`, the failing
`kill` aborts the handler before the later lines. That is exactly the
shape of the real `cleanup()`, and it means a server that already exited
leaves the `~/.Xauthority` entry and the temp auth file behind.

- [ ] **Step 2: Widen the TTY guard to FreeBSD consoles**

Replace the guard's first arm so the case reads:

```sh
case "$(tty 2>/dev/null || true)" in
    /dev/tty[0-9]*) ;;      # Linux virtual console
    /dev/ttyv[0-9a-f]*) ;;  # FreeBSD vt(4) console (ttyv0..ttyvf)
```

Leave the `*)` arm's logic alone; only retitle its hint:

```sh
       echo "       switch to a free VT (Linux: Ctrl-Alt-F2) and run starty there." >&2
```

- [ ] **Step 3: Make cleanup idempotent and non-failing, and arm the trap earlier**

Two changes. First, every command in `cleanup()` gets `|| :` so `set -e`
cannot abort the handler, and the pid is guarded because the trap now
fires before the server exists:

```sh
cleanup() {
    [ -z "${yserver_pid:-}" ] || kill -TERM "$yserver_pid" 2>/dev/null || :
    [ -z "${yserver_pid:-}" ] || wait "$yserver_pid" 2>/dev/null || :
    [ -z "${userauth:-}" ] || xauth -f "$userauth" remove ":$display" 2>/dev/null || :
    [ -z "${authfile:-}" ] || rm -f "$authfile" || :
}
```

Second, move `trap cleanup EXIT INT TERM` from after the server launch to
**immediately after `authfile` is created** — before the two `xauth add`
calls. Today a failure in either `xauth add` leaks the temp file and
possibly a half-written authority entry, because the trap is not armed
yet. The result should read:

```sh
authfile=$(mktemp /tmp/yserver-startx-auth.XXXXXX)
userauth="${XAUTHORITY:-$HOME/.Xauthority}"
trap cleanup EXIT INT TERM
cookie=$(mcookie)
```

with the later standalone `trap cleanup EXIT INT TERM` line deleted.
`cleanup()` must be *defined* above this point; move the function
definition up if it is not already.

- [ ] **Step 4: Replace `seq` with a POSIX counter**

Replace the socket-wait loop:

```sh
# Wait for the socket (cap at ~30 s like the recipe).
waited=0
while [ "$waited" -lt 30 ]; do
    [ -S /tmp/.X11-unix/X$display ] && break
    sleep 1
    waited=$((waited + 1))
    if ! kill -0 "$yserver_pid" 2>/dev/null; then
        echo "starty: yserver exited before the socket came up — see $server_log" >&2
        exit 1
    fi
done
```

- [ ] **Step 5: Replace `env -u` with `unset` + `export`**

The old code built a command prefix as a string and relied on
word-splitting. Replace it and both call sites:

```sh
# --- Run the session. ---------------------------------------------------
# Belt-and-braces: a real VT wouldn't have WAYLAND_* set anyway.
unset WAYLAND_DISPLAY WAYLAND_SOCKET
export XDG_SESSION_TYPE=x11
export XAUTHORITY="$userauth"
export DISPLAY=":$display"
if [ "$have_client" -eq 1 ]; then
    "$client_bin" "$@" >"$session_log" 2>&1
else
    sh "$xinitrc" >"$session_log" 2>&1
```

Exporting into the current shell is safe: `cleanup()` passes `xauth` an
explicit `-f "$userauth"` and an explicit `":$display"`, and reads
neither variable.

- [ ] **Step 6: Correct the header comment**

The header calls the file a POSIX script. It uses `[ -S ]`, `mktemp` and
`trap ... EXIT`, which are not all strict POSIX interfaces — they are
simply available everywhere this runs. Replace the `#!/bin/sh` line's
following description of portability, and add the platform note. After
the existing "Runs STANDALONE from a bare TTY" paragraph, add:

```sh
# Portable across the shells and platforms yserver supports (dash, bash,
# BusyBox ash; Linux and FreeBSD) rather than strictly POSIX — it relies
# on mktemp, [ -S ] and trap EXIT, which are universal in practice.
#
# Console takeover (which stops the kernel from turning Ctrl-C in an X
# client into a signal that kills the session) is Linux-only — it is
# compiled out elsewhere. FreeBSD gets a working launcher without it.
```

- [ ] **Step 7: Verify green**

The cleanup fix, tested the same way as the red:

```bash
sh -c 'set -eu
cleanup() { [ -z "${p:-}" ] || kill -TERM "$p" 2>/dev/null || :; echo "REACHED-CLEANUP-END"; }
trap cleanup EXIT
p=999999
exit 0'
```
Expected: `REACHED-CLEANUP-END`.

Then the script itself, across shells:

```bash
shellcheck -s sh starty && echo SHELLCHECK-OK
for s in sh dash bash; do
    command -v "$s" >/dev/null 2>&1 && { "$s" ./starty --help >/dev/null && echo "$s OK"; }
done
command -v busybox >/dev/null 2>&1 && { busybox sh ./starty --help >/dev/null && echo "busybox OK"; }
```
Expected: `SHELLCHECK-OK` plus an `OK` per shell.

The pty guard must still refuse — this is security-relevant and must not
have regressed:
```bash
./starty; echo "exit=$?"
```
Expected: `starty: must be run from a TTY (got: /dev/pts/N)`, `exit=1`.

And no temp file was leaked by that refusal:
```bash
ls /tmp/yserver-startx-auth.* 2>/dev/null || echo "no leak (good)"
```
Expected: `no leak (good)`.

- [ ] **Step 8: Commit**

```bash
git add starty
git commit -m "fix(starty): FreeBSD consoles, leak-free cleanup, portable constructs

Three fixes to the launcher we install on every supported platform.

The TTY guard matched /dev/tty[0-9]* only, so it refused to run on
FreeBSD vt(4) consoles (/dev/ttyv*).

cleanup() ran unguarded commands under set -eu, so a server that had
already exited made the failing kill abort the handler before it removed
the ~/.Xauthority entry and the temp auth file. Every command is now
|| :-guarded, and the trap is armed immediately after mktemp instead of
after the server launch, so a failing xauth add no longer leaks either.

seq(1) and env -u are replaced with a while counter and unset + export."
```

---

### Task 2: `build.rs` version-stamp override

A release tarball has no `.git`, so every packaged binary would report
`--version` as `unknown`. Spec section 5.

**Files:**
- Modify: `crates/yserver/build.rs` — `emit_git_commit` (~:97-127)

- [ ] **Step 1: Write the failing test**

`build.rs` is not compiled into a test target, so this is behavioural.
Note the `&&` — without it a failed build leaves the previous binary in
place and the check passes on stale output.

```bash
YSERVER_GIT_COMMIT=deadbeefcafe cargo build --locked --bin yserver \
    && ./target/debug/yserver --version
```

Expected FAILURE: prints the real git hash, **not** `deadbeefcafe`,
because the variable is currently ignored.

- [ ] **Step 2: Honour the pre-set variable**

Replace the opening of `emit_git_commit`:

```rust
fn emit_git_commit(manifest_dir: &Path) {
    let commit = match run_git(manifest_dir, &["rev-parse", "--short=12", "HEAD"]) {
```

with:

```rust
fn emit_git_commit(manifest_dir: &Path) {
    // Packagers build from an exported tarball with no `.git`, where the
    // git probe below yields "unknown". Let them stamp the release commit
    // explicitly instead. `rerun-if-env-changed` is required or a rebuild
    // in a warm target directory keeps the previous stamp baked in.
    println!("cargo:rerun-if-env-changed=YSERVER_GIT_COMMIT");
    if let Ok(preset) = std::env::var("YSERVER_GIT_COMMIT") {
        let preset = preset.trim();
        if !preset.is_empty() {
            println!("cargo:rustc-env=YSERVER_GIT_COMMIT={preset}");
            return;
        }
    }

    let commit = match run_git(manifest_dir, &["rev-parse", "--short=12", "HEAD"]) {
```

Everything after that — the `match` arms, the `rustc-env` emission and
the `rerun-if-changed` registrations — is unchanged.

- [ ] **Step 3: Update the doc comment**

Replace the comment above `emit_git_commit` with:

```rust
/// Emit `YSERVER_GIT_COMMIT` (consumed by `src/version.rs` via `env!`).
///
/// A pre-set `YSERVER_GIT_COMMIT` in the environment wins — that is how a
/// distro package stamps the release commit when building from a tarball
/// with no `.git`. Otherwise this is the 12-char `HEAD` hash, suffixed
/// `-dirty` when the working tree has uncommitted tracked changes, or
/// `"unknown"` outside a git checkout. Rerun triggers are registered so
/// the stamp tracks HEAD moves (commit / checkout / merge) and changes to
/// the override variable.
```

- [ ] **Step 4: Verify green, including the warm-target-dir case**

```bash
YSERVER_GIT_COMMIT=deadbeefcafe cargo build --locked --bin yserver \
    && ./target/debug/yserver --version
```
Expected: `yserver 1.4.0 (deadbeefcafe)`

Same target directory, different value — this is the part that only
passes because of `rerun-if-env-changed`:
```bash
YSERVER_GIT_COMMIT=feedfacefeed cargo build --locked --bin yserver \
    && ./target/debug/yserver --version
```
Expected: `yserver 1.4.0 (feedfacefeed)`. If it still says
`deadbeefcafe`, the `rerun-if-env-changed` line is missing or misspelled.

Unset — the checkout path must be unchanged:
```bash
cargo build --locked --bin yserver && ./target/debug/yserver --version
```
Expected: the real short hash.

- [ ] **Step 5: Format, lint, test**

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test --locked
```
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver/build.rs
git commit -m "build: let YSERVER_GIT_COMMIT override the git probe

Distro packages build from an exported tarball with no .git, so the
version stamp fell back to \"unknown\" and every packaged binary would
report yserver X.Y.Z (unknown). A pre-set YSERVER_GIT_COMMIT now wins,
letting a spec file or PKGBUILD stamp the release commit.

Also emits rerun-if-env-changed=YSERVER_GIT_COMMIT; without it a rebuild
in a warm target directory keeps the previous stamp."
```

---

### Task 3: Fix the stale signal comments

Six sites across four files claim `SIGUSR1`/`SIGUSR2` trigger the
diagnostic dumps. On the KMS server they do not: `SIGUSR1` is VT release,
`SIGUSR2` is VT acquire (`lib.rs:484-497`), and the dumps are reachable
**only** via Ctrl-Alt-Enter and Ctrl-Alt-F12. Task 5 writes a SIGNALS man
page section; left in place these comments would be copied into it. One
log line also names a key that does not trigger it.

**Files:**
- Modify: `crates/yserver/src/lib.rs:769`, `:772`
- Modify: `crates/yserver/src/input_thread.rs:457`, `:468`, `:472`, `:1430`
- Modify: `crates/yserver-core/src/core_loop/message.rs:183`, `:187`
- Modify: `crates/yserver-core/src/backend/trait_def.rs:609`, `:616`

- [ ] **Step 1: Confirm the ground truth first**

```bash
sed -n '480,500p' crates/yserver/src/lib.rs
grep -rn "Message::DumpScanout\|Message::DumpDrawables" crates/yserver/src/ | grep -v test
```

Expected: the signalfd loop maps `SIGUSR1 -> Message::VtRelease` and
`SIGUSR2 -> Message::VtAcquire`; the only *senders* of `DumpScanout` /
`DumpDrawables` are in `input_thread.rs`, the hotkey path. No signal
handler sends either. If that does not hold, stop — this task and task
5's SIGNALS section both depend on it.

- [ ] **Step 2: Fix `lib.rs` — the worst offenders**

These sit in the function that blocks the signals, so they are the most
likely to be believed. At `:769-770`, replace:

```rust
    // SIGUSR1 → diagnostic scanout dump. Blocked so signalfd consumes
    // it instead of the default-action (which would terminate us).
```
with:
```rust
    // SIGUSR1 → VT release (see the signalfd loop). Blocked so signalfd
    // consumes it instead of the default action, which would terminate us.
```

At `:772-773`, replace:

```rust
    // SIGUSR2 → diagnostic drawable-storage dump (root + COW + every
    // redirected backing). Same blocking rationale as SIGUSR1.
```
with:
```rust
    // SIGUSR2 → VT acquire. Same blocking rationale as SIGUSR1.
```

- [ ] **Step 3: Fix `input_thread.rs`**

At `:457`, replace `// core to dump the scanout (same code path as SIGUSR1).` with:
```rust
                // core to dump the scanout.
```

At `:468`, replace `// per-drawable storage (same code path as SIGUSR2).` with:
```rust
                // per-drawable storage.
```

At `:469`, replace `// D keypress itself, ask the core to dump` with:
```rust
                // F12 keypress itself, ask the core to dump
```

At `:472`, the log line names the wrong key — this hotkey is
`Hotkey::DumpDrawables`, which is Ctrl-Alt-F12. Replace:
```rust
                log::info!("yserver: Ctrl-Alt-D pressed — dumping drawables");
```
with:
```rust
                log::info!("yserver: Ctrl-Alt-F12 pressed — dumping drawables");
```

At `:1430`, the test's doc comment says `(Ctrl-Alt-F12 → SIGUSR2 path)`.
Replace that parenthetical:
```rust
    /// for the per-drawable storage dump (Ctrl-Alt-F12 hotkey path).
```

- [ ] **Step 4: Fix `message.rs`**

At `:183` and `:187`, the `DumpScanout` / `DumpDrawables` doc comments open
`/// SIGUSR1 received —` and `/// SIGUSR2 received —`. Replace those
opening phrases:

```rust
    /// Ctrl-Alt-Enter pressed — the backend should dump the current scanout
```
and
```rust
    /// Ctrl-Alt-F12 pressed — the backend should dump the storage content
```

Keep the remainder of each comment.

Leave `:172` and `:175` alone — they document `VtRelease`/`VtAcquire` and
correctly reference SIGUSR1/SIGUSR2. Read them to confirm before touching
anything nearby.

- [ ] **Step 5: Fix `trait_def.rs`**

At `:609`, replace `/// (SIGUSR1 on KMS). Default no-op for backends that don't` with:
```rust
    /// (Ctrl-Alt-Enter on KMS). Default no-op for backends that don't
```

At `:616`, replace `/// (SIGUSR2 on KMS). Default no-op for backends that don't` with:
```rust
    /// (Ctrl-Alt-F12 on KMS). Default no-op for backends that don't
```

- [ ] **Step 6: Verify no stale claim survives**

```bash
grep -rn "SIGUSR" crates/ --include=*.rs | grep -i "dump\|scanout\|drawable"
```
Expected: **no output.** This grep is the task's completion test; if it
prints anything, a site was missed.

```bash
cargo +nightly fmt
cargo clippy --all-targets -- -D warnings
cargo test --locked
```
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/yserver/src/lib.rs crates/yserver/src/input_thread.rs \
        crates/yserver-core/src/core_loop/message.rs \
        crates/yserver-core/src/backend/trait_def.rs
git commit -m "docs(comments): stop claiming SIGUSR1/2 trigger the diagnostic dumps

On the KMS server SIGUSR1 is VT release and SIGUSR2 is VT acquire; the
scanout and drawable dumps are reachable only via Ctrl-Alt-Enter and
Ctrl-Alt-F12. Six comments across four files still described an older
design where signals drove them — including two in the function that
blocks those very signals — and one log line announced Ctrl-Alt-D for
what is actually Ctrl-Alt-F12."
```

---

### Task 4: Example configs

Spec sections 4 and 4a. These are inputs to the install recipe, so they
land before it.

**Files:**
- Create: `examples/lightdm-99-yserver.conf.in`, `examples/yserver.tmpfiles`

- [ ] **Step 1: Write `examples/lightdm-99-yserver.conf.in`**

```ini
# Example lightdm drop-in: use yserver as the X server for graphical
# logins. Install as /etc/lightdm/lightdm.conf.d/99-yserver.conf, then
# restart lightdm from a free console:
#
#   sudo install -m644 99-yserver.conf /etc/lightdm/lightdm.conf.d/
#   sudo systemctl restart lightdm
#
# lightdm appends X-style arguments (:N -seat seatN -auth FILE
# -nolisten tcp vtN -novtswitch), all of which yserver accepts. It waits
# for the SIGUSR1 readiness handshake before starting the greeter.
#
# lightdm is used because its X server command is configurable; gdm and
# sddm do not expose that.
#
# Do NOT point this at starty. starty is the console launcher: it forks
# the server rather than exec'ing it, so the readiness signal would reach
# starty instead of lightdm, and its TTY guard rejects a DM-spawned
# process outright. For server logs, use a wrapper that execs yserver.

[Seat:*]
xserver-command=@PREFIX@/bin/yserver
```

- [ ] **Step 2: Write `examples/yserver.tmpfiles`**

```
# X11 socket directory. yserver creates /tmp/.X11-unix if it is missing,
# but a user-run server would create it owned by that user and subject to
# their umask; it must be root-owned and sticky so every user can bind a
# socket there. Systems with another X server installed already ship an
# equivalent line — this is for machines where yserver is the only one.
#
# Installed to $TMPFILESDIR (default $PREFIX/lib/tmpfiles.d). Set
# TMPFILESDIR= (empty) to skip it on non-systemd and non-Linux systems,
# which must arrange the equivalent themselves — see docs/setup.md.
d /tmp/.X11-unix 1777 root root -
```

- [ ] **Step 3: Verify the tmpfiles syntax**

```bash
systemd-tmpfiles --dry-run --create examples/yserver.tmpfiles && echo TMPFILES-OK
```
Expected: `TMPFILES-OK`. Syntax errors are reported with a line number.
(Skip on non-systemd — the file is data, not executed here.)

- [ ] **Step 4: Commit**

```bash
git add examples/lightdm-99-yserver.conf.in examples/yserver.tmpfiles
git commit -m "feat(examples): lightdm drop-in template and tmpfiles.d snippet

The lightdm config is a template: install substitutes @PREFIX@ so the
shipped file names a binary path that exists on that system. It also
says explicitly not to point xserver-command at starty, which would
break the readiness handshake.

The tmpfiles snippet forces /tmp/.X11-unix to root-owned 1777. yserver
creates the directory if missing but never sets its mode, so on a machine
with no other X server installed a user-run server leaves it owned by
that user, locking out every other UID."
```

---

### Task 5: Man page sources

Spec section 2. **Every factual claim below was verified against the
code** — an earlier draft of this plan got the default display, the
`RUST_LOG` default and the `YSERVER_MODE` syntax all wrong. Do not
"improve" these values without re-reading the source.

**Files:**
- Create: `docs/man/yserver.1.scd`, `docs/man/starty.1.scd`

- [ ] **Step 1: Re-verify the three values that were previously wrong**

```bash
grep -n "DEFAULT_DISPLAY" crates/yserver/src/launch.rs | head -2
grep -n "default_filter_or" crates/yserver/src/bin/yserver.rs
sed -n '/fn parse_mode_spec/,/^}/p' crates/yserver/src/drm/modeset.rs
```
Expected: `DEFAULT_DISPLAY: u16 = 7`; `default_filter_or("info")`; and a
`parse_mode_spec` that does `split_once('x')` then parses both halves as
`u16` — so `WIDTHxHEIGHT` only, no `@HZ` suffix. The man pages below
state exactly these. If any differs, fix the man page, not the code.

- [ ] **Step 2: Write `docs/man/yserver.1.scd`**

```
yserver(1)

# NAME

yserver - X11 server for Linux KMS/DRM

# SYNOPSIS

*yserver* [:_display_] [vt_N_] [-auth _file_] [-displayfd _fd_]
	[-layout _layout_] [--version]

# DESCRIPTION

*yserver* is an X11 server written from scratch in Rust. It is not an Xorg
derivative. It drives atomic KMS/DRM directly and renders with Vulkan.

There is no seat manager. The server process opens _/dev/dri/*_ and
_/dev/input/event*_ itself, so it needs read/write access to both, and it
requires a working Vulkan driver. How that access is obtained depends on
how the server is started:

*Launched by a display manager*
	The server runs as root and already has access. See
	_@DOCDIR@/examples/lightdm-99-yserver.conf_.

*Launched by the user from a console*
	Use *starty*(1). The invoking user needs the _video_ and _input_
	groups, or equivalent seat ACLs:

	```
	sudo usermod -aG video,input $USER
	```

	then log out and back in.

# OPTIONS

:_display_
	Display number to serve, e.g. *:1*. A bare number is also accepted.
	Defaults to *:7*.

vt_N_
	Use virtual console _N_, e.g. *vt7*.

-auth _file_
	Read the authority file used to validate connecting clients. This is
	the server's own copy; it is not the client-side _XAUTHORITY_.

-displayfd _fd_
	Write the chosen display number to _fd_ once the socket is listening,
	instead of taking the display from the command line.

-layout _layout_
	XKB layout name, e.g. *be*.

--version, -version
	Print the version and the commit it was built from, then exit.

## ACCEPTED FOR COMPATIBILITY, IGNORED

Display managers and *xinit*(1) pass these unconditionally. *yserver*
parses them so startup does not fail, but they have no effect:

-seat _seat_
	Parsed and discarded. Input is always assigned to *seat0*.

-novtswitch
	No-op.

-nolisten _protocol_, -config _file_, -background _spec_
	Parsed with their value and discarded. TCP is never listened on
	regardless of *-nolisten*.

# KEY BINDINGS

*Ctrl-Alt-Backspace*
	Terminate the server immediately and return to the console.

*Ctrl-Alt-F1* .. *Ctrl-Alt-F11*
	Switch to virtual console 1 to 11.

*Ctrl-Alt-Enter*
	Write the current scanout to a PPM file in the working directory.

*Ctrl-Alt-F12*
	Write every drawable's storage to PPM files in the working directory.

# SIGNALS

*SIGUSR1* (outgoing, to the parent process)
	Sent once when the server is ready to accept clients, but only if
	*SIGUSR1* was inherited with disposition *SIG_IGN* — the convention a
	display manager uses to request the readiness handshake. *lightdm*
	relies on this.

*SIGUSR1* (incoming)
	Release the virtual console. Part of the VT handover handshake, and
	unrelated to the readiness signal above.

*SIGUSR2* (incoming)
	Acquire the virtual console.

*SIGTERM*, *SIGINT*, *SIGHUP*
	Shut down, restoring the console.

The diagnostic dumps are *not* signal-driven; they are the key bindings
above.

# ENVIRONMENT

*RUST_LOG*
	Log filter. Defaults to *info*; *warn* is quieter, *debug* and *trace*
	are for diagnosis.

*YSERVER_DRM_DEVICE*
	Path to the DRM device to use instead of auto-selecting one, e.g.
	_/dev/dri/card1_.

*YSERVER_MODE*
	Force a mode instead of the connector's preferred mode, as
	_WIDTHxHEIGHT_ — for example *1920x1080*. A refresh-rate suffix is not
	accepted.

*YSERVER_ALLOW_SOFTWARE_VULKAN*
	Set to *1* to permit a software Vulkan implementation such as
	lavapipe. Refused by default, being far too slow for interactive use.

Further *YSERVER_*\* variables exist for development tracing and are
deliberately not documented here; see the project documentation.

# FILES

_/tmp/.X11-unix/X<display>_
	The listening socket.

_@DOCDIR@/setup.md_
	Setup guide: dependencies, device access, display manager and console
	startup, troubleshooting.

_@DOCDIR@/examples/lightdm-99-yserver.conf_
	Example lightdm drop-in.

# SEE ALSO

*starty*(1), *Xserver*(1), *xauth*(1), *lightdm*(1)

# BUGS

Report at https://github.com/joske/yserver/issues
```

- [ ] **Step 3: Write `docs/man/starty.1.scd`**

```
starty(1)

# NAME

starty - start yserver on a free display and run a session

# SYNOPSIS

*starty* [_client_ [_client_args_...]]

# DESCRIPTION

*starty* is to *yserver*(1) what *startx*(1) is to Xorg: it brings a
server up on a free display, runs a session against it, and tears the
server down when the session exits.

With no _client_, it runs _~/.xinitrc_, falling back to
_/etc/X11/xinit/xinitrc_. With a _client_, it resolves it via *PATH* and
runs it with the given arguments, no xinitrc needed.

It must be run from a virtual console. *yserver* drives KMS directly and
needs a real controlling console, so *starty* refuses to run from a
pseudo-terminal — over SSH, or in a terminal emulator inside another
graphical session. Switch to a free console first (on Linux,
*Ctrl-Alt-F2*).

The invoking user needs access to _/dev/dri/*_ and _/dev/input/event*_;
see *yserver*(1).

*starty* is for console use only. Do not use it as a display manager's X
server command: it forks the server rather than exec'ing it, so the
readiness handshake would never reach the display manager.

# OPERATION

. Scan _/tmp/.X11-unix/_ and pick the lowest free display number.
. Mint an MIT-MAGIC-COOKIE-1 with *mcookie*(1).
. Write the cookie to an unguessable temporary file and pass it to the
  server as *-auth*. This is the server's copy, used only to validate
  connecting clients.
. Add the same cookie to the user's authority file, keyed to the chosen
  display, so clients in the session authenticate — and so a second
  console login can reach the display with a bare *DISPLAY=:N*.
. Start the server and wait up to 30 seconds for its socket.
. Run the session with *DISPLAY*, *XAUTHORITY* and *XDG_SESSION_TYPE=x11*
  set, and *WAYLAND_DISPLAY*/*WAYLAND_SOCKET* unset.
. On exit, terminate the server, remove the display's entry from the
  user's authority file, and delete the temporary server copy.

*XDG_RUNTIME_DIR* is deliberately *not* overridden, so the session
inherits the console login's real _/run/user/UID_ and its *systemd
--user* instance. A _~/.xinitrc_ that repoints *XDG_RUNTIME_DIR* at a
temporary directory will break agent and keyring sockets.

# OPTIONS

-h, --help
	Print usage and exit.

# ENVIRONMENT

*RUST_LOG*
	Passed to the server. *starty* defaults it to *warn*, which is quieter
	than *yserver*'s own default of *info*.

*XAUTHORITY*
	Authority file to add the session cookie to. Defaults to
	_~/.Xauthority_.

*XINITRC*
	Alternative xinitrc path, used when no _client_ is given.

# FILES

_$XDG_STATE_HOME/yserver/yserver.log_
	Server output. _XDG_STATE_HOME_ defaults to _~/.local/state_.

_$XDG_STATE_HOME/yserver/session.log_
	Session output.

# REQUIREMENTS

*xauth*(1) and *mcookie*(1) must be on *PATH*, along with *yserver*(1).

# PLATFORM NOTES

Works on Linux virtual consoles (_/dev/ttyN_) and FreeBSD *vt*(4)
consoles (_/dev/ttyvN_). Console takeover — which stops the kernel from
turning *Ctrl-C* in an X client into a signal that kills the whole
session — is implemented on Linux only.

# SEE ALSO

*yserver*(1), *startx*(1), *xauth*(1), *mcookie*(1)
```

- [ ] **Step 4: Commit**

```bash
git add docs/man/yserver.1.scd docs/man/starty.1.scd
git commit -m "docs(man): add yserver(1) and starty(1) scdoc sources

Only the .scd sources are committed; the man recipe added next renders
them to target/man/.

Documents -seat and -novtswitch as accepted-and-ignored, because that is
what the code does, and separates the three meanings of SIGUSR1:
outgoing readiness to the parent, incoming VT release, and (SIGUSR2)
incoming VT acquire. Values checked against the source — the default
display is :7, RUST_LOG defaults to info, and YSERVER_MODE takes
WIDTHxHEIGHT with no refresh suffix."
```

---

### Task 6: `docs/setup.md` and README trim

Spec section 3. The README's dependency lists move here rather than being
copied. This documents `just install`, which task 7 adds — a forward
reference within the branch, resolved by the next commit and invisible
after the squash merge.

**Files:**
- Create: `docs/setup.md`
- Modify: `README.md:135-213`

- [ ] **Step 1: Read what is being moved, so nothing is lost**

```bash
sed -n '135,213p' README.md
```
Confirm you can see: the group-access note, the four `####` dependency
blocks (Arch/Ubuntu/Alpine/FreeBSD), the lightdm section, the `starty`
section and the keybind list. All of it must appear in `docs/setup.md`
before the README is cut.

- [ ] **Step 2: Write `docs/setup.md`**

````markdown
# Setting up yserver

This guide covers installing yserver, giving it access to the hardware it
needs, and starting a session — from a display manager or directly from a
console.

Installed copies live at `$prefix/share/doc/yserver/setup.md`, alongside
the example configs referenced below.

## Packages

- **Arch** — `yserver` (tagged releases) and `yserver-git` (tracks
  `master`, maintained by a third party) on the AUR.
- **Fedora / EL** — see <https://github.com/joske/yserver-packaging>.
- **Anything else** — build from source, below.

## Requirements

- A GPU with a working Vulkan driver. A software implementation such as
  lavapipe is refused by default; it is far too slow to be usable.
- Linux with atomic KMS/DRM, or FreeBSD.
- Access to `/dev/dri/*` and `/dev/input/event*` — see
  [Device access](#device-access).

## Build from source

A recent stable Rust toolchain plus:

### Arch

```sh
sudo pacman -S --needed just gcc libxshmfence libxkbcommon libinput shaderc systemd-libs fontconfig pkgconf mesa scdoc
```

### Ubuntu / Debian

```sh
sudo apt install just gcc libxshmfence-dev libxkbcommon-dev libinput-dev glslc libudev-dev libfontconfig-dev libgbm-dev scdoc
```

### Alpine

```sh
export RUSTFLAGS="-C target-feature=-crt-static"
apk add gcc musl-dev fontconfig-dev freetype-dev libxshmfence-dev libxkbcommon-dev libinput-dev shaderc mesa-dev scdoc
```

### FreeBSD

```sh
doas pkg install -y shaderc fontconfig libudev-devd scdoc GhostBSD-bzip2-dev GhostBSD-zlib-dev
```

Then build and install:

```sh
cargo build --release --bin yserver
just man
sudo PREFIX=/usr/local just install
```

Or in one step, which builds, renders the man pages and installs to
`/usr/local` with `sudo`:

```sh
just install-local
```

To run a session you also need `xauth` and `mcookie` at runtime —
`xorg-xauth` and `util-linux` on Linux, `xorg-xauth` and `util-linux` from
ports on FreeBSD.

Packagers: see [Packaging](#packaging).

## Device access

yserver has no seat manager. The server process opens the DRM and input
devices itself, so it needs read/write access to both. How it gets that
access depends on how it is started — a display-manager-launched server
runs as root and already has it; a user-launched one does not.

### Linux

Add your user to the `video` and `input` groups, then log out and back in
(group membership is established at login):

```sh
sudo usermod -aG video,input $USER
```

Alternatively grant the devices via seat ACLs. Confirm access with:

```sh
ls -l /dev/dri/ /dev/input/event0
```

### FreeBSD

Add your user to the groups owning `/dev/dri/*` and the input devices —
conventionally `video` and `operator`; check with `ls -l` as above. The
`systemctl` commands elsewhere in this guide do not apply.

Console takeover is implemented on Linux only, so on FreeBSD `Ctrl-C` in
an X client is not shielded from the kernel's console signal handling.

## The X11 socket directory

X servers share `/tmp/.X11-unix`, which must be root-owned and sticky
(`1777`) so any user can bind a socket in it. yserver creates the
directory if it is missing but does not set its mode, so on a machine
where no other X server has ever run, a user-launched server leaves it
owned by that user — and other users then cannot start a display.

On systemd systems the installed `$prefix/lib/tmpfiles.d/yserver.conf`
handles this. Apply it without rebooting:

```sh
sudo systemd-tmpfiles --create
ls -ld /tmp/.X11-unix    # expect: drwxrwxrwt ... root root
```

On non-systemd Linux, arrange the equivalent in local startup:

```sh
mkdir -p /tmp/.X11-unix && chown root:root /tmp/.X11-unix && chmod 1777 /tmp/.X11-unix
```

On FreeBSD, the same but with the conventional group:

```sh
mkdir -p /tmp/.X11-unix && chown root:wheel /tmp/.X11-unix && chmod 1777 /tmp/.X11-unix
```

## Use with a display manager (lightdm)

lightdm can launch yserver as its X server for a graphical login. It is
the practical choice because its X server command is configurable — gdm
and sddm do not expose that.

Copy the installed example into place and restart lightdm **from a free
console**, not from inside the session you are about to replace:

```sh
sudo install -m644 /usr/local/share/doc/yserver/examples/lightdm-99-yserver.conf \
    /etc/lightdm/lightdm.conf.d/99-yserver.conf
sudo systemctl restart lightdm
```

The greeter appears, you log in, and lightdm's PAM stack unlocks the login
keyring as usual.

The example already names the path yserver was installed to; if you move
the binary, update `xserver-command`. Do **not** point it at `starty` —
that is the console launcher, and it forks rather than execs, so the
readiness handshake would never reach lightdm.

## Use directly on a console (starty)

`starty` is the installed counterpart of `startx`. Switch to a free
console and run:

```sh
starty                 # runs ~/.xinitrc (or /etc/X11/xinit/xinitrc)
starty bspwm           # ...or a WM resolved via PATH, no xinitrc needed
starty bspwm -c ~/rc   # ...with arguments
```

It picks the lowest free display, mints a per-session
MIT-MAGIC-COOKIE-1 (a server copy plus an entry in `~/.Xauthority`),
waits for the socket, runs the session, and tears the server down on
exit. It refuses to run from a pseudo-terminal, so it cannot be started
over SSH or from a terminal inside another graphical session.

Logs:

| File | Contents |
| --- | --- |
| `~/.local/state/yserver/yserver.log` | server output |
| `~/.local/state/yserver/session.log` | session output |

Raise server verbosity with `RUST_LOG=debug starty`.

## Key bindings

| Keys | Effect |
| --- | --- |
| `Ctrl-Alt-Backspace` | terminate the server, return to the console |
| `Ctrl-Alt-F1` … `Ctrl-Alt-F11` | switch to virtual console 1–11 |
| `Ctrl-Alt-Enter` | write the current scanout to a PPM in the working directory |
| `Ctrl-Alt-F12` | write every drawable's storage to PPMs in the working directory |

## Troubleshooting

**Permission denied on input or DRM devices.** Device access is not set
up, or you have not logged out and back in since adding the groups. See
[Device access](#device-access).

**"must be run from a TTY".** `starty` was run from a pseudo-terminal.
Switch to a real console.

**No Vulkan device.** Check `vulkaninfo --summary`. yserver refuses a
software implementation unless `YSERVER_ALLOW_SOFTWARE_VULKAN=1`, which is
only useful for testing.

**Another user cannot start a display.** Check `ls -ld /tmp/.X11-unix` —
see [The X11 socket directory](#the-x11-socket-directory).

To capture a log for a bug report, note that `starty` redirects the
server's output to its own log file, so piping `starty` itself gets you
only the launcher's messages. Read the server log instead:

```sh
RUST_LOG=debug starty
# after it exits:
cat ~/.local/state/yserver/yserver.log
```

Or run the server directly on a free display:

```sh
RUST_LOG=debug yserver :8 2>&1 | tee /tmp/yserver.log
```

See also `yserver(1)` and `starty(1)`.

## Packaging

`just install` is the packaging entry point. It copies pre-built
artefacts and compiles nothing. Configuration is by environment variable
or `just` assignment — **not** by positional recipe arguments:

```sh
cargo build --locked --release --bin yserver     # %build
PREFIX=/usr just man                             # %build
DESTDIR="$pkgdir" PREFIX=/usr just install       # %install
```

| Variable | Default | Purpose |
| --- | --- | --- |
| `PREFIX` | `/usr/local` | install prefix |
| `DESTDIR` | empty | staging root |
| `TARGETDIR` | `${CARGO_TARGET_DIR:-target}/release` | where the built binaries are |
| `TMPFILESDIR` | `$PREFIX/lib/tmpfiles.d` | set empty to skip the tmpfiles snippet |

Override `TARGETDIR` when building with an explicit `--target`:

```sh
DESTDIR="$pkgdir" PREFIX=/usr \
    TARGETDIR="target/x86_64-unknown-linux-gnu/release" just install
```

Stamp the release commit so `--version` is accurate in a tarball build
with no `.git`:

```sh
YSERVER_GIT_COMMIT=<commit> cargo build --locked --release --bin yserver
```

Man pages are installed uncompressed and binaries unstripped —
compression, stripping and debuginfo extraction belong to distro tooling.
Everything under `share/doc/` may be relocated or dropped to match distro
policy; only `bin/` and `share/man/man1/` are stable.

`ynest` is intentionally not installed. It is unmaintained.

Dependencies no automatic scanner can find, because none of them appear
in the ELF headers:

| Need | Why it is invisible | Package |
| --- | --- | --- |
| Vulkan loader | `libvulkan.so.1` is dlopened at runtime | `vulkan-icd-loader`, `vulkan-loader` |
| A Vulkan ICD | runtime driver, not linked | `mesa-vulkan-drivers` — *recommends*, since the NVIDIA driver also qualifies |
| `xauth` | `starty` execs it | `xorg-xauth`, `xauth` |
| `mcookie` | `starty` execs it | `util-linux` |
| XKB data | keymap rules read at runtime | `xkeyboard-config` |
| X core fonts | *recommends* only — a fontless system falls back to built-ins | `xorg-fonts-misc`, `xfonts-base` |

Distro packaging recipes live in
<https://github.com/joske/yserver-packaging>.
````

- [ ] **Step 3: Replace the README sections with a pointer**

Delete README lines 135-213 — `## Running the standalone DRM/KMS server`
through the end of the keybind list, stopping just before
`## Regression coverage with xts5 and rendercheck`. In their place:

````markdown
## Installation and setup

Full instructions — dependencies per distro, device access, display
manager and console startup, key bindings, troubleshooting and packaging
— are in **[docs/setup.md](docs/setup.md)**, also installed to
`$prefix/share/doc/yserver/setup.md`.

The short version, from a source checkout:

```sh
just install-local                   # builds and installs to /usr/local
sudo usermod -aG video,input $USER   # then log out and back in
```

Then switch to a free console and run `starty`. yserver drives atomic KMS
directly with no seat manager, so it needs access to `/dev/dri/*` and
`/dev/input/event*` and a working Vulkan driver.

Packages: `yserver` and `yserver-git` on the AUR; Fedora/EL via
[yserver-packaging](https://github.com/joske/yserver-packaging).

See also `yserver(1)` and `starty(1)`.
````

- [ ] **Step 4: Verify nothing was lost and no anchor dangles**

```bash
for t in usermod lightdm starty Ctrl-Alt-Backspace scdoc mcookie xkeyboard-config vulkan-icd-loader; do
    printf '%-22s setup.md=%s\n' "$t" "$(grep -c "$t" docs/setup.md)"
done
```
Expected: every topic non-zero in `docs/setup.md`.

```bash
grep -o '](#[a-z0-9-]*)' docs/setup.md | sed 's/](#//;s/)//' | sort -u | while read -r a; do
    grep -qi "^#\+ .*$(echo "$a" | tr '-' ' ')" docs/setup.md || echo "DANGLING: #$a"
done
```
Expected: no `DANGLING:` lines (approximate check — eyeball any hit).

```bash
sed -n '130,160p' README.md
```
Expected: the new `## Installation and setup` section followed cleanly by
`## Regression coverage with xts5 and rendercheck`, with no orphaned code
fences or leftover `####` dependency blocks.

- [ ] **Step 5: Commit**

```bash
git add docs/setup.md README.md
git commit -m "docs: add docs/setup.md as the canonical setup guide

Moves the dependency lists, device access, lightdm and starty
instructions out of README.md into docs/setup.md, which is what gets
installed to share/doc/yserver. The README keeps a short pointer, so
there is exactly one copy.

Adds what the README never covered: the /tmp/.X11-unix mode requirement,
troubleshooting, per-platform device access (the old text applied Linux
group and systemctl commands to a list that included FreeBSD), the
runtime dependencies no ELF scanner can infer, and the packaging
variable interface."
```

---

### Task 7: The recipes, test-first

Spec section 1. The smoke script is written **before** the recipes so the
red/green is real. Re-read the "Configuration interface" section at the
top of this plan before starting.

**Files:**
- Create: `tools/install-smoke.sh`
- Modify: `Justfile` — add variables at the top, replace `install`, add `man`, `install-local`, `install-smoke`

- [ ] **Step 1: Write the failing test**

Create `tools/install-smoke.sh`:

```sh
#!/bin/sh
# Exercises the `just install` contract: staging layout, modes, byte
# fidelity, idempotence, the artefact-path override, and refusal to write
# a partial tree when an input is missing.
#
# Needs a release build and rendered man pages; run via
# `just install-smoke`, which arranges both.
set -eu

fail() { echo "install-smoke: FAIL: $*" >&2; exit 1; }
ok()   { echo "install-smoke: ok: $*"; }

stage=$(mktemp -d)
stage2=$(mktemp -d)
empty=$(mktemp -d)
alt=$(mktemp -d)
trap 'rm -rf "$stage" "$stage2" "$empty" "$alt"' EXIT INT TERM

targetdir="${CARGO_TARGET_DIR:-target}/release"

mode_of() { ls -l "$1" | cut -c1-10; }

# --- 1. Stage a full install under umask 077. -----------------------------
# 077 is what catches a plain `cp` where `install -m` was needed: a copy
# would inherit the umask and ship mode 600 files.
( umask 077; DESTDIR="$stage" PREFIX=/usr just install >/dev/null )

for f in usr/bin/yserver usr/bin/starty; do
    [ -f "$stage/$f" ] || fail "missing $f"
    m=$(mode_of "$stage/$f")
    [ "$m" = "-rwxr-xr-x" ] || fail "$f has mode $m, want -rwxr-xr-x"
done
ok "binaries staged 755 under umask 077"

for f in usr/share/man/man1/yserver.1 usr/share/man/man1/starty.1 \
         usr/share/doc/yserver/setup.md usr/share/doc/yserver/LICENSE \
         usr/share/doc/yserver/examples/lightdm-99-yserver.conf \
         usr/lib/tmpfiles.d/yserver.conf; do
    [ -f "$stage/$f" ] || fail "missing $f"
    m=$(mode_of "$stage/$f")
    [ "$m" = "-rw-r--r--" ] || fail "$f has mode $m, want -rw-r--r--"
done
ok "data files staged 644 under umask 077"

# --- 2. ynest must NOT be installed. Task 9 removes the binary -----------
# outright; this assertion is the belt to that braces, and keeps holding if
# anyone reintroduces a nested-server target later.
[ ! -e "$stage/usr/bin/ynest" ] || fail "ynest was installed; it must not be"
ok "ynest not installed"

# --- 3. Byte fidelity for every copied file. Existence plus mode does ----
# not prove a complete copy — a truncating write passes the weaker check.
cmp -s "$stage/usr/bin/yserver" "$targetdir/yserver" || fail "staged yserver differs"
cmp -s "$stage/usr/bin/starty" starty                || fail "staged starty differs"
cmp -s "$stage/usr/share/man/man1/yserver.1" target/man/yserver.1 \
    || fail "staged yserver.1 differs"
cmp -s "$stage/usr/share/man/man1/starty.1" target/man/starty.1 \
    || fail "staged starty.1 differs"
cmp -s "$stage/usr/share/doc/yserver/setup.md" docs/setup.md \
    || fail "staged setup.md differs"
cmp -s "$stage/usr/share/doc/yserver/LICENSE" LICENSE || fail "staged LICENSE differs"
cmp -s "$stage/usr/lib/tmpfiles.d/yserver.conf" examples/yserver.tmpfiles \
    || fail "staged tmpfiles snippet differs"
ok "every staged file is byte-identical to its source"

# --- 4. @PREFIX@ substitution; no leaked token, no leaked DESTDIR. -------
conf="$stage/usr/share/doc/yserver/examples/lightdm-99-yserver.conf"
grep -q '^xserver-command=/usr/bin/yserver$' "$conf" \
    || fail "lightdm example does not name /usr/bin/yserver"
! grep -q '@PREFIX@' "$conf" || fail "@PREFIX@ left unsubstituted"
# -F: the temp path contains no regex metacharacters today, but do not
# depend on mktemp's naming.
! grep -rFq -- "$stage" "$stage/usr/share" \
    || fail "DESTDIR leaked into staged file contents"
ok "prefix substituted, no DESTDIR leak"

# --- 5. TMPFILESDIR= skips the snippet (non-systemd / FreeBSD path). -----
DESTDIR="$stage2" PREFIX=/usr TMPFILESDIR= just install >/dev/null
[ ! -e "$stage2/usr/lib/tmpfiles.d/yserver.conf" ] \
    || fail "TMPFILESDIR= did not skip the tmpfiles snippet"
[ -f "$stage2/usr/bin/yserver" ] || fail "TMPFILESDIR= broke the rest of the install"
ok "TMPFILESDIR= skips the snippet"

# --- 6. Idempotence. ------------------------------------------------------
DESTDIR="$stage" PREFIX=/usr just install >/dev/null
ok "second install into the same stage succeeds"

# --- 7. Artefact-path override. The AUR PKGBUILD builds with an ---------
# explicit --target, so its binaries are in
# target/$CARCH-unknown-linux-gnu/release. A hardcoded target/release
# would break it.
mkdir -p "$alt/release"
cp "$targetdir/yserver" "$alt/release/yserver"
rm -rf "$stage2"; stage2=$(mktemp -d)
DESTDIR="$stage2" PREFIX=/usr TARGETDIR="$alt/release" just install >/dev/null
cmp -s "$stage2/usr/bin/yserver" "$alt/release/yserver" \
    || fail "TARGETDIR override did not install from the given directory"
ok "TARGETDIR override honoured"

# --- 8. Missing input must fail and write NOTHING. ----------------------
# Only holds if inputs are preflighted before the first write; a recipe
# that checks as it goes leaves a half-populated tree.
if DESTDIR="$empty" PREFIX=/usr TARGETDIR=/nonexistent just install >/dev/null 2>&1; then
    fail "install succeeded with a bogus TARGETDIR"
fi
[ -z "$(ls -A "$empty")" ] || fail "failed install left files behind in the stage"
ok "missing binary: fails, writes nothing"

echo "install-smoke: PASS"
```

```bash
chmod +x tools/install-smoke.sh
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo build --locked --release --bin yserver
sh tools/install-smoke.sh
```

Expected FAILURE. The current `install` recipe takes no variables, ignores
`DESTDIR`/`PREFIX`, and runs `sudo install` into `/usr/local` — so the
first assertion fails on a missing `$stage/usr/bin/yserver` (or the recipe
errors outright). It must not reach `install-smoke: PASS`. **If it passes,
stop** — the test is not testing anything.

- [ ] **Step 3: Add the variables at the top of the `Justfile`**

Insert immediately after the existing `KERNEL := ...` line:

```just
# --- Install contract configuration --------------------------------------
# These are top-level variables, NOT recipe parameters: recipe parameters
# in just are positional, so `just install PREFIX=/usr` would bind the
# literal string "PREFIX=/usr" to the first parameter. As variables, all
# of these work, and the command line beats the environment:
#
#   DESTDIR=$pkgdir PREFIX=/usr just install     (make-like; preferred)
#   just PREFIX=/usr DESTDIR=$pkgdir install
#   just --set PREFIX /usr install
PREFIX := env_var_or_default("PREFIX", "/usr/local")
DESTDIR := env_var_or_default("DESTDIR", "")
TARGETDIR := env_var_or_default("TARGETDIR", env_var_or_default("CARGO_TARGET_DIR", "target") / "release")
# Where the /tmp/.X11-unix tmpfiles.d snippet goes. Set empty to skip it
# — correct for non-systemd Linux, FreeBSD, and prefixes systemd does not
# scan. Not decided by sniffing `uname` on the build host, which would be
# wrong when cross-staging a Linux package elsewhere.
TMPFILESDIR := env_var_or_default("TMPFILESDIR", PREFIX / "lib/tmpfiles.d")
```

- [ ] **Step 4: Replace the `install` recipe and add the others**

Replace the entire existing recipe (the comment plus `install:` and its
three lines) with:

```just
# Render the scdoc man page sources to roff in target/man/. Separate from
# `install` on purpose: running scdoc is a build transformation, so a
# packager runs this in %build alongside `cargo build`, and `install`
# only copies. DOCDIR is baked in so the FILES section names real paths.
man:
    #!/usr/bin/env sh
    set -eu
    command -v scdoc >/dev/null 2>&1 || {
        echo "just man: scdoc not found on PATH" >&2
        echo "  Arch:   pacman -S scdoc" >&2
        echo "  Debian: apt install scdoc" >&2
        echo "  Alpine: apk add scdoc" >&2
        exit 1; }
    docdir='{{ PREFIX }}/share/doc/yserver'
    case "$docdir" in
        *'|'*|*'&'*|*'\'*) echo "just man: PREFIX may not contain | & or \\" >&2; exit 1;;
    esac
    mkdir -p target/man
    for page in yserver starty; do
        sed "s|@DOCDIR@|$docdir|g" "docs/man/$page.1.scd" | scdoc > "target/man/$page.1"
        echo "just man: target/man/$page.1"
    done

# Stage an install into $DESTDIR$PREFIX. Copies only — compiles nothing,
# so a packager can call it from %install after their own build step:
#
#   cargo build --locked --release --bin yserver
#   PREFIX=/usr just man
#   DESTDIR=$pkgdir PREFIX=/usr just install
#
# Every input is checked before the first write, so a failure leaves no
# partially populated stage. Uses `install -d` + `install -m` rather than
# GNU-only `install -D`, so this works on FreeBSD.
install:
    #!/usr/bin/env sh
    set -eu
    targetdir='{{ TARGETDIR }}'
    dest='{{ DESTDIR }}{{ PREFIX }}'
    prefix='{{ PREFIX }}'
    tmpfilesdir='{{ TMPFILESDIR }}'
    case "$prefix" in
        *'|'*|*'&'*|*'\'*) echo "just install: PREFIX may not contain | & or \\" >&2; exit 1;;
    esac

    # --- Preflight: verify every input before writing anything. ---------
    # Two flags rather than pattern-matching the accumulated list:
    # "yserver" appears in both a binary path and target/man/yserver.1,
    # so a glob over the list prints the wrong hint.
    missing=''
    need_build=0
    need_man=0
    for f in "$targetdir/yserver" starty; do
        [ -f "$f" ] || { missing="$missing $f"; need_build=1; }
    done
    for f in target/man/yserver.1 target/man/starty.1; do
        [ -f "$f" ] || { missing="$missing $f"; need_man=1; }
    done
    for f in LICENSE docs/setup.md \
             examples/lightdm-99-yserver.conf.in examples/yserver.tmpfiles; do
        [ -f "$f" ] || missing="$missing $f"
    done
    if [ -n "$missing" ]; then
        echo "just install: missing input(s):" >&2
        for f in $missing; do echo "  $f" >&2; done
        [ "$need_man" -eq 0 ] || echo "run: PREFIX=$prefix just man" >&2
        [ "$need_build" -eq 0 ] || {
            echo "run: cargo build --locked --release --bin yserver" >&2
            echo "(binaries looked for in $targetdir; override with TARGETDIR=)" >&2; }
        exit 1
    fi

    # --- Binaries. ynest is deliberately not installed: unmaintained. ---
    install -d "$dest/bin"
    install -m755 "$targetdir/yserver" "$dest/bin/yserver"
    install -m755 starty "$dest/bin/starty"

    # --- Man pages, uncompressed. Distro tooling owns compression. ------
    install -d "$dest/share/man/man1"
    install -m644 target/man/yserver.1 "$dest/share/man/man1/yserver.1"
    install -m644 target/man/starty.1  "$dest/share/man/man1/starty.1"

    # --- Documentation. Downstream may relocate or drop any of this to --
    # match distro policy; only bin/ and share/man/man1/ are stable.
    install -d "$dest/share/doc/yserver/examples"
    install -m644 docs/setup.md "$dest/share/doc/yserver/setup.md"
    install -m644 LICENSE       "$dest/share/doc/yserver/LICENSE"
    sed "s|@PREFIX@|$prefix|g" examples/lightdm-99-yserver.conf.in \
        > "$dest/share/doc/yserver/examples/lightdm-99-yserver.conf"
    chmod 644 "$dest/share/doc/yserver/examples/lightdm-99-yserver.conf"

    # --- tmpfiles.d, unless TMPFILESDIR is empty. -----------------------
    if [ -n "$tmpfilesdir" ]; then
        install -d "{{ DESTDIR }}$tmpfilesdir"
        install -m644 examples/yserver.tmpfiles "{{ DESTDIR }}$tmpfilesdir/yserver.conf"
    fi

    echo "just install: staged into $dest"

# Build a release yserver, render the man pages, and install both plus
# starty to /usr/local (needs sudo). Developer convenience wrapper.
install-local:
    cargo build --locked --release --bin yserver
    PREFIX=/usr/local just man
    sudo PREFIX=/usr/local just install
    @echo "installed to /usr/local — see 'man yserver' and 'man starty'"

# Verify the install contract. Builds and renders first so the smoke
# script has its inputs.
install-smoke:
    cargo build --locked --release --bin yserver
    PREFIX=/usr just man
    sh tools/install-smoke.sh
```

- [ ] **Step 5: Run the test and watch it pass**

```bash
just install-smoke
```
Expected: `install-smoke: ok:` lines ending in `install-smoke: PASS`.

If assertion 8 fails with "left files behind", the preflight is not
running before the first write — check the `missing` loop precedes every
`install` call.

- [ ] **Step 6: Verify the missing-input error paths by hand**

```bash
rm -rf target/man
stage=$(mktemp -d)
DESTDIR="$stage" PREFIX=/usr just install; echo "exit=$?"
ls -A "$stage"; rm -rf "$stage"
```
Expected: an error naming `PREFIX=/usr just man`, `exit=1`, and **empty**
`ls` output.

And the missing-`scdoc` path — a packager without it must get the package
name, not a bare "command not found":
```bash
d=$(mktemp -d)
for c in just sed sh mkdir echo cargo; do ln -sf "$(command -v $c)" "$d/$c"; done
env -i PATH="$d" HOME="$HOME" "$d/just" man; echo "exit=$?"
rm -rf "$d"
```
Expected: `just man: scdoc not found on PATH`, the three hints, `exit=1`.

- [ ] **Step 7: Verify the real install works end to end**

```bash
just install-local
command -v yserver starty
man -w yserver starty
yserver --version
grep xserver-command /usr/local/share/doc/yserver/examples/lightdm-99-yserver.conf
```
Expected: both binaries in `/usr/local/bin`; `man -w` printing
`/usr/local/share/man/man1/{yserver,starty}.1`; a version line; and
`xserver-command=/usr/local/bin/yserver`.

- [ ] **Step 8: Commit**

```bash
git add Justfile tools/install-smoke.sh
git commit -m "feat(build): DESTDIR/PREFIX-aware install contract

Replaces the sudo-baked, /usr/local-hardcoded install recipe with one a
packager can call: it copies pre-built artefacts into \$DESTDIR\$PREFIX
and compiles nothing, so it fits between %build and %install.

Configuration is top-level just variables, not recipe parameters —
parameters are positional, so \`just install PREFIX=/usr\` would bind the
literal string \"PREFIX=/usr\" instead of setting the prefix. With
env_var_or_default, the make-like \`DESTDIR=... PREFIX=... just install\`
works, as does \`just PREFIX=... install\`.

TARGETDIR locates the binaries and defaults to
\${CARGO_TARGET_DIR:-target}/release. Required, not cosmetic: the
existing AUR PKGBUILD builds with an explicit --target, so a hardcoded
path would break it. TMPFILESDIR can be emptied to skip the tmpfiles
snippet, rather than sniffing uname on the build host.

All inputs are preflighted before the first write, so a missing input
leaves no partial stage. install -d + install -m, not GNU-only
install -D, so FreeBSD works."
```

---

### Task 8: CI wiring

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add the new build dependencies**

Append `scdoc man-db shellcheck dash` to the `apt-get install` line in the
`Install packages needed for build` step. `man-db` provides `man` for the
roff check; `dash` is the strict-shell check for `starty`.

- [ ] **Step 2: Add the contract steps**

After `Run tests` and before `Run software-Vulkan render tests (lavapipe)`:

```yaml
      - name: Lint the shell scripts
        # starty is installed on every supported platform, so it must work
        # beyond bash.
        run: |
          shellcheck -s sh starty tools/install-smoke.sh
          dash ./starty --help > /dev/null

      - name: Render man pages
        run: PREFIX=/usr just man

      - name: Check man pages render without roff warnings
        # `man --warnings` prints to stderr but still exits 0, so assert
        # stderr is empty rather than trusting the exit status.
        run: |
          for p in yserver starty; do
            err=$(man --warnings -l "target/man/$p.1" 2>&1 >/dev/null)
            if [ -n "$err" ]; then echo "roff warnings in $p.1:"; echo "$err"; exit 1; fi
          done

      - name: Verify the install contract
        run: just install-smoke

      - name: Verify the version-stamp override
        # Two builds in the SAME target dir with different values — what
        # catches a missing rerun-if-env-changed. A clean one-shot build
        # passes either way.
        run: |
          set -eu
          YSERVER_GIT_COMMIT=deadbeefcafe cargo build --locked --bin yserver
          ./target/debug/yserver --version | grep -q deadbeefcafe \
            || { echo "override ignored:"; ./target/debug/yserver --version; exit 1; }
          YSERVER_GIT_COMMIT=feedfacefeed cargo build --locked --bin yserver
          ./target/debug/yserver --version | grep -q feedfacefeed \
            || { echo "stale stamp — rerun-if-env-changed missing:"; \
                 ./target/debug/yserver --version; exit 1; }
```

- [ ] **Step 3: Verify the workflow parses**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('YAML OK')"
```
Expected: `YAML OK`

- [ ] **Step 4: Run the same commands locally**

```bash
shellcheck -s sh starty tools/install-smoke.sh && echo SHELLCHECK-OK
dash ./starty --help >/dev/null && echo DASH-OK
PREFIX=/usr just man
for p in yserver starty; do
    err=$(man --warnings -l "target/man/$p.1" 2>&1 >/dev/null)
    [ -z "$err" ] || { echo "roff warnings in $p.1:"; echo "$err"; }
done
just install-smoke
```
Expected: `SHELLCHECK-OK`, `DASH-OK`, no roff warnings, `install-smoke: PASS`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: cover the install contract, man pages and version stamp

Adds shellcheck + dash on starty and the smoke script, a scdoc render
whose roff warnings are asserted via stderr (man --warnings exits 0 even
when it warns), just install-smoke, and a two-build check of the
YSERVER_GIT_COMMIT override in one target directory — the only form that
catches a missing rerun-if-env-changed."
```

---

### Task 9: Manifest — repository URL, and remove `ynest` entirely

Two manifest changes. Spec section 6 for the URL; the spec's `ynest`
decision is escalated from "not installed" to "not buildable" — a binary
that cannot be produced cannot be shipped by accident, which is stronger
than relying on every packager to not copy it.

The two recipes that build it (`rendercheck-ynest`, `xts-ynest`) go too;
they are stale. `tools/rendercheck.sh` and `tools/xts-xlib-sweep.sh` only
*mention* ynest in a comment and a message string — neither builds nor
invokes it, so they need no change.

The `host_x11`/`nested` modules in `yserver-core` stay. They are library
code, still compiled and still unit-tested; removing the nested backend is
a much larger change and is not in scope.

**Files:**
- Modify: `Cargo.toml` — `[workspace.package]` `repository`
- Modify: `crates/yserver/Cargo.toml` — drop the `ynest` `[[bin]]`, add `autobins = false`
- Delete: `crates/yserver/src/bin/ynest.rs`
- Modify: `Justfile` — delete the `rendercheck-ynest` and `xts-ynest` recipes

- [ ] **Step 1: Set the repository URL**

```bash
grep -n 'repository' Cargo.toml
```
Expected: `repository = ""`. Replace with:

```toml
repository = "https://github.com/joske/yserver"
```

- [ ] **Step 2: Prove that removing the `[[bin]]` block alone is not enough**

This is the trap: Cargo auto-discovers `src/bin/*.rs`. Delete only the
`[[bin]]` block and ynest still builds, under the same name. Demonstrate
it before doing the real change — remove the block from
`crates/yserver/Cargo.toml`:

```toml
[[bin]]
name = "ynest"
path = "src/bin/ynest.rs"
```

then:
```bash
cargo build --locked --bin ynest 2>&1 | tail -3
```
Expected: it **still builds** (or at minimum, `cargo metadata` still lists
a `ynest` target):
```bash
cargo metadata --format-version 1 --no-deps | grep -o '"name":"ynest"' | head -1
```
Expected: a match — proving the block's removal achieved nothing on its
own.

- [ ] **Step 3: Disable auto-discovery and delete the source**

In `crates/yserver/Cargo.toml`, add to the `[package]` section:

```toml
# No target auto-discovery: `yserver` is declared explicitly below, and a
# stray src/bin/*.rs must never become a shipped binary by accident (this
# is how ynest would come back).
autobins = false
```

Keep the explicit `yserver` `[[bin]]`. Then delete the source:

```bash
git rm crates/yserver/src/bin/ynest.rs
```

- [ ] **Step 4: Delete the two stale recipes**

From the `Justfile`, delete the `rendercheck-ynest` recipe (its comment
block plus the recipe, in the RENDERCHECK section) and the `xts-ynest`
recipe (its comment block plus the recipe, in the XTS section). Keep
`rendercheck-yserver`, `rendercheck-yserver-hw`, `xts-yserver`.

One comment on `xts-yserver` reads "then runs the same xts harness ynest
uses" — reword, since that harness no longer has a ynest caller:

```
# host because vng mounts the host rootfs --rw. Runs tools/xts-run.sh.
```

- [ ] **Step 5: Verify ynest is gone and nothing else broke**

```bash
cargo metadata --format-version 1 --no-deps | grep -o '"name":"ynest"' && echo "STILL PRESENT" || echo "ynest target gone (good)"
cargo build --locked --bin ynest 2>&1 | tail -2
```
Expected: `ynest target gone (good)`, and the build invocation failing
with "no bin target named `ynest`".

```bash
cargo metadata --format-version 1 --no-deps | grep -o '"repository":"[^"]*"' | sort -u
cargo build --locked
cargo +nightly fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
just --list > /dev/null && echo "Justfile parses"
grep -rn "ynest" Justfile || echo "no ynest left in Justfile"
```
Expected: the repository URL per crate; a clean build; fmt/clippy/tests
clean; `Justfile parses`; `no ynest left in Justfile`.

Clippy matters here: `host_x11`/`nested` lose their only binary consumer.
They are `pub` library modules so no `dead_code` warnings should appear —
if any do, that is real information about code only ynest reached. Report
it rather than silencing it with `#[allow]`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/yserver/Cargo.toml Justfile
git rm --cached crates/yserver/src/bin/ynest.rs 2>/dev/null || true
git commit -m "chore: remove the ynest binary and set the repository URL

ynest is unmaintained and its rendercheck/xts recipes are stale, so it is
removed rather than merely left uninstalled — a binary that cannot be
built cannot be shipped by accident.

Note that dropping the [[bin]] block is not sufficient on its own: cargo
auto-discovers src/bin/*.rs and would rebuild ynest under the same name.
autobins = false plus deleting the source is what actually removes it,
and it also stops a future stray src/bin/*.rs becoming a shipped binary.

The host_x11/nested library modules stay — they are still compiled and
unit-tested; removing the nested backend is a separate, larger change.

Cargo.toml's repository field was empty; spec files and PKGBUILDs
consume it."
```

---

### Task 10: Final verification

- [ ] **Step 1: Full gate, exactly as CI runs it**

```bash
cargo +nightly fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --locked
```
Expected: all clean.

- [ ] **Step 2: Contract from a clean slate**

```bash
rm -rf target/man
just install-smoke
```
Expected: `install-smoke: PASS`. Removing `target/man` first proves
`install-smoke` regenerates its own inputs.

- [ ] **Step 3: The `CARGO_TARGET_DIR` default path**

`TARGETDIR` is covered explicitly by the smoke script; this covers the
other way a packager relocates artefacts.

```bash
export CARGO_TARGET_DIR=/tmp/yserver-alt-target
cargo build --locked --release --bin yserver
PREFIX=/usr just man
stage=$(mktemp -d)
DESTDIR="$stage" PREFIX=/usr just install
ls "$stage/usr/bin/"
unset CARGO_TARGET_DIR; rm -rf /tmp/yserver-alt-target "$stage"
```
Expected: `starty` and `yserver` listed. Note `just man` writes to
`target/man/` regardless — that path is not affected by
`CARGO_TARGET_DIR`, and the install recipe reads it from there.

- [ ] **Step 4: Git-free, offline, vendored tarball build**

The case every package hits. Fedora requires all build inputs declared, so
a network `cargo fetch` during `%build` is a policy violation.

```bash
commit=$(git rev-parse --short=12 HEAD)
tarball=$(mktemp -d)
git archive --format=tar HEAD | tar -x -C "$tarball"
( cd "$tarball"
  [ -d .git ] && echo "UNEXPECTED: .git present" || echo "no .git (correct)"
  cargo vendor --locked vendor > vendor-config.toml
  mkdir -p .cargo && mv vendor-config.toml .cargo/config.toml
  YSERVER_GIT_COMMIT="$commit" cargo build --locked --offline --release --bin yserver
  ./target/release/yserver --version )
rm -rf "$tarball"
```
Expected: `no .git (correct)`, a successful `--offline` build, and a
version line naming the real commit — **not** `unknown`.

If `cargo vendor` needs the network on a cold cache, run it once with
network available; the point is that `cargo build --locked --offline` then
succeeds.

- [ ] **Step 5: Manual — `starty` on hardware**

Needs a real console and GPU. From a free VT:

```bash
just install-local
starty xterm
```
Expected: xterm on a fresh display; `Ctrl-Alt-Backspace` returns to the
console. Then confirm teardown was clean — this is the task 1 cleanup fix
in situ:

```bash
xauth list | grep -c ":$(echo $DISPLAY | tr -d ':')" || echo "no entry (good)"
ls /tmp/yserver-startx-auth.* 2>/dev/null || echo "no temp file (good)"
```
Expected: `no entry (good)` and `no temp file (good)`.

Also verify cleanup after an *abnormal* exit, which is what the `|| :`
guards are for — start it, then kill the server from another VT so
`cleanup` runs with an already-dead pid:

```bash
starty xterm &
sleep 5; pkill -TERM yserver; sleep 2
ls /tmp/yserver-startx-auth.* 2>/dev/null || echo "no temp file after abnormal exit (good)"
```
Expected: `no temp file after abnormal exit (good)`.

- [ ] **Step 6: Manual — the lightdm path**

The least-tested thing being shipped: all existing hardware testing goes
through `starty`, never the lightdm example. Do this in a VM, or with a
free console available to recover.

```bash
sudo install -m644 /usr/local/share/doc/yserver/examples/lightdm-99-yserver.conf \
    /etc/lightdm/lightdm.conf.d/99-yserver.conf
sudo systemctl restart lightdm     # from a FREE console, not the session
```
Expected: the greeter appears and a login succeeds. Then confirm the
readiness handshake actually fired rather than lightdm timing out and
retrying:

```bash
journalctl -u lightdm -b | grep -i 'signal\|ready\|timeout\|failed'
ps -o args= -C yserver
```
Expected: no readiness timeout, and argv like
`:0 -seat seat0 -auth /var/run/lightdm/root/:0 -nolisten tcp vt7 -novtswitch`.

- [ ] **Step 7: Report status honestly**

State which steps passed, which manual steps ran and on what hardware,
and which were skipped. Do **not** claim the lightdm path works if step 6
did not run — per the spec it is the least-covered path, and an
unverified claim there is worse than a stated gap.

---

## Out of scope

Do not start these:

- **The `yserver-packaging` repository** (Fedora spec, Debian, Alpine, Arch PKGBUILD). Separate deliverable, separate repo; needs explicit approval to create.
- **The AUR upload.** Needs explicit approval.
- **Contacting the `yserver-git` maintainer or the author of PR #13.** Public communication in the maintainer's name — draft for review, never send.
- **Hardening `create_dir_all("/tmp/.X11-unix")`** into a mode-validating create. Real bug, changes server behaviour, filed separately; the `tmpfiles.d` snippet is the packaging-level mitigation.
- **Any FreeBSD port.** Handled by engaging the ports tree upstream.
- **Removing the `host_x11`/`nested` library modules.** Task 9 removes the ynest *binary*; the nested backend code stays compiled and tested. Ripping it out is a separate, larger change.
