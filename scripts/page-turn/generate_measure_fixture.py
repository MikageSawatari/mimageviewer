#!/usr/bin/env python3
"""Generate a page-sized JPEG book for the section 1.58 page-turn measurement.

This is not the self-test fixture. The self-test uses 64x96 PNGs on purpose so it
stays fast; this one is sized and encoded like a scanned book so the per-page
cost is the real one:

- JPEG, because real books are JPEG and mIV decodes them through turbojpeg with
  DCT scaling. A PNG fixture measures a decoder the real case never uses, and
  overstates decode by roughly 1.5x (measured 256ms vs 172ms at 24MP).
- Noise, because a flat image compresses and decodes unrealistically fast.
- Large by default, because the UI-thread cost scales with source megapixels:
  measured 13.9ms at 11.8MP and 28.3ms at 24MP per page.

What this fixture does NOT reproduce: colorize, LUTs and colour adjustment. Those
need a real book prepared once through `page-turn-smoke.ps1 -Setup`. See
docs/display-pipeline.md 2.5.3 for why they are treated differently.

Requires numpy and pillow (development-only dependency, not used by the app).
"""

from __future__ import annotations

import argparse
from pathlib import Path

import numpy as np
from PIL import Image


def make_page(width: int, height: int, page: int, rng: np.random.Generator) -> Image.Image:
    base = np.full((height, width), 235, dtype=np.uint8)
    band = (page * 23) % max(1, height - 400)
    base[band : band + 400, :] = 60
    noise = rng.integers(-18, 18, size=(height, width), dtype=np.int16)
    out = np.clip(base.astype(np.int16) + noise, 0, 255).astype(np.uint8)
    return Image.fromarray(np.dstack([out, out, out]), mode="RGB")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--count", type=int, default=30)
    parser.add_argument("--width", type=int, default=4000)
    parser.add_argument("--height", type=int, default=6000)
    parser.add_argument("--quality", type=int, default=88)
    args = parser.parse_args()
    if args.count < 20:
        parser.error("--count must be at least 20: a hold burns through pages quickly")

    args.output.mkdir(parents=True, exist_ok=True)
    for existing in args.output.glob("*.jpg"):
        existing.unlink()
    rng = np.random.default_rng(20260811)
    for page in range(1, args.count + 1):
        make_page(args.width, args.height, page, rng).save(
            args.output / f"{page:03d}.jpg", quality=args.quality, subsampling=0
        )
    megapixels = args.width * args.height / 1_000_000
    print(
        f"wrote {args.count} pages of {args.width}x{args.height} "
        f"({megapixels:.1f} MP) to {args.output}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
