#!/usr/bin/env python3
"""Join a phased workload's phase markers against yserver's render telemetry.

Reports per-phase medians, so one run yields several comparable populations
instead of one blended average. Two branches run against
`tools/damage-workload.sh` can then be compared phase by phase, which is what
makes a before/after on this campaign mean anything: the 2026-09-02 step-2
attempt compared whole sessions and settled nothing, because content load
dominates `damage_fraction` and differs between any two human sessions.

Usage:
    tools/damage-phases.py <yserver.log> <phases-file> [<yserver.log> <phases>]

With two pairs it prints a side-by-side diff, `before` then `after`.

ARCHIVING A RUN: keep whole lines —

    grep "render_telemetry:" yserver-hw-awesome.log > <dest>-telemetry.log
    cp damage-phases.log <dest>-phases.log

NOT `grep -o "render_telemetry:.*"`, which strips the `[timestamp]` prefix this
tool joins on and leaves the archive unusable for comparison. That was learned
by losing a baseline to it.
"""

from __future__ import annotations

import re
import statistics
import sys
from datetime import datetime, timezone

# Counters worth reading per phase. `damage_fraction` is what was actually
# rasterised; `damage_region_fraction` is what was asked for, so the gap between
# them is bounding-box waste; `structural_fraction` isolates the scene diff's own
# contribution, which distinguishes "the diff is churning" from "a mutator still
# posts whole-output damage".
FIELDS = [
    ("damage_fraction", "painted", "{:.3f}"),
    ("damage_region_fraction", "region", "{:.3f}"),
    ("structural_fraction", "structural", "{:.3f}"),
    ("overdraw", "overdraw", "{:.2f}"),
    ("avg_gpu_render_ns", "gpu_us", "{:.1f}"),
    ("avg_compose_cb_record_ns", "cb_us", "{:.1f}"),
    ("composite_submits/s", "composes/s", "{:.0f}"),
    ("full_redraw_fallback/s", "full/s", "{:.1f}"),
    ("clipped_repaint/s", "clipped/s", "{:.1f}"),
    ("paint_submits/s", "paint/s", "{:.0f}"),
]
SCALE = {"avg_gpu_render_ns": 1e-3, "avg_compose_cb_record_ns": 1e-3}
TS = re.compile(r"\[(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)Z")


def parse_time(s: str) -> datetime:
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=timezone.utc)


def read_phases(path: str) -> list[tuple[datetime, str]]:
    out = []
    for line in open(path, encoding="utf-8"):
        parts = line.split()
        if len(parts) == 2:
            out.append((parse_time(parts[0].rstrip("Z")), parts[1]))
    return out


def read_telemetry(path: str) -> list[tuple[datetime, dict[str, float]]]:
    out = []
    for line in open(path, errors="ignore"):
        if "render_telemetry: paint" not in line:
            continue
        m = TS.search(line)
        if not m:
            continue
        row = {}
        for key, _, _ in FIELDS:
            v = re.search(re.escape(key) + r"=([0-9.]+)", line)
            if v:
                row[key] = float(v.group(1)) * SCALE.get(key, 1.0)
        out.append((parse_time(m.group(1)), row))
    return out


def bucket_by_phase(log: str, phases_file: str) -> dict[str, list[dict[str, float]]]:
    phases = read_phases(phases_file)
    if not phases:
        sys.exit(f"no phase markers in {phases_file}")
    rows = read_telemetry(log)
    if not rows:
        sys.exit(f"no render_telemetry lines in {log} — is YSERVER_LOOP_TELEMETRY set?")

    out: dict[str, list[dict[str, float]]] = {}
    for when, row in rows:
        # The phase in force at this sample: the last marker at or before it.
        # A sample is attributed to a phase only if it is wholly inside it —
        # the first second after a marker straddles the boundary and is dropped,
        # because a bucket is a one-second rollup.
        current = None
        for i, (start, name) in enumerate(phases):
            nxt = phases[i + 1][0] if i + 1 < len(phases) else None
            if when > start and (nxt is None or when <= nxt):
                current = name
                break
        # Non-phase markers: liveness checkpoints and the settle window, which
        # exists precisely to absorb startup's whole-output damage.
        if current in (None, "clip-ok", "launched", "found-window", "settle",
                       "shutdown", "done"):
            continue
        out.setdefault(current, []).append(row)
    return out


def summarise(name: str, buckets: dict[str, list[dict[str, float]]]) -> None:
    print(f"\n=== {name} ===")
    head = f"{'phase':10s} {'n':>4}" + "".join(f" {lbl:>11s}" for _, lbl, _ in FIELDS)
    print(head)
    for phase, rows in buckets.items():
        cells = []
        for key, _, fmt in FIELDS:
            vals = [r[key] for r in rows if key in r]
            cells.append(fmt.format(statistics.median(vals)) if vals else "-")
        print(f"{phase:10s} {len(rows):4d}" + "".join(f" {c:>11s}" for c in cells))


def compare(before: dict, after: dict) -> None:
    print("\n=== before -> after, per phase ===")
    for phase in [p for p in before if p in after]:
        print(f"\n  {phase}")
        for key, lbl, fmt in FIELDS:
            b = [r[key] for r in before[phase] if key in r]
            a = [r[key] for r in after[phase] if key in r]
            if not b or not a:
                continue
            mb, ma = statistics.median(b), statistics.median(a)
            delta = f"{100 * (ma - mb) / mb:+7.1f}%" if mb else "      -"
            print(f"    {lbl:12s} {fmt.format(mb):>10s} -> {fmt.format(ma):>10s}  {delta}")


def main() -> None:
    args = sys.argv[1:]
    if len(args) == 2:
        summarise(args[0], bucket_by_phase(args[0], args[1]))
    elif len(args) == 4:
        before = bucket_by_phase(args[0], args[1])
        after = bucket_by_phase(args[2], args[3])
        summarise(f"before: {args[0]}", before)
        summarise(f"after: {args[2]}", after)
        compare(before, after)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
