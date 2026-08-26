#!/usr/bin/env bash
# Controlled Phase-B/post-#95 counterbalanced A/B/A. Run from tty2 with the
# same PAIR_ID and release binary:
#   CASE=post95 RUN_LABEL=post95-1 PAIR_ID=phaseb-aba-01 ...
#   CASE=pre95  RUN_LABEL=pre95    PAIR_ID=phaseb-aba-01 ...
#   CASE=post95 RUN_LABEL=post95-2 PAIR_ID=phaseb-aba-01 ...
# Only flip visibility changes.

set -euo pipefail

CASE="${CASE:-}"
RUN_LABEL="${RUN_LABEL:-${CASE}}"
PAIR_ID="${PAIR_ID:-}"
WORKTREE="${WORKTREE:-/home/ariel_santangelo/Projects/yserver-phase-b}"
YSERVER_BIN="${YSERVER_BIN:-${WORKTREE}/target/release/yserver}"
SERVER_DISPLAY="${SERVER_DISPLAY:-:7}"
SESSION_CMD="${SESSION_CMD:-cinnamon-session}"
LOG="${LOG:-info,present_pace=debug}"
OUT_ROOT="${OUT_ROOT:-/home/ariel_santangelo/Projects/yserver/issue_115/phase-b-post95-ab}"

