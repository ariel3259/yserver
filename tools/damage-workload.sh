#!/usr/bin/env bash
# Deterministic, phased workload for before/after damage-repaint measurements.
#
# WHY THIS EXISTS
#
# Two human-driven desktop sessions differ in more dimensions than any counter
# captures — window count, what is on screen, where the video sits, panel
# activity. On 2026-09-02 a step-2 before/after on silence produced −63%, −17%,
# +14% and +34% across four paint-load bins and settled nothing, because the
# effect being measured was smaller than the between-session variance. This
# script removes that variance: the same events at the same times, so two
# branches see identical input.
#
# THE DESIGN
#
# Content damage is held CONSTANT and structural damage is toggled by phase:
#
#   * mpv plays a fixed clip at a fixed geometry for the whole run, so
#     per-frame content damage is the same in every phase.
#   * a separate xterm is moved / resized only during the `drag` and `resize`
#     phases, so structural damage is the only thing that varies.
#
# Phase boundaries are written to a phases file with wall-clock timestamps.
# `tools/damage-phases.py` joins those against the `render_telemetry:` lines,
# which carry the same clock, and reports per-phase medians. So one run yields
# several comparable populations rather than one blended average — which is the
# other half of what went wrong on 2026-09-02.
#
# USAGE
#
#   DISPLAY=:7 tools/damage-workload.sh <clip> <phases-file> [scale]
#
# `scale` multiplies every phase duration (default 1). Normally driven by
# `just yserver-*-hw-workload`, not run by hand.

set -u

clip=${1:?usage: damage-workload.sh <clip> <phases-file> [scale]}
phases=${2:?usage: damage-workload.sh <clip> <phases-file> [scale]}
scale=${3:-1}

# Fixed geometry so the workload is identical across runs and across branches.
readonly MPV_GEOM=960x540+80+80
readonly TERM_TITLE=yserver-damage-workload
readonly TERM_HOME=(1100 120)
readonly TERM_AWAY=(1500 620)
# Pixels, not character cells: `xdotool windowsize` is in pixels by default, so
# driving the size in cells here and in pixels there would silently resize the
# window to a few pixels across.
readonly TERM_SMALL=(600 400)
readonly TERM_LARGE=(1000 700)

: >"$phases"

mark() {
    # ISO-8601 UTC to match the log prefix `[2026-09-02T08:53:20Z ...]`.
    printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$phases"
}

dur() {
    # Durations scale together so a quick smoke and a real measurement run the
    # same shape.
    awk -v s="$scale" -v d="$1" 'BEGIN { printf "%.1f", d * s }'
}

cleanup() {
    [ -n "${mpv_pid:-}" ] && kill -TERM "$mpv_pid" 2>/dev/null
    [ -n "${term_pid:-}" ] && kill -TERM "$term_pid" 2>/dev/null
    mark shutdown
}
trap cleanup EXIT

if [ ! -r "$clip" ]; then
    echo "damage-workload: clip not readable: $clip" >&2
    exit 1
fi
mark clip-ok

# Content load: fixed clip, fixed size, looping so it lasts the whole run, and
# no OSD or window-manager interaction of its own.
mpv --loop=inf --no-osc --no-osd-bar --no-input-default-bindings \
    --geometry="$MPV_GEOM" --autofit="${MPV_GEOM%%+*}" \
    "$clip" >/dev/null 2>&1 &
mpv_pid=$!

# `-e` execs its argument directly, with no shell, so a shell one-liner here
# would be treated as the name of a program and xterm would exit immediately.
# Position only in `-geometry`; the size is driven through xdotool in pixels.
xterm -title "$TERM_TITLE" -geometry "+${TERM_HOME[0]}+${TERM_HOME[1]}" \
    -e sleep infinity >/dev/null 2>&1 &
term_pid=$!
xterm_wait=$(dur 6)

# Let both map and settle. Startup is whole-output damage by construction — every
# window maps and paints — so it must not leak into a measured phase.
sleep "$xterm_wait"
mark launched

# Poll rather than assume: a slow first map should not read as a missing WM.
win=
for _ in $(seq 20); do
    win=$(xdotool search --name "$TERM_TITLE" 2>/dev/null | head -1)
    [ -n "$win" ] && break
    sleep 0.5
done
if [ -z "$win" ]; then
    echo "damage-workload: xterm window never appeared." >&2
    echo "  xterm alive? $(kill -0 "$term_pid" 2>/dev/null && echo yes || echo NO)" >&2
    echo "  mpv alive?   $(kill -0 "$mpv_pid" 2>/dev/null && echo yes || echo NO)" >&2
    echo "  windows seen: $(xdotool search --name . 2>/dev/null | wc -l)" >&2
    exit 1
fi
mark found-window

# ── phase 1: settle ──────────────────────────────────────────────────
# Not measured. Anything still repainting from startup lands here.
mark settle
sleep "$(dur 6)"

# ── phase 2: idle ────────────────────────────────────────────────────
# Content damage only: mpv playing, nothing moving. This is the baseline the
# other phases are read against, and the phase step 4 alone already improves.
mark idle
sleep "$(dur 20)"

# ── phase 3: drag ────────────────────────────────────────────────────
# Structural damage: the window changes position and nothing else. Under the
# whole-output hammer every one of these is a full-screen repaint; under the
# scene diff it is the old rect union the new one.
mark drag
end=$(( $(date +%s) + $(printf '%.0f' "$(dur 20)") ))
while [ "$(date +%s)" -lt "$end" ]; do
    xdotool windowmove "$win" "${TERM_AWAY[0]}" "${TERM_AWAY[1]}"
    sleep 0.25
    xdotool windowmove "$win" "${TERM_HOME[0]}" "${TERM_HOME[1]}"
    sleep 0.25
done

# ── phase 4: idle again ──────────────────────────────────────────────
# A second idle stretch, so drift over the run is visible rather than being
# mistaken for an effect of the phase before it.
mark idle2
sleep "$(dur 20)"

# ── phase 5: resize ──────────────────────────────────────────────────
# Structural damage of a different shape: geometry changes reallocate window
# storage, which is a resample as well as a move.
mark resize
end=$(( $(date +%s) + $(printf '%.0f' "$(dur 20)") ))
while [ "$(date +%s)" -lt "$end" ]; do
    xdotool windowsize "$win" "${TERM_LARGE[0]}" "${TERM_LARGE[1]}"
    sleep 0.4
    xdotool windowsize "$win" "${TERM_SMALL[0]}" "${TERM_SMALL[1]}"
    sleep 0.4
done

# ── phase 6: restack ─────────────────────────────────────────────────
# Raise and lower, so stacking order changes with no geometry change at all —
# the one case the scene diff detects by rank rather than by region.
mark restack
end=$(( $(date +%s) + $(printf '%.0f' "$(dur 12)") ))
while [ "$(date +%s)" -lt "$end" ]; do
    xdotool windowraise "$win"
    sleep 0.3
    xdotool windowlower "$win"
    sleep 0.3
done

mark idle3
sleep "$(dur 10)"

mark done
