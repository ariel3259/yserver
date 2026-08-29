# Install contract + installed documentation

Date: 2026-07-28
Status: design approved, not implemented

## Problem

`just install` is a developer convenience, not an install contract:

```
install:
    cargo build --release --bin yserver
    sudo install -m755 target/release/yserver /usr/local/bin/yserver
    sudo install -m755 starty /usr/local/bin/starty
```

It hardcodes `/usr/local`, bakes in `sudo`, builds and installs in one
step, and installs nothing but the two binaries. A distro package cannot
use it: RPM and dpkg stage into a build root (`DESTDIR`) with a
`/usr` prefix, run `%build` and `%install` as separate steps, and cannot
call `sudo`.

There is also no installed documentation. Everything a user needs to set
up yserver — group access, the lightdm drop-in, `starty` usage, the
keybinds, where logs land — lives only in `README.md`, which is
dev-facing and not installed anywhere. A packaged yserver gives the user
a binary and no instructions.

PR #13 (jboero, closed) showed the consequence: to package yserver for
Fedora he had to write his own session launcher, his own systemd unit and
his own copy of the lightdm config, because the tree exported none of
them. That work belongs downstream of an install contract, not inside
each distro's spec file.

## Goals

Make this repository installable and self-documenting, so that a distro
package's install step reduces to one line:

```
DESTDIR=%{buildroot} PREFIX=/usr just install
```

The one-line claim is about the **`%install` step only**. A whole package
definition can never be one line: dependency declarations, licence
relocation, stripping, debuginfo extraction and man page compression are
downstream concerns, addressed in section 7.

Non-goal: distro packaging itself. Spec files and PKGBUILDs live in a
separate repository (`joske/yserver-packaging`), covered in "Follow-up,
separate deliverable" below and **not** part of this spec.

## Design

### 1. Install contract

Replace the `install` recipe with a staging-only recipe configured by
**top-level `just` variables**:

```
PREFIX      := env_var_or_default("PREFIX", "/usr/local")
DESTDIR     := env_var_or_default("DESTDIR", "")
TARGETDIR   := env_var_or_default("TARGETDIR", env_var_or_default("CARGO_TARGET_DIR", "target") / "release")
TMPFILESDIR := env_var_or_default("TMPFILESDIR", PREFIX / "lib/tmpfiles.d")

install:
```

Variables, **not recipe parameters** — this is load-bearing and was
verified by running it. Recipe parameters in `just` are positional, so
with `install PREFIX="/usr/local" DESTDIR="":` the invocation
`just install DESTDIR=/tmp/stage PREFIX=/usr` binds `PREFIX` to the
literal string `DESTDIR=/tmp/stage` and `DESTDIR` to `PREFIX=/usr`. It
fails silently rather than erroring, which is the worst possible
behaviour for a packaging interface.

As variables with `env_var_or_default`, three forms work, and the command
line beats the environment:

```sh
DESTDIR=$pkgdir PREFIX=/usr just install    # make-like; the documented form
just PREFIX=/usr DESTDIR=$pkgdir install    # just assignment, before the recipe
just --set PREFIX /usr install
```

It **does not compile anything** — neither Rust nor roff. Packagers run
their own build step; a recipe that compiles during `%install` fights the
packaging model. Every input must already exist, and `install` verifies
all of them *before writing anything*, so a failure leaves no partial
staging.

`TARGETDIR` locates the built artefacts and defaults to
`${CARGO_TARGET_DIR:-target}/release`. This matters: the existing AUR
PKGBUILD builds with `--target "$CARCH-unknown-linux-gnu"`, so its
binaries land in `target/$CARCH-unknown-linux-gnu/release/`. A hardcoded
`target/release` would break the one packager that exists today. Any
packager using an explicit `--target`, a private `CARGO_TARGET_DIR`, or a
distro cargo wrapper needs this override.

Installed layout, all paths under `$DESTDIR$PREFIX`:

