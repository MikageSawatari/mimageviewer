#!/usr/bin/env python3
"""Prepare the disposable image library for the GamepadBasic scenario."""

from __future__ import annotations

import argparse
import importlib.util
from pathlib import Path


WIDTH = 96
HEIGHT = 144
NAMES = ("01-red.png", "02-green.png", "03-blue.png")


def image_generator():
    source = Path(__file__).resolve().parents[1] / "page-turn" / "generate_fixture.py"
    spec = importlib.util.spec_from_file_location("miv_gamepad_basic_png", source)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load image generator: {source}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def prepare(output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise RuntimeError(f"refusing non-empty gamepad fixture output: {output}")
    make_png = image_generator().make_png
    sub = output / "library" / "sub"
    sub.mkdir(parents=True)
    for page, name in enumerate(NAMES, 1):
        (sub / name).write_bytes(make_png(WIDTH, HEIGHT, page))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    prepare(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
