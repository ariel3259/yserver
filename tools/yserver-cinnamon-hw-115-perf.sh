#!/usr/bin/env bash
# Hardware validation harness for Issue #115:
# XRender Submit Coalescing & Desktop Compositor Animation Performance.
#
# Runs yserver on a dedicated VT / display with Cinnamon session, capturing
# loop telemetry and submit traces to verify submit storm elimination and
# smooth frame pacing.
#
# Run from tty2 (or wherever you can take VT + DRM master):
#   bash tools/yserver-cinnamon-hw-115-perf.sh
#
# Environment variables:
#   LOG             default: info
#   DISPLAY         default: :7
#   SESSION_DISPLAY default: :7
#   BUILD           default: 0 (set BUILD=1 to rebuild target/release/yserver)
#   SESSION_CMD     default: cinnamon-session

set -euo pipefail

LOG="${LOG:-info}"
DISPLAY="${DISPLAY:-:7}"
SESSION_DISPLAY="${SESSION_DISPLAY:-:7}"
BUILD="${BUILD:-0}"
SESSION_CMD="${SESSION_CMD:-cinnamon-session}"

cd "$(dirname "$0")/.."

if [ "${BUILD}" = "1" ] || [ ! -f "target/release/yserver" ]; then
    echo "==> Building yserver (release)..."
    cargo build --release --bin yserver
fi

LOG_FILE="yserver-hw-cinnamon-115.log"
SUBMIT_FILE="yserver-cinnamon-115.submit.tsv"

rm -f "${LOG_FILE}" "${SUBMIT_FILE}"

echo "==> Launching yserver on DISPLAY=${DISPLAY} (log: ${LOG_FILE}, trace: ${SUBMIT_FILE})"

YSERVER_LOOP_TELEMETRY=1 \
YSERVER_SUBMIT_TRACE="${SUBMIT_FILE}" \
RUST_LOG="${LOG}" \
RUST_BACKTRACE=1 \
target/release/yserver > "${LOG_FILE}" 2>&1 &
yserver_pid=$!

echo "    yserver pid=${yserver_pid}"

trap 'echo "==> Stopping yserver..."; kill -TERM ${yserver_pid} 2>/dev/null || true; wait ${yserver_pid} 2>/dev/null || true' EXIT

sleep 2

if ! kill -0 ${yserver_pid} 2>/dev/null; then
    echo "ERROR: yserver failed to start. Check ${LOG_FILE}:"
    cat "${LOG_FILE}"
    exit 1
fi

echo "==> Launching ${SESSION_CMD} on DISPLAY=${SESSION_DISPLAY}"
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET \
    DISPLAY="${SESSION_DISPLAY}" GDK_BACKEND=x11 XDG_SESSION_TYPE=x11 \
    dbus-run-session ${SESSION_CMD} > cinnamon-115.log 2>&1 &
session_pid=$!

echo "    session pid=${session_pid}"
echo "==> Test desktop animations, window dragging, and menu popups for ~1-2 minutes."
echo "    Exit Cinnamon (or Ctrl+C) to terminate the test and see results."

wait ${session_pid} 2>/dev/null || true

echo "==> Session exited; stopping yserver"
kill -TERM ${yserver_pid} 2>/dev/null || true
wait ${yserver_pid} 2>/dev/null || true
trap - EXIT

echo
echo "================ Acceptance Metrics (Issue #115) ================"
if [ -f "${SUBMIT_FILE}" ]; then
    total_submits=$(wc -l < "${SUBMIT_FILE}" | awk '{print $1 - 1}')
    echo "-- Total Vulkan Queue Submits: ${total_submits}"
    python3 -c "
import csv
from collections import Counter
tsv = '${SUBMIT_FILE}'
kinds = Counter()
sec_counts = {}
first_ns = None
with open(tsv, 'r') as f:
    r = csv.DictReader(f, delimiter='\t')
    for row in r:
        ns = int(row['ns_mono'])
        if first_ns is None: first_ns = ns
        sec = int((ns - first_ns) // 1_000_000_000)
        sec_counts[sec] = sec_counts.get(sec, 0) + 1
        kinds[row['kind']] += 1

if sec_counts:
    dur = max(sec_counts.keys()) + 1
    avg = sum(sec_counts.values()) / max(1, dur)
    peak = max(sec_counts.values())
    print(f'-- Average Submits/sec: {avg:.1f} (target: <120/s, was >1500/s)')
    print(f'-- Peak Submits/sec:    {peak} (was >12000/s)')
    print(f'-- Breakdown: {dict(kinds)}')
" 2>/dev/null || true
else
    echo "-- Submit trace file not generated."
fi

echo
echo "-- Loop Telemetry (Page Flip Rate & Request Time under load):"
if [ -f "${LOG_FILE}" ]; then
    grep "loop telemetry" "${LOG_FILE}" | tail -n 25 || echo "   (no loop telemetry lines found)"
fi

echo "=================================================================="
