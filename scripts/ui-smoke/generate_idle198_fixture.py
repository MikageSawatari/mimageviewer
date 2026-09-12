#!/usr/bin/env python3
"""Generate the four wide images used by the idle-upgrade convergence smoke."""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path


WIDTH = 884
HEIGHT = 444
COUNT = 4


def load_page_turn_generator():
    source = Path(__file__).resolve().parents[1] / "page-turn" / "generate_fixture.py"
    spec = importlib.util.spec_from_file_location("miv_idle198_fixture", source)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load fixture generator: {source}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    image_generator = load_page_turn_generator()
    args.output.mkdir(parents=True, exist_ok=True)
    existing = list(args.output.iterdir())
    if existing:
        raise RuntimeError(f"refusing non-empty fixture output: {args.output}")
    for page in range(1, COUNT + 1):
        (args.output / f"idle198-{page:03d}.png").write_bytes(
            image_generator.make_png(WIDTH, HEIGHT, page)
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
