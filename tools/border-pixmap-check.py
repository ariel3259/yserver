#!/usr/bin/env python3
"""Verify a tiled window border, pixel by pixel, against Xorg's tile phase.

Companion to `tools/vng-scenarios/border-pixmap.sh`. Checks EVERY ring pixel
rather than sampling, because the interesting failures are phase errors that a
handful of samples can miss (a tile aligned to the outer corner instead of the
content origin still looks like a plausible pattern).

The expected value comes from Xorg's source, not from prose:

    mi/miexpose.c:461   tile_x_off = pWin->drawable.x;      (content origin)
    dix/window.c:888    pWin->drawable.x = parent->drawable.x + x + bw;

CreateWindow's x,y is the OUTER corner, so a ring pixel at absolute (ax,ay)
samples tile ((ax - content_x) mod 64, (ay - content_y) mod 64). The client
paints the tile as a 4x4 grid of 16x16 cells, cell (col,row) coloured
R = col*64+32, G = row*64+32, B = 128, so the phase is readable off one pixel.

    tools/border-pixmap-check.py <scanout.ppm>

yserver's Ctrl+Alt+Enter dump is already a P6. For an Xorg capture convert the
scenario's `screen.png` with an EXPLICIT depth —

    magick screen.png -depth 8 screen.ppm

— because ImageMagick picks the maxval from the image's actual colour count,
and this frame has few enough colours that it writes `255`-less PPMs (maxval
15) that no 8-bit reader will take.

Exit code 0 if everything matches, 1 otherwise. Geometry defaults match the
client's constants; override with the flags if the client changes.
"""

import argparse
import importlib.util
import sys
from pathlib import Path

# `ppm-regions.py` is not an importable module name, so load it by path.
_spec = importlib.util.spec_from_file_location(
    "ppm_regions", Path(__file__).resolve().parent / "ppm-regions.py"
)
_ppm = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_ppm)


def expected_tile_colour(ax, ay, content_x, content_y, tile, cell):
    tx = (ax - content_x) % tile
    ty = (ay - content_y) % tile
    col, row = tx // cell, ty // cell
    return (col * 64 + 32, row * 64 + 32, 128)


def ring_pixels(outer, inner):
    ox, oy, ow, oh = outer
    ix, iy, iw, ih = inner
    for y in range(oy, oy + oh):
        for x in range(ox, ox + ow):
            if ix <= x < ix + iw and iy <= y < iy + ih:
                continue
            yield x, y


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("file")
    parser.add_argument("--tiled-x", type=int, default=100)
    parser.add_argument("--tiled-y", type=int, default=100)
    parser.add_argument("--solid-x", type=int, default=500)
    parser.add_argument("--solid-y", type=int, default=100)
    parser.add_argument("--w", type=int, default=200)
    parser.add_argument("--h", type=int, default=100)
    parser.add_argument("--bw", type=int, default=16)
    parser.add_argument("--tile", type=int, default=64)
    parser.add_argument("--cell", type=int, default=16)
    parser.add_argument("--solid-colour", default="ff00ff")
    parser.add_argument(
        "--max-report", type=int, default=8, help="mismatching pixels to print"
    )
    args = parser.parse_args()

    width, height, body = _ppm.read_ppm(args.file)
    print(f"{args.file}: {width}x{height}")
    failures = []

    def pixel(x, y):
        return _ppm.pixel(body, width, x, y)

    def check(name, outer, inner, want_at):
        total = bad = 0
        for x, y in ring_pixels(outer, inner):
            if not (0 <= x < width and 0 <= y < height):
                continue
            total += 1
            got, exp = pixel(x, y), want_at(x, y)
            if got != exp:
                bad += 1
                if len(failures) < args.max_report:
                    failures.append(
                        f"  {name} ({x},{y}) got "
                        f"{got[0]:02x}{got[1]:02x}{got[2]:02x} "
                        f"want {exp[0]:02x}{exp[1]:02x}{exp[2]:02x}"
                    )
        verdict = "OK" if bad == 0 else f"{bad} WRONG"
        print(f"{name}: {total} ring px checked — {verdict}")
        return bad

    ow = args.w + 2 * args.bw
    oh = args.h + 2 * args.bw

    tiled_outer = (args.tiled_x, args.tiled_y, ow, oh)
    tiled_inner = (args.tiled_x + args.bw, args.tiled_y + args.bw, args.w, args.h)
    content_x, content_y = tiled_inner[0], tiled_inner[1]
    bad = check(
        "tiled ring",
        tiled_outer,
        tiled_inner,
        lambda x, y: expected_tile_colour(
            x, y, content_x, content_y, args.tile, args.cell
        ),
    )

    # The corner that distinguishes the two candidate phases outright.
    corner = pixel(args.tiled_x, args.tiled_y)
    want_content = expected_tile_colour(
        args.tiled_x, args.tiled_y, content_x, content_y, args.tile, args.cell
    )
    want_outer = expected_tile_colour(
        args.tiled_x, args.tiled_y, args.tiled_x, args.tiled_y, args.tile, args.cell
    )
    got_hex = f"{corner[0]:02x}{corner[1]:02x}{corner[2]:02x}"
    print(
        f"tile phase at the outer corner: {got_hex} — "
        f"content-aligned would be {want_content[0]:02x}"
        f"{want_content[1]:02x}{want_content[2]:02x}, "
        f"outer-aligned {want_outer[0]:02x}{want_outer[1]:02x}{want_outer[2]:02x}"
    )
    if corner == want_outer and want_outer != want_content:
        print("  ^ ALIGNED TO THE OUTER CORNER — wrong phase (miexpose.c:461)")

    solid = tuple(int(args.solid_colour[i : i + 2], 16) for i in (0, 2, 4))
    bad += check(
        "solid ring",
        (args.solid_x, args.solid_y, ow, oh),
        (args.solid_x + args.bw, args.solid_y + args.bw, args.w, args.h),
        lambda x, y: solid,
    )

    for line in failures:
        print(line)
    if bad:
        print(f"FAIL: {bad} wrong ring pixels")
        return 1
    print("PASS: every ring pixel matches")
    return 0


if __name__ == "__main__":
    sys.exit(main())
