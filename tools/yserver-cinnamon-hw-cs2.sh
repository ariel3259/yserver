#!/usr/bin/env bash
# Task 6 of docs/superpowers/plans/2026-08-12-direct-scanout-latest-wins-supersession.md:
# hardware validation of the direct-level latest-wins supersession on the nvidia box.
#
# Builds release yserver, launches it on DISPLAY=:7 with the Phase B A/B override
# (YSERVER_HW_CURSOR_NVIDIA=1) + loop telemetry + submit trace, then boots a
# dbus-isolated cinnamon-session on top. After you quit Cinnamon (or kill this
# script), yserver is stopped and the acceptance-metric greps are printed.
#
# Run from tty2 (or wherever you can take VT + DRM master):
#   bash tools/yserver-cinnamon-hw-cs2.sh
#
# Args (env vars):
#   LOG     default: info   (RUST_LOG; telemetry lines are `info`)
#   DISPLAY default: :7     (host X display for yserver)
#   SESSION_DISPLAY default: :7  (Cinnamon's DISPLAY; keep equal to DISPLAY)
#   RUN_CS2 default: 1      (set RUN_CS2=0 to skip launching steam/cs2)
#
# After the run, verification greps (see plan Task 6 Step 3/4):
#   grep "loop telemetry"  yserver-hw-cinnamon.log | grep -o "page_flip/s=[0-9.]*" | tail -40
#   grep -c "scanout_m2: composed unflip retired"  yserver-hw-cinnamon.log   # expect ~0
#   grep -c "scanout_m2: live direct submit"       yserver-hw-cinnamon.log   # expect > 0
#   grep -c "chain direct submit failed"           yserver-hw-cinnamon.log   # expect 0
#   grep -c "queued frame skipped"                 yserver-hw-cinnamon.log   # 0 or small
#   grep -i "request_exit"                         yserver-hw-cinnamon.log   # expect nothing

set -euo pipefail

LOG="${LOG:-info}"
DISPLAY="${DISPLAY:-:7}"
SESSION_DISPLAY="${SESSION_DISPLAY:-:7}"
RUN_CS2="${RUN_CS2:-1}"

cd "$(dirname "$0")/.."

echo "==> cargo build --release --bin yserver"
cargo build --release --bin yserver

echo "==> launching yserver on DISPLAY=${DISPLAY} (log: ${LOG})"
rm -f yserver-hw-cinnamon.log yserver-cinnamon.submit.tsv

YSERVER_LOOP_TELEMETRY=1 \
YSERVER_SUBMIT_TRACE=yserver-cinnamon.submit.tsv \
YSERVER_HW_CURSOR_NVIDIA=1 \
RUST_LOG="${LOG}" \
RUST_BACKTRACE=1 \
target/release/yserver > yserver-hw-cinnamon.log 2>&1 &
yserver_pid=$!
echo "    yserver pid=$yserver_pid"

trap 'echo "==> stopping yserver"; kill -TERM $yserver_pid 2>/dev/null || true; wait $yserver_pid 2>/dev/null || true' EXIT

sleep 2

echo "==> launching cinnamon-session on DISPLAY=${SESSION_DISPLAY}"
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET \
    DISPLAY="${SESSION_DISPLAY}" GDK_BACKEND=x11 XDG_SESSION_TYPE=x11 \
    dbus-run-session cinnamon-session > cinnamon.log 2>&1 &
cinnamon_pid=$!

if [ "${RUN_CS2}" = "1" ]; then
    echo "==> play CS2 fullscreen vsync OFF for ~3 minutes, then exit Cinnamon"
fi

wait $cinnamon_pid

echo "==> cinnamon exited; stopping yserver"
kill -TERM $yserver_pid 2>/dev/null || true
wait $yserver_pid 2>/dev/null || true
trap - EXIT

echo
echo "===== acceptance metrics (plan Task 6 Step 3/4) ====="
echo "-- page_flip/s (tail 40) -- expected ~60 sustained (was 54-56):"
grep "loop telemetry" yserver-hw-cinnamon.log | grep -o "page_flip/s=[0-9.]*" | tail -40 || echo "   (no page_flip/s in log)"
echo "-- composed unflip retired count -- expected near 0:"
grep -c "scanout_m2: composed unflip retired" yserver-hw-cinnamon.log || true
echo "-- live direct submit count -- expected > 0:"
grep -c "scanout_m2: live direct submit" yserver-hw-cinnamon.log || true
echo "-- request_exit -- expected nothing:"
grep -i "request_exit" yserver-hw-cinnamon.log || echo "   (none)"
echo "-- chain direct submit failed -- expected 0:"
grep -c "chain direct submit failed" yserver-hw-cinnamon.log || true
echo "-- queued frame skipped -- expected 0 or small:"
grep -c "queued frame skipped" yserver-hw-cinnamon.log || true
echo "-- DRM_IOCTL_CRTC_QUEUE_SEQUENCE unsupported warning -- expected absent:"
grep -c "EOPNOTSUPP" yserver-hw-cinnamon.log || true
echo "===== done ====="
