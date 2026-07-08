# Contributing to yserver

## Toolchain
- Use rustup — the repo pins its toolchain in `rust-toolchain.toml` (stable),
  and rustup honors it automatically. Distro `rustc` (apt/pacman) ignores that
  pin and can lag a release behind; if it won't build, use rustup.
- Nightly is only used for formatting (`cargo +nightly fmt`).

## Before opening a PR — run what CI runs (CI denies warnings)
- `cargo +nightly fmt`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`

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
