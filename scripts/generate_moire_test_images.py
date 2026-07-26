#!/usr/bin/env python3
"""Generate deterministic images for checking minification moire and mipmaps.

The output is intentionally ignored by git (``testimage/`` is a local fixture
tree).  Pillow and NumPy are required.  Run from the repository root:

    python scripts/generate_moire_test_images.py
"""

from __future__ import annotations

import argparse
import hashlib
import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont


BACKGROUND = 232
INK = 12
HEADER_HEIGHT = 128


def load_font(size: int) -> ImageFont.ImageFont:
    try:
        return ImageFont.truetype("DejaVuSans.ttf", size)
    except OSError:
        return ImageFont.load_default()


TITLE_FONT = load_font(42)
LABEL_FONT = load_font(25)
SMALL_FONT = load_font(20)


def new_canvas(width: int, height: int, title: str) -> Image.Image:
    image = Image.new("L", (width, height), BACKGROUND)
    draw = ImageDraw.Draw(image)
    draw.rectangle((0, 0, width, HEADER_HEIGHT - 1), fill=18)
    draw.text((32, 31), title, fill=245, font=TITLE_FONT)
    return image


def panel_layout(width: int, height: int, columns: int, rows: int):
    margin = 32
    gap = 18
    top = HEADER_HEIGHT + margin
    panel_w = (width - 2 * margin - (columns - 1) * gap) // columns
    panel_h = (height - top - margin - (rows - 1) * gap) // rows
    for row in range(rows):
        for column in range(columns):
            left = margin + column * (panel_w + gap)
            upper = top + row * (panel_h + gap)
            yield left, upper, panel_w, panel_h


def paste_labeled_panel(
    image: Image.Image,
    box: tuple[int, int, int, int],
    label: str,
    pixels: np.ndarray,
) -> None:
    left, upper, width, height = box
    label_h = 42
    draw = ImageDraw.Draw(image)
    draw.rectangle((left, upper, left + width - 1, upper + height - 1), fill=250, outline=64, width=2)
    draw.rectangle((left + 2, upper + 2, left + width - 3, upper + label_h), fill=212)
    draw.text((left + 10, upper + 8), label, fill=20, font=SMALL_FONT)
    pattern = Image.fromarray(pixels.astype(np.uint8), mode="L")
    image.paste(pattern, (left + 2, upper + label_h + 1))


def pattern_shape(box: tuple[int, int, int, int]) -> tuple[int, int]:
    _, _, width, height = box
    return height - 45, width - 4


