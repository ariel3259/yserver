#!/usr/bin/env python3
"""Report axis-aligned bounding boxes of same-colour regions in a PPM or PAM.

Written for scanout dumps: `yserver-scanout-*.ppm` is a P6 the render
backend writes on Ctrl+Alt+Enter. Eyeballing a resized PNG has produced
several wrong conclusions in this codebase, so measure instead.

    tools/ppm-regions.py FILE [--colour RRGGBB] [--min-area N]
    tools/ppm-regions.py FILE --at X,Y          # colour of one pixel
    tools/ppm-regions.py FILE --histogram N     # N most common colours
"""

import argparse
import sys
from collections import Counter, deque


def read_pam(path, data):
    """Netpbm PAM (P7), which is what the per-drawable storage dump writes.

    Header is one `KEY VALUE` per line up to ENDHDR. Only RGB_ALPHA /
    MAXVAL 255 appears in practice; the alpha channel is dropped so a PAM
    and a PPM of the same surface compare equal.
    """
    header, _, body = data.partition(b"ENDHDR\n")
    keys = {}
    for line in header.splitlines()[1:]:
        parts = line.split()
        if len(parts) == 2:
            keys[parts[0]] = parts[1]
    width, height = int(keys[b"WIDTH"]), int(keys[b"HEIGHT"])
    depth, maxval = int(keys[b"DEPTH"]), int(keys[b"MAXVAL"])
    if maxval != 255 or depth not in (3, 4):
        sys.exit(f"ppm-regions: {path}: unsupported PAM depth {depth} maxval {maxval}")
    expected = width * height * depth
    if len(body) < expected:
        sys.exit(f"ppm-regions: {path} truncated: {len(body)} of {expected} bytes")
    if depth == 3:
        return width, height, body[:expected]
    rgb = bytearray(width * height * 3)
    for i in range(width * height):
        rgb[i * 3 : i * 3 + 3] = body[i * 4 : i * 4 + 3]
    return width, height, bytes(rgb)


def read_ppm(path):
    with open(path, "rb") as handle:
        data = handle.read()
    if data.startswith(b"P7"):
        return read_pam(path, data)
    fields, offset = [], 0
    while len(fields) < 4:
        while offset < len(data) and data[offset : offset + 1].isspace():
            offset += 1
        if data[offset : offset + 1] == b"#":
            while data[offset : offset + 1] not in (b"\n", b""):
                offset += 1
            continue
        start = offset
        while offset < len(data) and not data[offset : offset + 1].isspace():
            offset += 1
        fields.append(data[start:offset])
    offset += 1
    magic, width, height, maxval = fields
    if magic != b"P6" or maxval != b"255":
        sys.exit(f"ppm-regions: {path} is not an 8-bit P6 ({magic!r} {maxval!r})")
    width, height = int(width), int(height)
    expected = width * height * 3
    body = data[offset : offset + expected]
    if len(body) != expected:
        sys.exit(f"ppm-regions: {path} truncated: {len(body)} of {expected} bytes")
    return width, height, body


def pixel(body, width, x, y):
    i = (y * width + x) * 3
    return body[i], body[i + 1], body[i + 2]


def regions(body, width, height, colour, min_area):
    seen = bytearray(width * height)
    found = []
    for y0 in range(height):
        for x0 in range(width):
            if seen[y0 * width + x0] or pixel(body, width, x0, y0) != colour:
                continue
            queue = deque([(x0, y0)])
            seen[y0 * width + x0] = 1
            minx = maxx = x0
            miny = maxy = y0
            area = 0
            while queue:
                x, y = queue.popleft()
                area += 1
                minx, maxx = min(minx, x), max(maxx, x)
                miny, maxy = min(miny, y), max(maxy, y)
                for nx, ny in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
                    if 0 <= nx < width and 0 <= ny < height and not seen[ny * width + nx]:
                        if pixel(body, width, nx, ny) == colour:
                            seen[ny * width + nx] = 1
                            queue.append((nx, ny))
            if area >= min_area:
                found.append((area, minx, miny, maxx - minx + 1, maxy - miny + 1))
    found.sort(reverse=True)
    return found


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("file")
    parser.add_argument("--colour", default="ffffff")
    parser.add_argument("--min-area", type=int, default=200)
    parser.add_argument("--at")
    parser.add_argument("--histogram", type=int)
    args = parser.parse_args()

    width, height, body = read_ppm(args.file)
    print(f"{args.file}: {width}x{height}")

    if args.at:
        x, y = (int(part) for part in args.at.split(","))
        r, g, b = pixel(body, width, x, y)
        print(f"({x},{y}) = {r:02x}{g:02x}{b:02x}")
        return

    if args.histogram:
        counts = Counter(
            tuple(body[i : i + 3]) for i in range(0, len(body), 3)
        )
        for (r, g, b), n in counts.most_common(args.histogram):
            print(f"  {r:02x}{g:02x}{b:02x}  {n:>9}  {100 * n / (width * height):5.2f}%")
        return

    colour = tuple(int(args.colour[i : i + 2], 16) for i in (0, 2, 4))
    found = regions(body, width, height, colour, args.min_area)
    print(f"regions of {args.colour} with area >= {args.min_area}: {len(found)}")
    for area, x, y, w, h in found:
        fill = 100 * area / (w * h)
        print(f"  x={x} y={y} {w}x{h}  area={area} ({fill:.1f}% of bbox)")


if __name__ == "__main__":
    main()
