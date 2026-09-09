#!/usr/bin/env python3
"""Generate the PDF sibling and mixed-aspect image book used by StillStripDrag."""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path


ASPECT_SIZES = (
    (64, 96),    # portrait
    (160, 90),   # landscape
    (90, 90),    # square
    (40, 160),   # narrow portrait (exercises the strip's minimum-width bound)
    (120, 80),   # wide landscape
)


def load_page_turn_generator(name: str, filename: str):
    source = Path(__file__).resolve().parents[1] / "page-turn" / filename
    spec = importlib.util.spec_from_file_location(name, source)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load fixture generator: {source}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--count", type=int, default=40)
    args = parser.parse_args()
    if args.count < 40:
        parser.error("--count must be at least 40")

    image_generator = load_page_turn_generator(
        "miv_page_turn_fixture", "generate_fixture.py"
    )
    pdf_generator = load_page_turn_generator(
        "miv_pdf_fixture", "generate_pdf_fixture.py"
    )
    args.output.mkdir(parents=True, exist_ok=True)
    image_dir = args.output / "images"
    image_dir.mkdir(parents=True, exist_ok=True)
    for existing in image_dir.glob("*.png"):
        existing.unlink()
    for page in range(1, args.count + 1):
        width, height = ASPECT_SIZES[(page - 1) % len(ASPECT_SIZES)]
        (image_dir / f"{page:03d}.png").write_bytes(
            image_generator.make_png(width, height, page)
        )
    (args.output / "zzz-sibling.pdf").write_bytes(
        pdf_generator.build_pdf(0, 2, *pdf_generator.SIZES[0])
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
