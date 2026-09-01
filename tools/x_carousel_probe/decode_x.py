# -*- coding: utf-8 -*-
# Read the gap X inserts between carousel images, in ORIGINAL image pixels.
#
#   python decode_x.py <screenshot.png> [--tiles 2|3|4] [--y ROW]
#
# The screenshot must include the coloured band (the middle ~70% of the image
# height). The white ruler bands at the top and bottom carry no code.
#
# Output is ASCII only so it stays readable in a CP932 PowerShell console.

import argparse
import statistics
import sys

from PIL import Image

TILE_W = 1536
B_CONST = 96


def decode(px, w_total):
    coarse = px[0] / 255 * (w_total - 1)
    g = px[1]
    return round((coarse - g) / 256) * 256 + g


def classify_row(row):
    n = len(row)
    kinds = []
    for x, px in enumerate(row):
        r, g, b = px[0], px[1], px[2]
        if r > 200 and g < 70 and b > 200:
            kinds.append("frame")  # magenta tile border
            continue
        lo, hi = max(0, x - 2), min(n, x + 3)
        window = [row[i][1] for i in range(lo, hi)]
        varies = max(window) - min(window) >= 2
        kinds.append("code" if abs(b - B_CONST) <= 12 and varies else "other")
    return kinds


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("image")
    ap.add_argument("--tiles", type=int, default=2, choices=(2, 3, 4))
    ap.add_argument("--y", type=int, default=None)
    args = ap.parse_args()

    im = Image.open(args.image).convert("RGB")
    w, h = im.size
    y = args.y if args.y is not None else h // 2
    w_total = TILE_W * args.tiles
    row = [im.getpixel((x, y)) for x in range(w)]
    kinds = classify_row(row)
    if "code" not in kinds:
        sys.exit("no coloured band on row y=%d; pick another row with --y" % y)

    runs = []
    for x, k in enumerate(kinds):
        if runs and runs[-1][0] == k:
            runs[-1][2] = x
        else:
            runs.append([k, x, x])

    slopes = []
    for kind, x0, x1 in runs:
        if kind != "code" or x1 - x0 < 40:
            continue
        a = decode(row[x0 + 5], w_total)
        b = decode(row[x1 - 5], w_total)
        slopes.append((b - a) / max(x1 - 5 - (x0 + 5), 1))
    if not slopes:
        sys.exit("coloured band too short to measure scale; capture a wider area")
    scale = statistics.median(slopes)

    print("row y=%d  tiles=%d  combined width=%dpx" % (y, args.tiles, w_total))
    print("scale: 1 screen px = %.3f source px  (one tile drawn at %.0f px wide)"
          % (scale, TILE_W / scale))
    print("")
    gaps = []
    for i, (kind, x0, x1) in enumerate(runs):
        width = x1 - x0 + 1
        if kind == "code":
            a = decode(row[x0 + 2], w_total)
            b = decode(row[x1 - 2], w_total)
            print("  image   screen %5d..%5d (%4dpx)   source x %5d..%5d" % (x0, x1, width, a, b))
        elif kind == "frame":
            print("  border  screen %5d..%5d (%4dpx)" % (x0, x1, width))
        elif width >= 2:
            inner = 0 < i < len(runs) - 1
            src = width * scale
            tag = "GAP " if inner else "edge"
            print("  %s    screen %5d..%5d (%4dpx)   source %7.1f px   = %.2f %% of one tile"
                  % (tag, x0, x1, width, src, src / TILE_W * 100))
            if inner:
                gaps.append(src / TILE_W * 100)
    print("")
    if gaps:
        print("=> cut %.2f %% of a tile width at each seam (median of %d gap(s))"
              % (statistics.median(gaps), len(gaps)))
    else:
        print("=> no gap found between two images; capture a shot that shows a seam")


if __name__ == "__main__":
    main()
