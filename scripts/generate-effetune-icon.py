#!/usr/bin/env python3
"""Generate the EffeTune host icon from assets/icon.ico (requires Pillow).

Run: python scripts/generate-effetune-icon.py
Check committed output: python scripts/generate-effetune-icon.py --check
Optional preview: add --preview target/effetune-icon-preview.png

The note is geometry, never a font/emoji glyph. Each requested size uses the
matching base ICO frame when available, with a supersampled bottom-right badge.
The ICO uses uncompressed 32-bit DIBs, avoiding PNG encoder/compression variation.
The original mIV icon is read only. No app is launched.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import struct

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parents[1]
SIZES = (16, 24, 32, 48, 64, 256)
SUPERSAMPLE = 8


def render_frames(source: Path) -> list[Image.Image]:
    frames = []
    with Image.open(source) as base:
        if base.format != "ICO":
            raise ValueError("Base icon must be an ICO file")
        available = sorted(base.ico.sizes())
        for size in SIZES:
            selected = next((s for s in available if s[0] >= size and s[1] >= size), available[-1])
            frame = base.ico.getimage(selected).convert("RGBA")
            if frame.size != (size, size):
                frame = frame.resize((size, size), Image.Resampling.LANCZOS)
            scale = size * SUPERSAMPLE
            overlay = Image.new("RGBA", (scale, scale))
            draw = ImageDraw.Draw(overlay)

            def points(coords):
                return [(round(x * scale), round(y * scale)) for x, y in coords]

            # A dark badge separates the white note from the mIV lettering.
            draw.ellipse(points(((0.565, 0.565), (0.990, 0.990))), fill="#18213b")
            # One eighth note: filled head, stem and flag. The narrowest stroke
            # remains at least one pixel wide at 16px after downsampling.
            draw.ellipse(points(((0.639, 0.818), (0.789, 0.919))), fill="white")
            draw.rectangle(points(((0.747, 0.644), (0.813, 0.866))), fill="white")
            draw.polygon(points(((0.747, 0.644), (0.830, 0.663),
                                 (0.893, 0.720), (0.898, 0.782),
                                 (0.851, 0.748), (0.803, 0.729),
                                 (0.780, 0.724))), fill="white")
            overlay = overlay.resize((size, size), Image.Resampling.LANCZOS)
            frames.append(Image.alpha_composite(frame, overlay))
    return frames


def dib(frame: Image.Image) -> bytes:
    """Bottom-up BGRA XOR bitmap plus DWORD-aligned 1-bit AND mask."""
    size = frame.width
    pixels = frame.tobytes("raw", "BGRA", 0, -1)
    mask_stride = ((size + 31) // 32) * 4
    mask = bytearray(mask_stride * size)
    for y in range(size):
        for x in range(size):
            if frame.getpixel((x, size - 1 - y))[3] < 128:
                mask[y * mask_stride + x // 8] |= 0x80 >> (x % 8)
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32,
                         0, len(pixels), 0, 0, 0, 0)
    return header + pixels + mask


def encode_ico(frames: list[Image.Image]) -> bytes:
    entries = bytearray()
    payloads = []
    offset = 6 + 16 * len(frames)
    for frame in frames:
        payload = dib(frame)
        dimension = frame.width % 256  # ICO encodes 256 as zero.
        entries.extend(struct.pack("<BBBBHHII", dimension, dimension, 0, 0,
                                   1, 32, len(payload), offset))
        payloads.append(payload)
        offset += len(payload)
    return struct.pack("<HHH", 0, 1, len(frames)) + bytes(entries) + b"".join(payloads)


def preview(frames: list[Image.Image], destination: Path) -> None:
    # Actual-size frames and nearest-neighbor enlargements expose the 16px shape.
    sheet = Image.new("RGB", (824, 320), "#e7e7eb")
    draw = ImageDraw.Draw(sheet)
    x = 16
    for frame in frames:
        draw.text((x, 8), f"{frame.width}px", fill="#18213b")
        sheet.paste(frame, (x, 30), frame)
        if frame.width <= 64:
            enlarged = frame.resize((96, 96), Image.Resampling.NEAREST)
            sheet.paste(enlarged, (x, 300 - 96), enlarged)
        x += max(104, frame.width + 16)
    destination.parent.mkdir(parents=True, exist_ok=True)
    sheet.save(destination)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=ROOT / "assets/icon.ico")
    parser.add_argument("--output", type=Path, default=ROOT / "assets/effetune.ico")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--preview", type=Path)
    args = parser.parse_args()
    if args.source.resolve() == args.output.resolve():
        parser.error("Output must differ from the base icon")
    frames = render_frames(args.source)
    data = encode_ico(frames)
    if args.preview:
        preview(frames, args.preview)
    if args.check:
        if not args.output.is_file() or args.output.read_bytes() != data:
            parser.exit(1, f"Generated icon differs: {args.output}\n")
        print(f"Icon is current: {args.output}")
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_bytes(data)
        print(f"Generated {args.output}: {len(data)} bytes; sizes {SIZES}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
