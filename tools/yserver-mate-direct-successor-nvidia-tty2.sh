#!/usr/bin/env bash
# One-command NVIDIA/MATE hardware capture for the direct-successor queue.
# Run interactively from tty2 after building target/release/yserver.

set -euo pipefail

script_path="$(readlink -f -- "${BASH_SOURCE[0]}")"
script_dir="$(cd -- "$(dirname -- "${script_path}")" && pwd)"
worktree="$(cd -- "${script_dir}/.." && pwd)"
pair_id="${PAIR_ID:-mate-direct-successor-nvidia-$(date +%Y%m%d-%H%M%S)}"
out_root="${OUT_ROOT:-/tmp/yserver-mate-direct-successor}"

if [ ! -x "${worktree}/target/release/yserver" ]; then
    echo "ERROR: falta ${worktree}/target/release/yserver"
    echo "Compilar primero con: cargo build --release --bin yserver"
    exit 1
fi

echo "NVIDIA/MATE direct-successor capture"
echo "pair_id=${pair_id}"
echo "output=${out_root}/${pair_id}/post95-1"

# One minute is enough to aggregate the new per-gate diagnostics. The final
# successor-queue validation can override this back to three.
exec env \
    RUN_LABEL=post95-1 \
    PAIR_ID="${pair_id}" \
    WORKTREE="${worktree}" \
    YSERVER_BIN="${worktree}/target/release/yserver" \
    SESSION_CMD="${script_dir}/mate-no-compositor-session.sh" \
    CAPTURE_MINUTES="${CAPTURE_MINUTES:-1}" \
    OUT_ROOT="${out_root}" \
    LOG="info,yserver::kms::render::backend=debug,present_pace=debug" \
    "${script_dir}/yserver-phase-b-post95-repeat-tty2.sh"
