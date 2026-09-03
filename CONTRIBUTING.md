# Contributing to yserver

## Toolchain
- Use rustup — the repo pins its toolchain in `rust-toolchain.toml` (stable),
  and rustup honors it automatically. Distro `rustc` (apt/pacman) ignores that
  pin and can lag a release behind; if it won't build, use rustup.
- Nightly is only used for formatting (`cargo +nightly fmt`).

## Before opening a PR — run what CI runs (CI denies warnings)
- `cargo +nightly fmt` (CI checks with `cargo +nightly fmt -- --check`)
- `cargo clippy --all-targets -- -D warnings`
- `cargo test --all-targets --locked`

The build dependencies in `docs/setup.md` are all the suite needs — the
font tests generate their own `fonts.dir` fixtures, and a machine with no X
core fonts at all is a supported configuration with its own coverage, so
don't install fonts to make tests pass. If a test fails only because fonts
are missing, that is a bug worth reporting.

### If X core fonts appear not to load at runtime
yserver only puts a directory on the font path if it has a readable
`fonts.dir` (`kms/core.rs`). On Arch that index is **not shipped** by the font
packages — it is generated post-transaction by `xorg-mkfontscale`'s pacman
hook (`mkfontdir` / `mkfontscale`). So with `xorg-fonts-misc` installed but
`xorg-mkfontscale` missing, every directory is skipped and all lookups fall
back to the built-ins FPE, which looks exactly like having no fonts.
Installing the font packages *after* `xorg-mkfontscale`, or reinstalling
them, is what triggers the hook. Debian's `xfonts-*` packages ship the index,
so this is Arch-specific.

## Commits & PRs
- Sign your commits (SSH or GPG) so they show **Verified** — required to merge.
  https://docs.github.com/authentication/managing-commit-signature-verification
- Fork → branch → PR against `master`. Keep PRs focused; history stays linear
  (squash / fast-forward, no merge commits).

## Rendering / KMS changes
- CI has no GPU (Vulkan tests run on software lavapipe), so KMS/Vulkan render-path
  changes need real-hardware testing. Note in the PR whether you could test on HW.

## Portability
- Targets Linux (glibc + musl) and FreeBSD. Watch libc-isms — ioctl request types
  and struct fields differ across these; avoid nightly-only language features.
- Verification commands for portable compile gates:
  - Linux (glibc): `cargo check -p yserver`
  - Linux (musl): `cargo check -p yserver --target x86_64-unknown-linux-musl`
  - FreeBSD: `cargo check -p yserver --target x86_64-unknown-freebsd`
  Target toolchains can be added with `rustup target add <target>`.