| Path | Source | Mode |
| --- | --- | --- |
| `bin/yserver` | `$TARGETDIR/yserver` | 755 |
| `bin/starty` | `starty` | 755 |
| `share/man/man1/yserver.1` | `target/man/yserver.1` (from `just man`) | 644 |
| `share/man/man1/starty.1` | `target/man/starty.1` (from `just man`) | 644 |
| `share/doc/yserver/setup.md` | `docs/setup.md` | 644 |
| `share/doc/yserver/LICENSE` | `LICENSE` | 644 |
| `share/doc/yserver/examples/lightdm-99-yserver.conf` | `examples/lightdm-99-yserver.conf.in`, `@PREFIX@` substituted | 644 |
| `$TMPFILESDIR/yserver.conf` (default `$PREFIX/lib/tmpfiles.d`) | `examples/yserver.tmpfiles` | 644 |

`ynest` is **removed from the workspace entirely**, not merely left
uninstalled. It is unmaintained and not verified against current work,
and its `rendercheck-ynest` / `xts-ynest` recipes are stale. A binary
that cannot be built cannot be shipped by accident, which is stronger
than relying on every packager to not copy it.

Note that dropping the `[[bin]]` block does **not** achieve this: cargo
auto-discovers `src/bin/*.rs` and rebuilds `ynest` under the same name.
Removal means `autobins = false` plus deleting `src/bin/ynest.rs` — which
also stops a future stray `src/bin/*.rs` becoming a shipped binary.

The `host_x11`/`nested` modules in `yserver-core` stay: library code,
still compiled and unit-tested. Removing the nested backend is a separate,
larger change.

Packages expose `yserver` and `starty` only.

Implementation must use portable `install -d` followed by `install -m`,
**not** GNU-only `install -D`, so the recipe works on FreeBSD.

Two convenience recipes sit on top:

```
man:                # scdoc docs/man/*.scd -> target/man/*.1
install-local:      # cargo build --release, just man, then sudo PREFIX=/usr/local just install
install-smoke:      # build + man, then tools/install-smoke.sh
```

`DESTDIR` defaults to empty, so `sudo PREFIX=/usr/local just install` as
root also behaves like the old recipe.

### 2. Man pages

Source is scdoc (`docs/man/*.scd`). Only the `.scd` files are committed —
one source of truth, no generated artefact that can drift.

Generation is its own recipe, `just man`, writing `target/man/*.1`.
Running `scdoc` is a build transformation, so it belongs in the build
step next to `cargo build`, not in `install`. Packagers call `just man`
in `%build`; `install` only copies. This also keeps `install` honest
about compiling nothing.

`scdoc` is therefore a build dependency for every packager, and is added
to the per-distro dependency lists — which section 3 moves from
`README.md` into `docs/setup.md`, so `scdoc` is added there, in the one
remaining copy.

`install` fails if `target/man/*.1` is absent, naming `just man`. It does
not silently skip the man pages: a package that ships without them is a
defect that surfaces only after release. `just man` itself fails with the
package name to install when `scdoc` is missing.

Man pages are installed **uncompressed and unstripped binaries are
installed as built**. Compression, stripping and debuginfo extraction are
distro tooling's job (`dh_compress`, `namcap`/`makepkg`, RPM's
`find-debuginfo`); doing any of it upstream fights every packager.

`yserver.1` covers:

- SYNOPSIS — `:N`, `vtN`, `-auth FILE`, `-displayfd FD`,
  `-layout LAYOUT`, `--version` as functional options. Then a separate
  "Accepted for compatibility, ignored" group: `-seat SEAT`,
  `-novtswitch`, `-nolisten`, `-config`, `-background`. Documenting
  these as functional would be a lie —
  - `-seat` is parsed but never used; input is unconditionally assigned
    to `seat0` (`input/context.rs:183`),
  - `-novtswitch` is an explicit no-op (`launch.rs:85`),
  - and the rest consume-and-discard their value.

  lightdm and xinit pass all of them, which is why they are accepted at
  all.
- DESCRIPTION — drives atomic KMS directly with no seat manager, so the
  **server process** needs read/write access to `/dev/dri/*` and
  `/dev/input/event*`, and a Vulkan driver. How that access is obtained
  differs by launch path and must be stated as such: a lightdm-launched
  server runs as root; a `starty` session runs as the user and needs the
  `video` and `input` groups (or equivalent seat ACLs).
- ENVIRONMENT — `RUST_LOG`, `YSERVER_DRM_DEVICE`, `YSERVER_MODE`,
  `YSERVER_ALLOW_SOFTWARE_VULKAN`. The remaining
  `YSERVER_*` variables are development diagnostics and get a single
  pointer line to the repository docs, not individual entries — man
  pages describe the supported interface, not the tracing surface.
