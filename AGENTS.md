# yserver

A modern X11 server written from scratch in Rust.

Info about the project is in README.md
Current status is in docs/status.md and should be kept up to date
focus is on yserver (KMS) now

## Instructions

- it's fine not to use clippy pedantic in this repo but DO use regular clippy
- before committing, run clippy exactly as CI does: `cargo clippy --all-targets -- -D warnings`. CI fails on any warning, and `--all-targets` lints test code too — a crate-scoped or non-`--all-targets` run misses lints in tests (e.g. needless_range_loop) and they only surface on GH.
- use `cargo +nightly fmt` for formatting
- when adding or changing ioctls, watch for libc linuxisms: request-type aliases like `libc::Ioctl` are not portable across all supported targets, and musl/FreeBSD have regressed here before. Keep ioctl request typing/buildability valid on Linux glibc, Linux musl, and FreeBSD.
- design docs (specs) go in docs/superpowers/specs
- impl plans go in docs/superpowers/plans
- adversarial review of a spec or plan goes through docs/superpowers/review/review.sh,
  which holds the frozen brief and pins the reviewer model, so finding counts stay
  comparable between rounds; write the result to docs/superpowers/findings and paste in
  the provenance block it prints. See docs/superpowers/review/README.md.
- work on feature branch for phases
- squash merge when ready (ask confirmation)
- Spec compliance is the goal, but if Xorg deviates from spec (unlikely), we need to follow Xorg, clients are tested for 40+ years on Xorg.

- tests must exercise the real paths, for every feature and spec:
  - **end-state check**: a scenario test that allocates or retires resources ends by checking nothing is left
    behind that should not be (no unjustified retained objects, the resource ledger equal to the expected live
    set, no buffer stuck in an intermediate phase) — not only the value the author expected to change;
  - **production driver**: tests advance the backend only through the entry points the core loop itself calls
    (the `Backend` trait methods such as `before_block`, `on_owner_completion_ready`, `next_wakeup`), never a
    hand-rolled loop that imitates them; a stub may stand in for the kernel only where the test says what the
    stub cannot reproduce;
  - **hardware per task**: a task that touches a real KMS/GPU path runs its hardware test when that task is done,
    not at the end of the plan (ask first when the machine is in use).
  Plans name these checks per test, and plan reviews ask whether each scenario comes from production entries and
  whether a stub hides a side effect the real kernel/driver produces.

## environment

- you are most likely running in a bwrap sandbox, if you see /home/jos/realhome, you are.
- the project dir is rw mounted
- in /home/jos/Projects/xserver/hw/kdrive/ephyr/ you can find the source to Xephyr for reference
- in /home/jos/Projects/xserver/hw/xnest/ you can find the sources to Xnest for reference
- to test you can run ynest with RUST_LOG=debug, capture its output
- x11trace is available if you want to trace how Xephyr/Xnest does things, x11trace always needs -n flag
