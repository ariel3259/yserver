#!/usr/bin/env bash
# Compare two captures produced by yserver-phase-b-post95-repeat-tty2.sh.

set -euo pipefail

PAIR_DIR="${1:-}"
if [ -z "${PAIR_DIR}" ]; then
    echo "Usage: $0 /path/to/PAIR_ID"
    exit 1
fi

for run in post95-1 post95-2; do
    for file in yserver.log markers.tsv metadata.txt; do
        if [ ! -f "${PAIR_DIR}/${run}/${file}" ]; then
            echo "ERROR: missing ${PAIR_DIR}/${run}/${file}"
            exit 1
        fi
    done
done

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

analyze_run() {
    local run="$1"
    local dir="${PAIR_DIR}/${run}"
    local log="${dir}/yserver.log"
    local start_mono gameplay_mono start_ms end_ms
    start_mono="$(sed -n 's/^server_start_monotonic_seconds=//p' "${dir}/metadata.txt")"
    gameplay_mono="$(awk -F '\t' '$4 == "gameplay-start" { print $3; exit }' "${dir}/markers.tsv")"
    end_mono="$(awk -F '\t' '$4 == "gameplay-end" { print $3; exit }' "${dir}/markers.tsv")"
    if [ -z "${start_mono}" ] || [ -z "${gameplay_mono}" ] || [ -z "${end_mono}" ]; then
        echo "ERROR: ${run} lacks complete timing markers"
        exit 1
    fi
    start_ms="$(awk -v a="${gameplay_mono}" -v b="${start_mono}" 'BEGIN { printf "%.0f", (a-b)*1000 }')"
    end_ms="$(awk -v a="${end_mono}" -v b="${start_mono}" 'BEGIN { printf "%.0f", (a-b)*1000 }')"

    awk -v begin="${start_ms}" -v end="${end_ms}" '
        /PACE-INSTR/ {
            t = -1
            for (i=1; i<=NF; i++) {
                if ($i ~ /^t=[0-9]+$/) { split($i,a,"="); t=a[2]+0 }
                if ($i ~ /^stage=/) { split($i,s,"="); stage=s[2] }
            }
            if (t >= begin && t <= end) count[stage]++
        }
        END {
            printf "requests=%d parked=%d superseded=%d fired=%d acquired_deferred=%d\n", \
                count["request"], count["parked_msc"], count["superseded"], \
                count["fired"], count["acquire_deferred"]
        }
    ' "${log}" > "${tmp_dir}/${run}.pace"

    awk -v begin="${start_ms}" -v end="${end_ms}" '
        /PACE-INSTR/ {
            t=-1; stage=""
            for (i=1; i<=NF; i++) {
                if ($i ~ /^t=[0-9]+$/) { split($i,a,"="); t=a[2]+0 }
                if ($i ~ /^stage=/) { split($i,s,"="); stage=s[2] }
            }
            if (t >= begin && t <= end) {
                minute=int((t-begin)/60000)+1
                if (stage == "request") requests[minute]++
                else if (stage == "parked_msc") parked[minute]++
                else if (stage == "superseded") superseded[minute]++
                else if (stage == "fired") fired[minute]++
                if (minute > max_minute) max_minute=minute
            }
        }
        END {
            print "minute requests parked superseded fired"
            for (m=1; m<=max_minute; m++)
                printf "%d %d %d %d %d\n",m,requests[m],parked[m],superseded[m],fired[m]
        }
    ' "${log}" > "${tmp_dir}/${run}.minutes"

    # UST is monotonic microseconds. The first clock sample anchors it to the
    # server start; retain only deltas whose relative time is in gameplay.
    awk -v begin="${start_ms}" -v end="${end_ms}" '
        /present_clock sample source=pageflip/ {
            ust=-1
            for (i=1; i<=NF; i++) if ($i ~ /^ust=[0-9]+$/) { split($i,a,"="); ust=a[2]+0 }
            if (ust < 0) next
            if (!base) { base=ust; prev=ust; next }
            rel=(ust-base)/1000
            if (rel >= begin && rel <= end) {
                delta=ust-prev
                if (delta > 0) {
                    print delta > deltas
                    minute=int((rel-begin)/60000)+1
                    samples[minute]++
                    sum[minute]+=delta
                    if (delta>20000) over20[minute]++
                    if (delta>33333) over33[minute]++
                    if (delta>maximum[minute]) maximum[minute]=delta
                    if (minute>max_minute) max_minute=minute
                }
            }
            prev=ust
        }
        END {
            print "minute samples avg_ms max_ms over20ms over33ms" > minutes
            for (m=1; m<=max_minute; m++) {
                avg=samples[m] ? sum[m]/samples[m]/1000 : 0
                printf "%d %d %.3f %.3f %d %d\n",m,samples[m],avg,maximum[m]/1000,over20[m],over33[m] > minutes
            }
        }
    ' deltas="${tmp_dir}/${run}.deltas.unsorted" minutes="${tmp_dir}/${run}.flip-minutes" "${log}"
    sort -n "${tmp_dir}/${run}.deltas.unsorted" > "${tmp_dir}/${run}.deltas"

    local n p50 p95 p99 max over20 over25 over33
    n="$(wc -l < "${tmp_dir}/${run}.deltas")"
    if [ "${n}" -gt 0 ]; then
        read -r p50 p95 p99 max over20 over25 over33 < <(awk -v n="${n}" '
            { v[NR]=$1; if ($1>20000) a++; if ($1>25000) b++; if ($1>33333) c++ }
            END {
                i50=int((n-1)*0.50)+1; i95=int((n-1)*0.95)+1; i99=int((n-1)*0.99)+1
                printf "%.3f %.3f %.3f %.3f %d %d %d\n", v[i50]/1000, v[i95]/1000, \
                    v[i99]/1000, v[n]/1000, a, b, c
            }' "${tmp_dir}/${run}.deltas")
    else
        p50=0 p95=0 p99=0 max=0 over20=0 over25=0 over33=0
    fi

    local direct unflip errors observation
    direct="$(grep -c 'scanout_m2: direct frame retired' "${log}" || true)"
    unflip="$(grep -c 'scanout_m2: composed unflip retired' "${log}" || true)"
    errors="$(grep -Ec 'panicked at|fatal|ERROR' "${log}" || true)"
    observation="$(sed -n 's/^observation=//p' "${dir}/metadata.txt")"
    echo "===== ${run} ====="
    echo "observation=${observation:-MISSING}"
    echo "direct_retires_total=${direct} composed_unflips_total=${unflip} errors=${errors}"
    cat "${tmp_dir}/${run}.pace"
    printf 'pageflip_intervals=%d p50_ms=%s p95_ms=%s p99_ms=%s max_ms=%s >20ms=%s >25ms=%s >33ms=%s\n' \
        "${n}" "${p50}" "${p95}" "${p99}" "${max}" "${over20}" "${over25}" "${over33}"
    echo "-- Present stages per gameplay minute --"
    cat "${tmp_dir}/${run}.minutes"
    echo "-- KMS page-flip intervals per gameplay minute --"
    cat "${tmp_dir}/${run}.flip-minutes"
    if [ -s "${dir}/nvidia.csv" ]; then
        awk -F ',' 'NR>1 { gsub(/[% W]/,"",$4); if ($4 ~ /^[0-9.]+$/) {n++; sum+=$4; if ($4>max) max=$4} } \
            END {if(n) printf "gpu_util_avg=%.1f%% gpu_util_max=%.1f%%\n",sum/n,max}' "${dir}/nvidia.csv"
    fi
}

echo "commit=$(cat "${PAIR_DIR}/source.commit")"
echo "sha256=$(cat "${PAIR_DIR}/binary.sha256")"
analyze_run post95-1
analyze_run post95-2