- SIGNALS — three distinct things that must not be conflated:
  - **outgoing** `SIGUSR1` to the parent process at readiness (what
    lightdm waits on), sent only when the inherited disposition says the
    parent wants it;
  - **incoming** `SIGUSR1` = VT release and **incoming** `SIGUSR2` = VT
    acquire, the console handover handshake (`lib.rs:479`);
  - `SIGTERM` teardown.

  A SIGNALS section saying only "SIGUSR1 readiness" would be actively
  misleading, since incoming SIGUSR1 means something else entirely.
- The Ctrl-Alt-Backspace / Ctrl-Alt-Enter / Ctrl-Alt-F12 keybinds.
- FILES, and SEE ALSO `starty(1)`.

`starty.1` covers: lowest-free-display scan, the MIT-MAGIC-COOKIE-1
handshake (server copy in a `mktemp` file via `-auth`, matching entry
added to `~/.Xauthority` and removed on teardown), the TTY guard that
refuses pty/SSH callers, `~/.xinitrc` versus a PATH-resolved client,
`$XDG_STATE_HOME/yserver/{yserver,session}.log`, and the `xauth` +
`mcookie` requirements.

### 2a. `starty` portability

The shipped launcher's contract is currently narrower than the project's
claimed platform support, and installing it unchanged on FreeBSD ships
something that cannot work:

- the TTY guard matches `/dev/tty[0-9]*` only (`starty:48`), whereas
  FreeBSD consoles are `/dev/ttyv*`;
- `seq` (`starty:108`) and `env -u` (`starty:119`) are not POSIX, so the
  file's "POSIX sh" self-description is inaccurate.

Fix all three in place: extend the guard to accept `/dev/ttyv[0-9a-z]*`,
replace `seq` with a `while` counter, and replace `env -u` with explicit
`unset` in a subshell. These are small, and they make the installed
contract true on every platform the README claims.

Note that console takeover itself is `#[cfg(target_os = "linux")]`
(`lib.rs:302`) and compiled out elsewhere, so FreeBSD gets a working
launcher but no Ctrl-C protection. `starty.1` and `docs/setup.md` say so
rather than implying parity.

### 3. `docs/setup.md`

The canonical setup guide, and the file installed to
`share/doc/yserver/setup.md`. Content, moved out of `README.md` rather
than copied:

- Build dependencies per distro (Arch, Ubuntu, Alpine, FreeBSD) — the
  existing lists plus `scdoc`, and Alpine's
  `RUSTFLAGS="-C target-feature=-crt-static"`.
- Device access, structured **per platform rather than as one Linux
  recipe trailing a multi-distro dependency list**. The current README
  puts `sudo usermod -aG video,input $USER` and `systemctl` right after
  a list that includes FreeBSD, which reads as though they apply
  everywhere. Split it: Linux (groups or seat ACLs, `systemctl`), and
  FreeBSD (its own group names, no `systemctl`, and no console takeover
  since that code is Linux-only).
- Display-manager setup, pointing at the installed example config rather
  than inlining a snippet that can drift from the shipped file.
- `starty` on a bare TTY.
- `/tmp/.X11-unix`: what the shipped `tmpfiles.d` snippet does, and what
  non-systemd and FreeBSD packagers must arrange instead (per section 4a).
- Keybinds and log locations.
- A Packages section linking the AUR packages (`yserver` tagged,
  `yserver-git` third-party) and `joske/yserver-packaging` (Fedora/COPR).

`README.md` keeps a short Installation section pointing at
`docs/setup.md` and noting its installed path. This makes the README
shorter and more of a project pitch, and leaves exactly one copy of the
setup instructions.

### 4. Example lightdm config

`examples/lightdm-99-yserver.conf.in` is a template:

```ini
[Seat:*]
xserver-command=@PREFIX@/bin/yserver
```

`install` substitutes `@PREFIX@`, so a packaged install ships
`/usr/bin/yserver` and `install-local` ships `/usr/local/bin/yserver`.
The shipped file always names a path that exists on that system, which a
hardcoded snippet cannot do across both.

Only the lightdm drop-in is shipped. No systemd unit (starty covers the
VT path without one) and no sample `xinitrc`.

