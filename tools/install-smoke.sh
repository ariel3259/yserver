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

# `stat` is not portable (GNU -c vs BSD -f), so read the mode from `ls`.
# shellcheck disable=SC2012  # every path here is one we created under mktemp -d
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

# --- 2. ynest must NOT be installed. The binary is removed from the ------
# workspace outright; this assertion is the belt to that braces, and keeps
# holding if anyone reintroduces a nested-server target later.
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
# `TMPFILESDIR=` with an empty value is deliberate — it is exactly how a
# packager opts out, and just's env_var_or_default then yields "".
# shellcheck disable=SC1007
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
