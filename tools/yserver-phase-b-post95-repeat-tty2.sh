#!/usr/bin/env bash
# Repeatability probe for Phase B after #95. Run twice from tty2 with the same
# PAIR_ID and release binary, changing only RUN_LABEL:
#   RUN_LABEL=post95-1 PAIR_ID=phaseb-post95-repeat-01 ./tools/yserver-phase-b-post95-repeat-tty2.sh
#   RUN_LABEL=post95-2 PAIR_ID=phaseb-post95-repeat-01 ./tools/yserver-phase-b-post95-repeat-tty2.sh

set -euo pipefail

RUN_LABEL="${RUN_LABEL:-}"
PAIR_ID="${PAIR_ID:-}"
WORKTREE="${WORKTREE:-/home/ariel_santangelo/Projects/yserver-phase-b}"
YSERVER_BIN="${YSERVER_BIN:-${WORKTREE}/target/release/yserver}"
SERVER_DISPLAY="${SERVER_DISPLAY:-:7}"
SESSION_CMD="${SESSION_CMD:-cinnamon-session}"
CAPTURE_MINUTES="${CAPTURE_MINUTES:-6}"
LOG="${LOG:-info,present_pace=debug}"
OUT_ROOT="${OUT_ROOT:-/home/ariel_santangelo/Projects/yserver/issue_115/phase-b-post95-repeat}"

if [ -z "${PAIR_ID}" ] || [[ ! "${PAIR_ID}" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "ERROR: set a filesystem-safe PAIR_ID shared by both runs."
    exit 1
fi
if [ -z "${RUN_LABEL}" ] || [[ ! "${RUN_LABEL}" =~ ^post95-[12]$ ]]; then
    echo "ERROR: set RUN_LABEL=post95-1 or RUN_LABEL=post95-2."
    exit 1
fi
if [[ ! "${CAPTURE_MINUTES}" =~ ^[1-9][0-9]*$ ]]; then
    echo "ERROR: CAPTURE_MINUTES must be a positive integer."
    exit 1
fi
if [ ! -t 0 ]; then echo "ERROR: run interactively from tty2."; exit 1; fi
tty_name="$(tty)"
if [ "${tty_name}" != /dev/tty2 ]; then
    echo "WARNING: expected /dev/tty2, running from ${tty_name}."
    read -r -p "Continue anyway? [y/N] " answer
    case "${answer}" in y|Y|yes|YES) ;; *) exit 1 ;; esac
fi
if [ ! -x "${YSERVER_BIN}" ]; then
    echo "ERROR: missing release binary: ${YSERVER_BIN}"
    exit 1
fi

commit="$(git -C "${WORKTREE}" rev-parse HEAD)"
binary_sha="$(sha256sum "${YSERVER_BIN}" | cut -d ' ' -f 1)"
pair_dir="${OUT_ROOT}/${PAIR_ID}"
run_dir="${pair_dir}/${RUN_LABEL}"
mkdir -p "${pair_dir}"
if [ -e "${run_dir}" ]; then
    echo "ERROR: run already exists: ${run_dir}"
    exit 1
fi

if [ -f "${pair_dir}/binary.sha256" ]; then
    read -r expected_sha < "${pair_dir}/binary.sha256"
    read -r expected_commit < "${pair_dir}/source.commit"
    read -r expected_minutes < "${pair_dir}/capture_minutes"
    if [ "${binary_sha}" != "${expected_sha}" ] || [ "${commit}" != "${expected_commit}" ] ||
       [ "${CAPTURE_MINUTES}" != "${expected_minutes}" ]; then
        echo "ERROR: binary, commit, or duration differs from the first run."
        echo "expected commit=${expected_commit} sha256=${expected_sha} minutes=${expected_minutes}"
        echo "actual   commit=${commit} sha256=${binary_sha} minutes=${CAPTURE_MINUTES}"
        exit 1
    fi
else
    printf '%s\n' "${binary_sha}" > "${pair_dir}/binary.sha256"
    printf '%s\n' "${commit}" > "${pair_dir}/source.commit"
    printf '%s\n' "${CAPTURE_MINUTES}" > "${pair_dir}/capture_minutes"
fi

mkdir "${run_dir}"
yserver_log="${run_dir}/yserver.log"
cinnamon_log="${run_dir}/cinnamon.log"
submit_trace="${run_dir}/submit.tsv"
markers="${run_dir}/markers.tsv"
metadata="${run_dir}/metadata.txt"
gpu_csv="${run_dir}/nvidia.csv"
printf 'wall_time\tepoch_seconds\tmonotonic_seconds\tphase\n' > "${markers}"

mark_phase() {
    printf '%s\t%s\t%s\t%s\n' "$(date --iso-8601=seconds)" "$(date +%s)" \
        "$(cut -d ' ' -f 1 /proc/uptime)" "$1" | tee -a "${markers}"
}