### 4a. `/tmp/.X11-unix` ownership and mode

yserver calls a bare `fs::create_dir_all("/tmp/.X11-unix")` (`lib.rs:340`)
and never forces the mode. On a machine where no other X server package
has already created that directory, a user-run `starty` creates it owned
by that user and subject to their umask, instead of the conventional
root-owned sticky `1777` directory. A second user then cannot bind a
socket there.

Today this is masked because test machines all have Xorg installed. A
standalone packaged X server cannot assume that.

Ship `examples/yserver.tmpfiles` installed to
`$PREFIX/lib/tmpfiles.d/yserver.conf`:

```
d /tmp/.X11-unix 1777 root root -
```

This is what Xorg's own packaging does, and it is the mechanism every
systemd distro already runs. It is Linux-only and skipped on other
platforms — the recipe installs it only when the host is Linux, and
`docs/setup.md` tells non-systemd and FreeBSD packagers to arrange the
equivalent themselves.

Hardening `create_dir_all` into a mode-validating create is the more
complete fix but changes server behaviour, so it is deliberately left out
of this spec and filed separately.

### 5. Version stamping outside a git checkout

`crates/yserver/build.rs::emit_git_commit` shells out to git
unconditionally and falls back to `"unknown"` when git is absent. Every
package builds from an exported tarball, so every packaged binary would
report `yserver 1.4.0 (unknown)`.

Change `emit_git_commit` to honour a pre-set `YSERVER_GIT_COMMIT`
environment variable and only shell out to git when it is unset. Spec
files and PKGBUILDs then stamp the release commit, and `--version`
stays truthful in packages. Behaviour inside a git checkout is
unchanged.

The function must also emit:

```rust
println!("cargo:rerun-if-env-changed=YSERVER_GIT_COMMIT");
```

Without it, changing the variable while reusing a target directory may
not rerun the build script, leaving a stale version stamp baked into the
binary. The existing function registers rerun triggers only for git
files (`build.rs:102`), so this is a real omission and not defensive
noise.

### 6. `Cargo.toml` metadata

`repository` is `""`. Set it to `https://github.com/joske/yserver`;
spec files and PKGBUILDs consume it, and an empty field in a published
manifest is a defect regardless.

### 7. What this contract does *not* do

The install contract deliberately stops short of things that are
downstream policy. Stating them here is the point of the section: they
are the parts a packager must still supply, and the parts most easily
got wrong.

**Runtime dependencies that no automatic scanner can infer.** None of
these appear in the ELF headers, so `rpm`'s dependency generator,
`dpkg-shlibdeps` and `namcap` all miss them:

| Need | Why invisible | Typical package |
| --- | --- | --- |
| Vulkan loader | `ash::Entry::load()` dlopens `libvulkan.so.1` (`kms/vk/device.rs:92`) | `vulkan-icd-loader` / `vulkan-loader` |
| A Vulkan ICD | runtime driver, not linked | `mesa-vulkan-drivers` — a *recommends*, since the NVIDIA blob also qualifies |
| `xauth` | `starty` execs it (`starty:70`) | `xorg-xauth` / `xauth` |
| `mcookie` | `starty` execs it (`starty:70`) | `util-linux` |
| XKB rules and keymap data | xkbcommon reads system rules at runtime | `xkeyboard-config` |

X core fonts are a soft dependency: `default_font_path()` filters to dirs
that actually carry a `fonts.dir` and always appends `built-ins`, so a
fontless system degrades rather than fails. A *recommends* on
`xorg-fonts-misc` is right; a hard requires is not.

**Licence and documentation placement.** `share/doc/yserver/LICENSE` is
correct generic upstream placement, but no single layout satisfies every
distro, and upstream should not try to implement five:

- Arch expects `/usr/share/licenses/yserver/`;
- Debian requires `/usr/share/doc/yserver/copyright` with more content
  than a bare MIT text;
- FreeBSD consumes the licence via `LICENSE_FILE` and uses `DOCSDIR` /
  `EXAMPLESDIR`;
- Alpine splits docs and man pages into a `-doc` subpackage.

So the contract's rule is explicit: **downstream may relocate or delete
anything under `share/doc/`**, and the staged layout is a starting point,
not a mandate. Only `bin/` and `share/man/man1/` are stable.

