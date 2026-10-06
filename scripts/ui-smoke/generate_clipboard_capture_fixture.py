#!/usr/bin/env python3
"""Prepare disposable ClipboardCapture files, without clipboard or UI access."""
import argparse
import struct
import zlib
from pathlib import Path


def fixture_png():
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    pixels = (b"\0" + bytes((30, 80, 200, 255)) * 200) * 200
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 200, 200, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b"")


def prepare(output):
    output.mkdir(parents=True, exist_ok=True)
    if list(output.iterdir()):
        raise RuntimeError("refusing non-empty clipboard fixture output")
    for name in ("manual", "source", "captures"):
        (output / name).mkdir()
    png = fixture_png()
    (output / "manual" / "seed.png").write_bytes(png)
    (output / "source" / "shell-copy.png").write_bytes(png)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    prepare(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