{
    echo "mode=post95"
    echo "run_label=${RUN_LABEL}"
    echo "pair_id=${PAIR_ID}"
    echo "started=$(date --iso-8601=seconds)"
    echo "tty=${tty_name}"
    echo "display=${SERVER_DISPLAY}"
    echo "commit=${commit}"
    echo "binary=${YSERVER_BIN}"
    echo "sha256=${binary_sha}"
    echo "capture_minutes=${CAPTURE_MINUTES}"
    echo "kernel=$(uname -srmo)"
    command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi || true
} > "${metadata}" 2>&1

yserver_pid=""
session_pid=""
gpu_pid=""
cleanup() {
    if [ -n "${gpu_pid}" ]; then kill -TERM "${gpu_pid}" 2>/dev/null || true; wait "${gpu_pid}" 2>/dev/null || true; fi
    if [ -n "${session_pid}" ]; then kill -TERM "${session_pid}" 2>/dev/null || true; wait "${session_pid}" 2>/dev/null || true; fi
    if [ -n "${yserver_pid}" ]; then kill -TERM "${yserver_pid}" 2>/dev/null || true; wait "${yserver_pid}" 2>/dev/null || true; fi
}
trap cleanup EXIT INT TERM

echo "==> Phase-B post95 repeatability: ${RUN_LABEL} (pair ${PAIR_ID})"
echo "    Keep identical: map/demo, settings, route and actions."
echo "    Do not stop yserver during the timed capture."
read -r -p "Press Enter to launch yserver and Cinnamon. " _

server_start_mono="$(cut -d ' ' -f 1 /proc/uptime)"
printf 'server_start_monotonic_seconds=%s\n' "${server_start_mono}" >> "${metadata}"
YSERVER_LOOP_TELEMETRY=1 \
YSERVER_PRESENT_TRACE=1 \
YSERVER_SUBMIT_TRACE="${submit_trace}" \
DISPLAY="${SERVER_DISPLAY}" RUST_LOG="${LOG}" RUST_BACKTRACE=1 \
"${YSERVER_BIN}" > "${yserver_log}" 2>&1 &
yserver_pid=$!
sleep 2
if ! kill -0 "${yserver_pid}" 2>/dev/null; then
    echo "ERROR: yserver exited during startup. Inspect ${yserver_log}"
    exit 1
fi

env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET DISPLAY="${SERVER_DISPLAY}" \
    GDK_BACKEND=x11 XDG_SESSION_TYPE=x11 dbus-run-session ${SESSION_CMD} \
    > "${cinnamon_log}" 2>&1 &
session_pid=$!

if command -v nvidia-smi >/dev/null 2>&1; then
    nvidia-smi --query-gpu=timestamp,pstate,temperature.gpu,utilization.gpu,utilization.memory,clocks.current.graphics,clocks.current.memory,power.draw \
        --format=csv -l 1 > "${gpu_csv}" 2>&1 &
    gpu_pid=$!
fi

read -r -p "After Cinnamon settles for 30 seconds, press Enter. " _
mark_phase "desktop-warmup-end"
read -r -p "Start CS2 fullscreen/no-vsync. At the main menu press Enter. " _
mark_phase "cs2-menu"
read -r -p "Start the SAME demo/map and route. At gameplay start press Enter. " _
mark_phase "gameplay-start"
echo "==> Timed capture: ${CAPTURE_MINUTES} minutes."
for ((minute = 1; minute <= CAPTURE_MINUTES; minute++)); do
    sleep 60
    mark_phase "gameplay-minute-${minute}"
done
mark_phase "gameplay-end"

read -r -p "Alt-Tab to desktop and press Enter for a 30-second recovery sample. " _
mark_phase "recovery-start"
sleep 30
mark_phase "recovery-end"
read -r -p "Perceived lag (none/rare/late/continuous + when): " observation
printf 'observation=%s\n' "${observation}" >> "${metadata}"
read -r -p "Press Enter to stop the session and complete the run. " _

cleanup
yserver_pid=""
session_pid=""
gpu_pid=""
trap - EXIT INT TERM
echo "finished=$(date --iso-8601=seconds)" >> "${metadata}"

direct_retires="$(grep -c 'scanout_m2: direct frame retired' "${yserver_log}" || true)"
echo "===== ${RUN_LABEL} complete: direct retires=${direct_retires} ====="
echo "Output: ${run_dir}"
if [ "${direct_retires}" -eq 0 ]; then
    echo "ERROR: Phase B never retired a direct frame; this run is invalid."
    exit 2
fi
echo "After both runs: tools/analyze-phase-b-post95-repeat.sh ${pair_dir}"