**Offline and reproducible builds.** `Cargo.lock` pins resolution but
does not make sources available offline. Fedora in particular requires
every non-base build requirement to be declared, which a network `cargo
fetch` violates. Vendoring (`cargo vendor` + `--offline`) is downstream's
call per distro; what this repo owes is that `cargo build --locked
--offline` succeeds against a vendored tree and an exported (git-free)
source tarball. That is a release test, listed below.

## Testing

The install contract is shell plumbing, so the tests are real staged
installs rather than unit tests. A `just install-smoke` recipe runs
everything scriptable so CI covers it.

**Staging correctness**

1. Stage into a fresh `mktemp -d` (never a fixed `/tmp/stage`, which
   collides between parallel CI jobs and hides stale files): assert the
   exact file list with expected modes, and that the staged lightdm
   config names `/usr/bin/yserver`.
2. `cmp` the staged `bin/yserver` and `bin/starty` against their sources.
   Existence plus mode does not prove byte-for-byte staging — a
   truncating copy passes the weaker check.
3. Run the whole thing under `umask 077` and assert modes are still
   755/644. This is the check that catches a `cp` where an `install -m`
   was needed.
4. Install twice into the same stage and assert idempotence.
5. Install with `TARGETDIR` pointed at a `--target
   x86_64-unknown-linux-gnu` build tree, and separately with
   `CARGO_TARGET_DIR` set — the two ways a real packager builds.

**Failure modes**

6. Missing `target/man/*.1` must fail naming `just man`, and missing
   binaries must fail naming the build command. Both must write
   **nothing** — assert the stage is still empty afterwards, which only
   holds if all inputs are preflighted before the first write rather than
   checked as they are reached.
7. `just man` with `scdoc` masked off `PATH` must fail naming the package.

**Generated output**

8. `man -l` both pages: no roff errors, well-formed NAME lines for
   `whatis`.

**Version stamping**

9. `YSERVER_GIT_COMMIT=deadbeefcafe cargo build --release` reports
   `deadbeefcafe`; then rebuild in the **same target directory** with a
   different value and assert it changes. A clean one-shot build passes
   even without `rerun-if-env-changed`, so this must reuse the target dir
   to be meaningful.
10. Build from a `git archive` export (no `.git`) with `--locked
    --offline` against a vendored tree, and assert `--version` shows the
    passed commit rather than `unknown`.

**Launcher portability**

11. ShellCheck `starty`, and run `starty --help` under `dash` and BusyBox
    `sh` as well as bash — the point of removing `seq`/`env -u` is lost if
    nothing checks it.

**Manual, needs hardware**

12. `install-local`, then `starty` from a real TTY: the installed
    `starty` finds the installed `yserver` on `PATH` and brings up a
    session.
13. The **lightdm path**, in a VM or on hardware: lightdm-appended
    `vtN`/`-seat`/`-auth`/`-nolisten` argv, socket creation, and the
    outgoing SIGUSR1 readiness handshake reaching lightdm. The existing
    hardware testing only ever exercises `starty`, so the shipped lightdm
    example is the least-tested thing being shipped.

Downstream, each packaging recipe additionally runs its native QA —
`rpmlint`, `lintian`, `namcap`, `abuild sanitycheck`. Those belong to the
packaging repo, not here.

Step 11 is the only FreeBSD-relevant automated check, and it runs on
Linux (`dash`/BusyBox `sh` stand in for a stricter shell). Nothing in CI
can exercise a real FreeBSD install, so the `ttyv*` guard and portable
`install -d` are verified by inspection plus a manual run on the GhostBSD
box when convenient — not gated on it.

## Follow-up, separate deliverable

`joske/yserver-packaging`, out of scope for this spec and for the
implementation plan that follows it:

```
fedora/yserver.spec     seeded from PR #13, jboero maintains
arch/PKGBUILD           the tagged `yserver` package, maintained here
debian/                 control, rules, install — covers Debian + Ubuntu
alpine/APKBUILD         musl/Alpine
README.md               how to build each; ownership per directory
```

**No FreeBSD port lives here.** GitHub Actions cannot build FreeBSD
packages, so a `Makefile`/`pkg-plist` in this repo would be unbuildable
and untested — worse than absent. FreeBSD is handled by engaging the
FreeBSD ports tree directly, upstream of us.