case "${CASE}" in post95|pre95) ;; *) echo "ERROR: set CASE=post95 or CASE=pre95"; exit 1 ;; esac
if [ -z "${PAIR_ID}" ] || [[ ! "${PAIR_ID}" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "ERROR: set a filesystem-safe PAIR_ID shared by both cases."
    exit 1
fi
if [ -z "${RUN_LABEL}" ] || [[ ! "${RUN_LABEL}" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "ERROR: RUN_LABEL must be filesystem-safe."
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
    echo "Build in ${WORKTREE} with: cargo build --release --bin yserver"
    exit 1
fi

commit="$(git -C "${WORKTREE}" rev-parse HEAD)"
binary_sha="$(sha256sum "${YSERVER_BIN}" | cut -d ' ' -f 1)"
pair_dir="${OUT_ROOT}/${PAIR_ID}"
case_dir="${pair_dir}/${RUN_LABEL}"
mkdir -p "${pair_dir}"
if [ -e "${case_dir}" ]; then
    echo "ERROR: case already exists: ${case_dir}"
    exit 1
fi

pair_binary_file="${pair_dir}/binary.sha256"
pair_commit_file="${pair_dir}/source.commit"
if [ -f "${pair_binary_file}" ]; then
    read -r expected_sha < "${pair_binary_file}"
    read -r expected_commit < "${pair_commit_file}"
    if [ "${binary_sha}" != "${expected_sha}" ] || [ "${commit}" != "${expected_commit}" ]; then
        echo "ERROR: binary/commit differs from the first case in pair ${PAIR_ID}."
        echo "expected commit=${expected_commit} sha256=${expected_sha}"
        echo "actual   commit=${commit} sha256=${binary_sha}"
        exit 1
    fi
else
    printf '%s\n' "${binary_sha}" > "${pair_binary_file}"
    printf '%s\n' "${commit}" > "${pair_commit_file}"
fi

mkdir "${case_dir}"
yserver_log="${case_dir}/yserver.log"
cinnamon_log="${case_dir}/cinnamon.log"
submit_trace="${case_dir}/submit.tsv"
markers="${case_dir}/markers.tsv"
metadata="${case_dir}/metadata.txt"
printf 'wall_time\tmonotonic_seconds\tphase\n' > "${markers}"

mark_phase() {
    printf '%s\t%s\t%s\n' "$(date --iso-8601=seconds)" "$(cut -d ' ' -f 1 /proc/uptime)" "$1" | tee -a "${markers}"
}

{
    echo "case=${CASE}"
    echo "run_label=${RUN_LABEL}"
    echo "pair_id=${PAIR_ID}"
    echo "started=$(date --iso-8601=seconds)"
    echo "tty=${tty_name}"
    echo "display=${SERVER_DISPLAY}"
    echo "worktree=${WORKTREE}"
    echo "commit=${commit}"
    echo "binary=${YSERVER_BIN}"
    echo "sha256=${binary_sha}"
    echo "YSERVER_PHASE_B_FLIP_VISIBILITY=${CASE}"
    echo "YSERVER_HW_CURSOR_NVIDIA=1"
    echo "kernel=$(uname -srmo)"
    command -v nvidia-smi >/dev/null 2>&1 && nvidia-smi || true
} > "${metadata}" 2>&1

yserver_pid=""
session_pid=""
cleanup() {
    if [ -n "${session_pid}" ]; then kill -TERM "${session_pid}" 2>/dev/null || true; wait "${session_pid}" 2>/dev/null || true; fi
    if [ -n "${yserver_pid}" ]; then kill -TERM "${yserver_pid}" 2>/dev/null || true; wait "${yserver_pid}" 2>/dev/null || true; fi
}
trap cleanup EXIT INT TERM

echo "==> Phase-B post-#95 A/B/A: ${RUN_LABEL}, mode ${CASE} (pair ${PAIR_ID})"
echo "    commit: ${commit}"
echo "    sha256: ${binary_sha}"
echo "    output: ${case_dir}"
echo "Keep identical: CS2 map/mode/settings, duration and actions."
read -r -p "Press Enter to launch yserver and Cinnamon. " _

YSERVER_LOOP_TELEMETRY=1 \
YSERVER_PRESENT_TRACE=1 \
YSERVER_SUBMIT_TRACE="${submit_trace}" \
YSERVER_HW_CURSOR_NVIDIA=1 \
YSERVER_PHASE_B_FLIP_VISIBILITY="${CASE}" \
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

read -r -p "After Cinnamon settles for 30 seconds, press Enter. " _
mark_phase "desktop-warmup-end"
read -r -p "Start CS2 fullscreen/no-vsync. At the main menu press Enter. " _
mark_phase "cs2-menu"
read -r -p "Enter the same map/match. When gameplay starts press Enter. " _
mark_phase "gameplay-start"
echo "==> Gameplay capture: 3 minutes. Play normally and observe the in-game Hz."
for remaining in 150 120 90 60 30 0; do
    sleep 30
    echo "    ${remaining} seconds remaining"
done
mark_phase "gameplay-end"
read -r -p "Alt-Tab to the desktop, then press Enter to start the 30-second recovery sample. " _
sleep 30
mark_phase "recovery-end"
read -r -p "Briefly describe perceived Hz/lag for this run: " observation
printf 'observation=%s\n' "${observation}" >> "${metadata}"
read -r -p "Press Enter to stop Cinnamon and finish capture. " _

cleanup
yserver_pid=""
session_pid=""
trap - EXIT INT TERM
echo "finished=$(date --iso-8601=seconds)" >> "${metadata}"

echo "===== ${RUN_LABEL} (${CASE}) summary ====="
direct_retires="$(grep -c "scanout_m2: direct frame retired" "${yserver_log}" || true)"
for pattern in "scanout_m2: direct frame retired" "scanout_m2: composed unflip retired" \
    "stage=parked_msc reason=flip_in_flight" "stage=superseded"
do
    printf '%-48s ' "${pattern}"
    grep -c "${pattern}" "${yserver_log}" || true
done
echo "-- recent page_flip/s --"
grep "loop telemetry" "${yserver_log}" | grep -o "page_flip/s=[0-9.]*" | tail -40 || true
echo "===== capture complete: ${case_dir} ====="
if [ "${direct_retires}" -eq 0 ]; then
    echo "ERROR: Phase B never retired a direct frame; this case is not a valid A/B probe."
    echo "Inspect the first scanout_m1 decline in ${yserver_log}."
    exit 2
fi
