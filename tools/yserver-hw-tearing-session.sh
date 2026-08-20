#!/usr/bin/env bash
# Hardware validation script for Direct Scanout with Async Page Flip (Tearing)
# on NVIDIA / AMD hardware.
#
# Environment variables configured:
#   YSERVER_HW_CURSOR_NVIDIA=1   (Enables NVIDIA hardware cursor plane so direct scanout engages)
#   YSERVER_LOOP_TELEMETRY=1     (Enables per-second loop telemetry: page_flip/s, input latency, etc.)
#   YSERVER_SUBMIT_TRACE=...     (Logs submit trace TSV)
#   RUST_LOG=info (or debug)
#   RUST_BACKTRACE=1
#
# Usage (run from a separate TTY like tty2):
#   bash tools/yserver-hw-tearing-session.sh
#
# Environment overrides:
#   LOG=info|debug              (default: info)
#   DISPLAY=:7                  (default: :7)
#   SESSION_CMD=...             (default: dbus-run-session cinnamon-session)
#

set -euo pipefail

LOG="${LOG:-info}"
DISPLAY="${DISPLAY:-:7}"
SESSION_DISPLAY="${DISPLAY}"
SESSION_CMD="${SESSION_CMD:-dbus-run-session cinnamon-session}"

cd "$(dirname "$0")/.."

echo "==> Building yserver in release mode..."
cargo build --release --bin yserver

echo "==> Preparing log files..."
rm -f yserver-hw-session.log yserver-hw-session.submit.tsv session.log

echo "==> Launching yserver on DISPLAY=${DISPLAY} (log: ${LOG}) with NVIDIA HW Cursor override..."
YSERVER_LOOP_TELEMETRY=1 \
YSERVER_SUBMIT_TRACE=yserver-hw-session.submit.tsv \
YSERVER_HW_CURSOR_NVIDIA=1 \
RUST_LOG="${LOG}" \
RUST_BACKTRACE=1 \
target/release/yserver > yserver-hw-session.log 2>&1 &
yserver_pid=$!
echo "    yserver running (PID: $yserver_pid)"

cleanup() {
    echo "==> Stopping session and yserver..."
    kill -TERM "$yserver_pid" 2>/dev/null || true
    wait "$yserver_pid" 2>/dev/null || true
}
trap cleanup EXIT

echo "==> Waiting 2 seconds for yserver to initialize..."
sleep 2

echo "==> Launching desktop session (${SESSION_CMD}) on DISPLAY=${SESSION_DISPLAY}..."
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET \
    DISPLAY="${SESSION_DISPLAY}" GDK_BACKEND=x11 XDG_SESSION_TYPE=x11 \
    ${SESSION_CMD} > session.log 2>&1 &
session_pid=$!

echo "======================================================================"
echo " Session active! You can now launch your game (CS2 / Marvel Rivals)"
echo " in fullscreen with VSync DISABLED to test uncapped FPS & tearing."
echo " When done, exit the desktop session (or press Ctrl+C)."
echo "======================================================================"

wait "$session_pid" 2>/dev/null || true

echo "==> Desktop session exited. Stopping yserver..."
kill -TERM "$yserver_pid" 2>/dev/null || true
wait "$yserver_pid" 2>/dev/null || true
trap - EXIT

echo
echo "==================== TELEMETRY & ACCEPTANCE METRICS ===================="
echo "-- Recent page_flip/s samples:"
grep "loop telemetry" yserver-hw-session.log | grep -o "page_flip/s=[0-9.]*" | tail -30 || echo "   (no telemetry found)"
echo
echo "-- Direct Scanout Live Submits (expect > 0):"
grep -c "scanout_m2: live direct submit" yserver-hw-session.log || true
echo
echo "-- Direct Scanout Chain Submits:"
grep -c "scanout_m2: chain direct submit" yserver-hw-session.log || true
echo
echo "-- Composed Unflips Retired (expect near 0 in steady state):"
grep -c "scanout_m2: composed unflip retired" yserver-hw-session.log || true
echo
echo "-- Queued frames skipped (latest-wins drops):"
grep -c "queued frame skipped" yserver-hw-session.log || true
echo
echo "-- Present Skips per second (supersession coalescing):"
grep -c "present_skips/s=[1-9]" yserver-hw-session.log || true
echo
echo "-- Unexpected Exit Requests (expect 0):"
grep -i "request_exit" yserver-hw-session.log || echo "   (none - clean exit)"
echo "========================================================================"