That does **not** relax the FreeBSD portability requirements on the
install contract itself (portable `install -d`/`install -m` in section 1,
the `ttyv*` launcher guard in section 2a, the per-platform setup split in
section 3). Those exist so a ports maintainer can consume `just install`
unmodified. Shipping no port is a decision about who does the packaging;
shipping a Linux-only install contract would be a decision to make the
port impossible.

Debian and Ubuntu share one `debian/` directory; the build-dependency
names are identical on both, so a single source package covers them.
`debian/rules` is a `dh` stub whose `override_dh_auto_install` is
`DESTDIR=debian/yserver PREFIX=/usr just install`.

Alpine is a supported target — musl builds (issue #15 closed), and
`AGENTS.md` requires ioctl request typing to stay valid on Linux musl.
Two Alpine-specific notes: `RUSTFLAGS="-C target-feature=-crt-static"`
is required (already in the README's Alpine dependency list), and Alpine
policy splits documentation and man pages into a `-doc` subpackage, so
the `APKBUILD` relocates what `just install` stages rather than
consuming the layout verbatim.

### Arch: two packages, two owners

`yserver-git` already exists on the AUR, maintained by a third party
(openglfreak / Reyka Matthies), and tracks `master`. It keeps that role.
Alongside it we publish **`yserver`** — the tagged-release package —
to `aur.archlinux.org/yserver.git`.

The two coexist with no coordination: `yserver-git` already declares
`provides=('yserver')` and `conflicts=('yserver')`, which is exactly the
right relationship to a stable package of that name.

The canonical copy of our `PKGBUILD` lives in `yserver-packaging/arch/`
and is pushed to the AUR repo, rather than being authored in the AUR
repo directly — it keeps both distro recipes reviewable in one place,
and an AUR git repo is a poor review surface. It builds from the GitHub
release tarball for the tag (with a real `sha256sum`, not `SKIP`), which
makes section 5 load-bearing: a release tarball has no `.git`, so
without the `YSERVER_GIT_COMMIT` override every stable package would
report `--version` as `unknown`.

The existing `yserver-git` PKGBUILD has three defects this work bears
on, and its maintainer is worth notifying once the install contract
lands (it already lists `just` in `makedepends` without calling it):

1. `depends` omits `vulkan-icd-loader`. `ash::Entry::load()`
   (`crates/yserver/src/kms/vk/device.rs:92`) dlopens `libvulkan.so.1`
   at runtime, so Arch's ELF dependency scanner cannot see it and a
   clean install can fail at startup. This is the same trap jboero
   guarded against explicitly for RPM.
2. `depends` still lists `seatd`. libseat was removed — the tree's only
   mentions are comments recording its removal — so the dependency is
   stale.
3. It installs `ynest` and no `starty` (it predates the launcher), and
   `cp -r docs` copies every development handoff and XTS baseline into
   `/usr/share/doc`.

Only defect 3 is fixed by adopting `just install` — that one is purely
about which files get staged. Defects 1 and 2 live in the `depends`
array, which no install contract can touch; they need a maintainer edit
either way. Section 7's dependency table exists so that edit is a lookup
rather than a rediscovery.

Seeding preserves authorship: cherry-pick jboero's files with him as
commit author, then rework in a separate commit on top. The rework adds
`scdoc` and `just` to `BuildRequires`, replaces the hand-written
`%install` with `DESTDIR=%{buildroot} PREFIX=/usr just install`, drops
his `yserver-session` launcher (superseded by `starty`), drops the
systemd unit and sysconfig, and drops `ynest`. Two findings from his
spec are kept because they are not rediscoverable from the source:
`vulkan-loader` must be an explicit `Requires` since `ash` dlopens
`libvulkan.so.1` where RPM's ELF dependency generator cannot see it, and
`mesa-vulkan-drivers` belongs in `Recommends`. His note about a
`GPL-3.0-only` / MIT mismatch is already resolved — the workspace
declares MIT.

Releases become: tag, GH release, then a `Version:`/`pkgver` bump in the
packaging repo.

Creating that repository and contacting jboero about commit rights are
public actions in the maintainer's name and require explicit approval
per interaction; text gets drafted for review first, never posted
directly.
