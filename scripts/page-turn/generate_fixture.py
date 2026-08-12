#!/usr/bin/env python3
"""Generate a tiny deterministic image book for page-turn self-tests."""

from __future__ import annotations

import argparse
import struct
import zlib
from pathlib import Path


def png_chunk(kind: bytes, payload: bytes) -> bytes:
    body = kind + payload
    return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body))


def make_png(width: int, height: int, page: int) -> bytes:
    color = ((page * 37) % 220 + 24, (page * 71) % 220 + 24, (page * 103) % 220 + 24)
    rows = []
    for y in range(height):
        row = bytearray([0])
        for x in range(width):
            border = x < 3 or y < 3 or x >= width - 3 or y >= height - 3
            if border:
                row.extend((16, 16, 16))
            else:
                row.extend(color)
        rows.append(bytes(row))
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + png_chunk(b"IHDR", header)
        + png_chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
        + png_chunk(b"IEND", b"")
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    parser.add_argument("--count", type=int, default=12)
    args = parser.parse_args()
    if args.count < 10:
        parser.error("--count must be at least 10")

    args.output.mkdir(parents=True, exist_ok=True)
    for existing in args.output.glob("*.png"):
        existing.unlink()
    for page in range(1, args.count + 1):
        (args.output / f"{page:03d}.png").write_bytes(make_png(64, 96, page))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