def checkerboard(height: int, width: int, cell: int) -> np.ndarray:
    x = np.arange(width, dtype=np.int32)[None, :]
    y = np.arange(height, dtype=np.int32)[:, None]
    return np.where(((x // cell + y // cell) & 1) == 0, 248, 8)


def line_screen(
    height: int,
    width: int,
    period: float,
    angle_degrees: float,
    duty: float = 0.5,
) -> np.ndarray:
    x = np.arange(width, dtype=np.float32)[None, :]
    y = np.arange(height, dtype=np.float32)[:, None]
    angle = math.radians(angle_degrees)
    coordinate = x * math.cos(angle) + y * math.sin(angle)
    phase = np.mod(coordinate, period)
    return np.where(phase < period * duty, 12, 246)


def dot_screen(
    height: int,
    width: int,
    pitch: float,
    angle_degrees: float,
    coverage: float,
) -> np.ndarray:
    x = np.arange(width, dtype=np.float32)[None, :]
    y = np.arange(height, dtype=np.float32)[:, None]
    angle = math.radians(angle_degrees)
    cosine = math.cos(angle)
    sine = math.sin(angle)
    u = np.mod(x * cosine + y * sine + pitch / 2.0, pitch) - pitch / 2.0
    v = np.mod(-x * sine + y * cosine + pitch / 2.0, pitch) - pitch / 2.0
    radius = pitch * math.sqrt(coverage / math.pi)
    return np.where(u * u + v * v <= radius * radius, 8, 248)


def crossed_screen(
    height: int,
    width: int,
    period: float,
    base_angle: float,
    angle_delta: float,
    period_delta: float,
) -> np.ndarray:
    first = line_screen(height, width, period, base_angle, 0.20) < 128
    second = line_screen(
        height,
        width,
        period * (1.0 + period_delta),
        base_angle + angle_delta,
        0.20,
    ) < 128
    return np.where(first | second, 8, 248)


def chirp(
    height: int,
    width: int,
    direction: str,
    binary: bool,
) -> np.ndarray:
    x = np.arange(width, dtype=np.float32)[None, :]
    y = np.arange(height, dtype=np.float32)[:, None]
    if direction == "horizontal":
        coordinate = np.broadcast_to(x, (height, width))
        extent = width
    elif direction == "vertical":
        coordinate = np.broadcast_to(y, (height, width))
        extent = height
    elif direction == "diagonal":
        coordinate = (x + y) / math.sqrt(2.0)
        extent = (width + height) / math.sqrt(2.0)
    else:
        raise ValueError(direction)

    f0 = 1.0 / 256.0
    f1 = 0.5
    slope = (f1 - f0) / max(extent - 1.0, 1.0)
    phase = 2.0 * math.pi * (f0 * coordinate + 0.5 * slope * coordinate * coordinate)
    wave = np.sin(phase)
    if binary:
        return np.where(wave >= 0.0, 246, 10)
    return np.clip(127.5 + 122.5 * wave, 0, 255)


def radial_zone_plate(height: int, width: int, binary: bool) -> np.ndarray:
    x = np.arange(width, dtype=np.float32)[None, :] - (width - 1.0) / 2.0
    y = np.arange(height, dtype=np.float32)[:, None] - (height - 1.0) / 2.0
    radius = np.sqrt(x * x + y * y)
    radius_max = max(min(width, height) / 2.0 - 1.0, 1.0)
    # The instantaneous frequency reaches Nyquist at the outer edge.
    phase = 2.0 * math.pi * 0.25 * radius * radius / radius_max
    wave = np.sin(phase)
    if binary:
        return np.where(wave >= 0.0, 246, 10)
    return np.clip(127.5 + 122.5 * wave, 0, 255)


def angular_spokes(height: int, width: int, spokes: int = 720) -> np.ndarray:
    x = np.arange(width, dtype=np.float32)[None, :] - (width - 1.0) / 2.0
    y = np.arange(height, dtype=np.float32)[:, None] - (height - 1.0) / 2.0
    angle = np.arctan2(y, x)
    wave = np.sin(angle * (spokes / 2.0))
    return np.where(wave >= 0.0, 246, 10)


def generate_checkerboards(size: int) -> Image.Image:
    image = new_canvas(size, size, "01  Checkerboards - cell size sweep")
    cells = [1, 2, 3, 4, 5, 6, 8, 10, 12, 16, 24, 32, 48, 64, 96, 128]
    for box, cell in zip(panel_layout(size, size, 4, 4), cells, strict=True):
        height, width = pattern_shape(box)
        paste_labeled_panel(image, box, f"cell={cell}px", checkerboard(height, width, cell))
    return image


def generate_parallel_lines(size: int) -> Image.Image:
    image = new_canvas(size, size, "02  Parallel lines - period and angle sweep")
    periods = [2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 20, 24, 32, 48, 64, 96]
    angles = [0, 5, 15, 30, 45, 60, 75, 89] * 2
    for box, period, angle in zip(panel_layout(size, size, 4, 4), periods, angles, strict=True):
        height, width = pattern_shape(box)
        pixels = line_screen(height, width, float(period), float(angle))
        paste_labeled_panel(image, box, f"period={period}px  angle={angle}deg", pixels)
    return image


def generate_manga_screentones(size: int) -> Image.Image:
    image = new_canvas(size, size, "03  Manga dot screentones - pitch, angle, density")
    pitches = [3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 32, 40, 64, 96]
    angles = [0.0, 7.5, 15.0, 22.5, 30.0, 37.5, 45.0, 52.5] * 2
    coverages = [0.18, 0.32, 0.50, 0.65] * 4
    for box, pitch, angle, coverage in zip(
        panel_layout(size, size, 4, 4), pitches, angles, coverages, strict=True
    ):
        height, width = pattern_shape(box)
        pixels = dot_screen(height, width, float(pitch), angle, coverage)
        paste_labeled_panel(
            image,
            box,
            f"pitch={pitch}px  angle={angle:g}deg  ink={coverage:.0%}",
            pixels,
        )
    return image


def generate_crossed_screens(size: int) -> Image.Image:
    image = new_canvas(size, size, "04  Crossed screens - close-angle interference")
    periods = [4, 5, 6, 8, 10, 12, 16, 24] * 2
    bases = [0, 15, 30, 45] * 4
    angle_deltas = [0.25, 0.5, 1.0, 2.0] * 4
    period_deltas = [0.0, 0.01, 0.02, 0.04] * 4
    for box, period, base, angle_delta, period_delta in zip(
        panel_layout(size, size, 4, 4),
        periods,
        bases,
        angle_deltas,
        period_deltas,
        strict=True,
    ):
        height, width = pattern_shape(box)
        pixels = crossed_screen(height, width, period, base, angle_delta, period_delta)
        paste_labeled_panel(
            image,
            box,
            f"p={period}px  {base}deg + {angle_delta:g}deg  dp={period_delta:.0%}",
            pixels,
        )
    return image


def generate_zone_plates(size: int) -> Image.Image:
    image = new_canvas(size, size, "05  Zone plates - frequency reaches Nyquist at the edge")
    boxes = list(panel_layout(size, size, 2, 2))
    generators = [
        ("radial binary", lambda h, w: radial_zone_plate(h, w, True)),
        ("radial grayscale", lambda h, w: radial_zone_plate(h, w, False)),
        ("720 angular spokes", lambda h, w: angular_spokes(h, w, 720)),
        (
            "radial x diagonal chirp",
            lambda h, w: np.clip(
                radial_zone_plate(h, w, False).astype(np.float32)
                * chirp(h, w, "diagonal", False).astype(np.float32)
                / 255.0,
                0,
                255,
            ),
        ),
    ]
    for box, (label, generator) in zip(boxes, generators, strict=True):
        height, width = pattern_shape(box)
        paste_labeled_panel(image, box, label, generator(height, width))
    return image


def generate_frequency_sweeps(size: int) -> Image.Image:
    image = new_canvas(size, size, "06  Linear frequency sweeps - low frequency to Nyquist")
    boxes = list(panel_layout(size, size, 2, 2))
    specs = [
        ("horizontal binary", "horizontal", True),
        ("horizontal grayscale", "horizontal", False),
        ("vertical binary", "vertical", True),
        ("diagonal grayscale", "diagonal", False),
    ]
    for box, (label, direction, binary) in zip(boxes, specs, strict=True):
        height, width = pattern_shape(box)
        paste_labeled_panel(image, box, label, chirp(height, width, direction, binary))
    return image


def tone_tile(width: int, height: int, kind: str) -> Image.Image:
    if kind == "dots":
        pixels = dot_screen(height, width, 7.0, 22.5, 0.38)
    elif kind == "lines":
        pixels = line_screen(height, width, 6.0, 32.0, 0.35)
    elif kind == "cross":
        pixels = crossed_screen(height, width, 9.0, 18.0, 1.0, 0.02)
    else:
        raise ValueError(kind)
    return Image.fromarray(pixels.astype(np.uint8), mode="L")


def generate_mixed_manga_page(size: int) -> Image.Image:
    image = new_canvas(size, size, "07  Mixed manga-like page - tones, line art, flat whites")
    draw = ImageDraw.Draw(image)
    margin = 50
    top = HEADER_HEIGHT + 34
    gap = 32
    half_w = (size - margin * 2 - gap) // 2
    upper_h = int(size * 0.40)
    bottom_y = top + upper_h + gap
    bottom_h = size - bottom_y - margin

    panels = [
        (margin, top, margin + half_w, top + upper_h),
        (margin + half_w + gap, top, size - margin, top + upper_h),
        (margin, bottom_y, size - margin, size - margin),
    ]
    for panel in panels:
        draw.rectangle(panel, fill=252, outline=8, width=12)

    def point(panel: tuple[int, int, int, int], x: float, y: float) -> tuple[int, int]:
        return (
            round(panel[0] + (panel[2] - panel[0]) * x),
            round(panel[1] + (panel[3] - panel[1]) * y),
        )

    def rect(
        panel: tuple[int, int, int, int],
        x0: float,
        y0: float,
        x1: float,
        y1: float,
    ) -> tuple[int, int, int, int]:
        return (*point(panel, x0, y0), *point(panel, x1, y1))

    # Upper-left: dotted sky behind bold character-like silhouettes.
    p0 = panels[0]
    tone = tone_tile(p0[2] - p0[0] - 20, p0[3] - p0[1] - 20, "dots")
    image.paste(tone, (p0[0] + 10, p0[1] + 10))
    draw.ellipse(rect(p0, 0.20, 0.13, 0.78, 0.80), fill=244, outline=8, width=18)
    draw.ellipse(rect(p0, 0.34, 0.26, 0.46, 0.42), fill=18)
    draw.ellipse(rect(p0, 0.55, 0.26, 0.67, 0.42), fill=18)
    draw.arc(rect(p0, 0.38, 0.37, 0.63, 0.62), 15, 165, fill=18, width=20)
    draw.ellipse(rect(p0, 0.04, 0.04, 0.42, 0.26), fill=252, outline=8, width=10)
    draw.text(point(p0, 0.10, 0.11), "DOT TONE", fill=12, font=LABEL_FONT)

    # Upper-right: fine hatching, white bubble, and flat black areas.
    p1 = panels[1]
    hatch = tone_tile(p1[2] - p1[0] - 20, p1[3] - p1[1] - 20, "lines")
    image.paste(hatch, (p1[0] + 10, p1[1] + 10))
    draw.polygon(
        [point(p1, 0.04, 0.93), point(p1, 0.42, 0.11), point(p1, 0.95, 0.28), point(p1, 0.95, 0.94)],
        fill=32,
    )
    draw.ellipse(rect(p1, 0.08, 0.09, 0.56, 0.47), fill=252, outline=8, width=10)
    draw.text(point(p1, 0.20, 0.23), "LINE TONE", fill=12, font=LABEL_FONT)

    # Bottom: close-angle cross screen clipped to diagonal regions.
    p2 = panels[2]
    inner_w = p2[2] - p2[0] - 20
    inner_h = p2[3] - p2[1] - 20
    cross = tone_tile(inner_w, inner_h, "cross")
    mask = Image.new("L", (inner_w, inner_h), 0)
    mask_draw = ImageDraw.Draw(mask)
    mask_draw.polygon([(0, 0), (inner_w, 0), (inner_w // 2, inner_h)], fill=255)
    mask_draw.ellipse((inner_w // 8, inner_h // 5, inner_w * 7 // 8, inner_h * 6 // 5), fill=255)
    image.paste(cross, (p2[0] + 10, p2[1] + 10), mask)
    draw.line((*point(p2, 0.02, 0.93), *point(p2, 0.97, 0.08)), fill=8, width=18)
    draw.line((*point(p2, 0.04, 0.97), *point(p2, 0.99, 0.13)), fill=8, width=7)
    for offset in range(0, inner_w, 180):
        draw.arc(
            (p2[0] + offset - 240, p2[1] + 240, p2[0] + offset + 420, p2[3] - 160),
            190,
            345,
            fill=20,
            width=8,
        )
    draw.rectangle(rect(p2, 0.03, 0.06, 0.31, 0.25), fill=252, outline=8, width=10)
    draw.text(point(p2, 0.07, 0.13), "CROSS SCREEN + LINE ART", fill=12, font=LABEL_FONT)
    return image


def generate_odd_dimension_edges(size: int) -> Image.Image:
    width = size - 3
    height = (size * 3) // 4 + 7
    image = new_canvas(width, height, "08  Odd dimensions - mip edges and last row/column")
    top = HEADER_HEIGHT
    content_h = height - top
    third = width // 3
    image.paste(Image.fromarray(checkerboard(content_h, third, 3).astype(np.uint8), mode="L"), (0, top))
    image.paste(
        Image.fromarray(dot_screen(content_h, third, 7.0, 17.0, 0.42).astype(np.uint8), mode="L"),
        (third, top),
    )
    last_w = width - third * 2
    image.paste(
        Image.fromarray(radial_zone_plate(content_h, last_w, True).astype(np.uint8), mode="L"),
        (third * 2, top),
    )
    draw = ImageDraw.Draw(image)
    draw.line((0, 0, width - 1, 0), fill=32, width=5)
    draw.line((0, 0, 0, height - 1), fill=80, width=5)
    draw.line((width - 1, 0, width - 1, height - 1), fill=150, width=5)
    draw.line((0, height - 1, width - 1, height - 1), fill=220, width=5)
    draw.line((third, top, third, height - 1), fill=128, width=5)
    draw.line((third * 2, top, third * 2, height - 1), fill=128, width=5)
    draw.text((28, height - 56), f"exact size: {width} x {height}", fill=128, font=SMALL_FONT)
    return image


def write_readme(output_dir: Path, files: list[Path]) -> None:
    lines = [
        "# Mipmap / moire test images",
        "",
        "Generated by `python scripts/generate_moire_test_images.py`.",
        "",
        "Recommended checks:",
        "",
        "1. Open each image in the static-image fullscreen viewer.",
        "2. Use `Standard (interpolated)` and compare page-fit, width-fit, and 100%.",
        "3. At page-fit, look for moving bands, false waves, flicker, and sudden tone changes.",
        "4. Thumbnails and `Nearest` intentionally do not use the static-image mipmap path.",
        "5. `04` contains real interference beats; compare their stability instead of expecting them to disappear.",
        "6. In `05` and `06`, the clean/aliased boundary should remain smooth and stable while resizing.",
        "7. In `08`, confirm that the right and bottom edges do not disappear on odd mip dimensions.",
        "",
        "Files:",
        "",
    ]
    descriptions = {
        "01_": "Checker cells from 1 px to 128 px.",
        "02_": "Line periods and angles, including near-horizontal/vertical cases.",
        "03_": "Rotated manga dot tones with multiple densities.",
        "04_": "Close-angle and close-period screens that expose unstable beats.",
        "05_": "Radial and angular frequency ramps up to Nyquist.",
        "06_": "Horizontal, vertical, and diagonal linear chirps.",
        "07_": "Mixed flat white, line art, dots, hatching, and crossed screens.",
        "08_": "Odd-size edge and mip-chain test.",
    }
    for path in files:
        description = next(
            (description for prefix, description in descriptions.items() if path.name.startswith(prefix)),
            "Generated moire fixture.",
        )
        lines.append(f"- `{path.name}` - {description}")
    lines.append("")
    (output_dir / "README.md").write_text("\n".join(lines), encoding="utf-8")


def write_manifest(output_dir: Path, files: list[Path]) -> None:
    manifest = []
    for path in files:
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        manifest.append(f"{digest} *{path.name}")
    (output_dir / "manifest.sha256").write_text("\n".join(manifest) + "\n", encoding="ascii")


def save_png(image: Image.Image, path: Path) -> None:
    image.save(path, format="PNG", optimize=True, compress_level=9)
    print(f"wrote {path}  {image.width}x{image.height}  {path.stat().st_size:,} bytes")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--size", type=int, default=4096, help="base square image size (default: 4096)")
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("testimage/moire"),
        help="output directory (default: testimage/moire)",
    )
    args = parser.parse_args()
    if args.size < 1024:
        parser.error("--size must be at least 1024")

    output_dir = args.output.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    size = args.size
    generators = [
        (f"01_checkerboards_{size}.png", generate_checkerboards),
        (f"02_parallel_lines_{size}.png", generate_parallel_lines),
        (f"03_manga_screentones_{size}.png", generate_manga_screentones),
        (f"04_crossed_screens_{size}.png", generate_crossed_screens),
        (f"05_zone_plates_{size}.png", generate_zone_plates),
        (f"06_frequency_sweeps_{size}.png", generate_frequency_sweeps),
        (f"07_mixed_manga_page_{size}.png", generate_mixed_manga_page),
        (f"08_odd_dimensions_{size - 3}x{(size * 3) // 4 + 7}.png", generate_odd_dimension_edges),
    ]

    files: list[Path] = []
    for filename, generator in generators:
        path = output_dir / filename
        save_png(generator(size), path)
        files.append(path)
    write_readme(output_dir, files)
    write_manifest(output_dir, files)
    print(f"wrote {output_dir / 'README.md'}")
    print(f"wrote {output_dir / 'manifest.sha256'}")


if __name__ == "__main__":
    main()
