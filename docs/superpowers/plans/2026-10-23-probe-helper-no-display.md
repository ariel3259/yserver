# The PRIME probe helper runs without `DISPLAY`

Short plan (no spec: one environment variable is removed from one child process).

## Problem

The disposable PRIME route probe helper (`internal_probe.rs`, `ProbeHelperSupervisor::run`) re-executes yserver with
the parent's whole environment, `DISPLAY` included. The NVIDIA Vulkan ICD (`libGLX_nvidia.so`, 615.78.08) opens the X
display named by `DISPLAY` while it initializes. When `DISPLAY` names yserver itself and the client that changed the
configuration holds a `GrabServer` (xrandr always grabs around `RRSetCrtcConfig`), the helper's X connection waits on the
grab, the grab waits on the parked `RRSetCrtcConfig`, and that waits on the helper: a deadlock until the 30 s watchdog,
after which the configuration fails (`RRSetCrtcConfig apply failed: probe helper child state is uncertain: disposable
probe helper watchdog expired`, `xrandr: Configure crtc 4 failed`).

Measured on master 7b74b7b1 with an RTX 5060 Ti primary and an AMD Raphael sink (`YSERVER_DRM_DEVICES=card1,card0`,
`xrandr --output HDMI-1 --auto`), `strace -ff`:

- the helper loads the NVIDIA ICD, connects to the server's own socket, and blocks waiting for the `QueryExtension`
  reply;
- the core is idle in `epoll_wait`: it receives that request at once and answers it only after the watchdog kill;
- xrandr's `GrabServer` arrives immediately before its `RRSetCrtcConfig`.

Control: the same run with `DISPLAY` unset for the server only. The helper opens no X connection, xrandr returns in
0.85 s, HDMI-1 lights at 1920x1080 and the route qualifies.

`starty` exports `DISPLAY` only after the server is up, so the normal path does not hit this. A server started with
`DISPLAY` already set to its own display does, for example from a terminal or a test harness.

## Change

- The helper `Command` gets `.env_remove("DISPLAY")`. The rest of the environment is kept, because the Vulkan loader
  may depend on it (`VK_ICD_FILENAMES`, `LD_LIBRARY_PATH`, …); `env_clear()` is ruled out for that reason.
- The `Command` construction moves into one small function so a test can inspect it; the code stays the same.
- The note in `specs/2026-07-14-reverse-prime-copy-design.md` says the helper runs without `DISPLAY`. The same note
  says that "continuing to service … other clients" does not hold under a `GrabServer`.

Out of scope: `GrabServer` semantics (blocking other clients is correct X11 and Xorg behaviour; the defect is only that
the helper became a client), `WAYLAND_DISPLAY` (no measured failure), the watchdog, the probe FIFO.

## Tests

- The helper `Command` removes `DISPLAY` (`Command::get_envs` reports it as removed) and passes another variable
  through unchanged. Mutation: drop the `env_remove`; the test must fail.
- Gate: fmt, `cargo build --bin yserver`, workspace + lavapipe ignored suite, clippy `-D warnings`,
  `tools/check-layout.sh`, FreeBSD cross-clippy.

## Hardware

The copied route above, with the server started **with** `DISPLAY` set to its own display. Master: fails after 30 s
(`watchdog expired`). Branch: `xrandr --output HDMI-1 --auto` succeeds in under 1 s, HDMI-1 lights, and the helper
opens 0 X connections in `strace -ff`.
