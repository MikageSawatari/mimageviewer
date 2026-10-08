#!/usr/bin/env python3
"""Create folder and ZIP books with real sidecar import entries."""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path
from zipfile import ZipFile, ZIP_STORED


def image_generator():
    source = Path(__file__).resolve().parents[1] / "page-turn" / "generate_fixture.py"
    spec = importlib.util.spec_from_file_location("miv_stills_png", source)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load image generator: {source}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def sidecar(path: Path, key: str) -> None:
    adjust = {
        "brightness": 12.0,
        "contrast": 0.0,
        "gamma": 1.0,
        "saturation": 0.0,
        "temperature": 0.0,
        "black_point": 0,
        "white_point": 255,
        "midtone": 1.0,
        "auto_mode": None,
        "upscale_model": None,
    }
    path.write_text(
        json.dumps({"version": 1, "items": {key: {"adjust": adjust}}}),
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if list(args.output.iterdir()):
        raise RuntimeError(f"refusing non-empty fixture output: {args.output}")
    make_png = image_generator().make_png
    folder = args.output / "a-folder"
    folder.mkdir()
    zip_folder = args.output / "z-zip"
    zip_folder.mkdir()
    zip_path = zip_folder / "book.zip"
    with ZipFile(zip_path, "w", ZIP_STORED) as archive:
        for page in range(3):
            name = f"page-{page:03d}.png"
            folder.joinpath(name).write_bytes(make_png(600, 900, page + 1))
            archive.writestr(name, make_png(600, 900, page + 11))
    sidecar(folder / "mimageviewer.dat", "page-000.png")
    sidecar(zip_folder / "mimageviewer.dat", "book.zip::page-000.png")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
