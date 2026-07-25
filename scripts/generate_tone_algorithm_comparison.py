"""Generate deterministic screentone samples for the colorize weak/strong modes."""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageEnhance, ImageFilter, ImageFont


WIDTH = 2048
HEIGHT = 1440
INK = 24
PAPER = 244
OUTPUT_DIR = Path(__file__).resolve().parents[1] / "samples" / "tone-algorithm-comparison"

BAYER_8 = (
    (0, 48, 12, 60, 3, 51, 15, 63),
    (32, 16, 44, 28, 35, 19, 47, 31),
    (8, 56, 4, 52, 11, 59, 7, 55),
    (40, 24, 36, 20, 43, 27, 39, 23),
    (2, 50, 14, 62, 1, 49, 13, 61),
    (34, 18, 46, 30, 33, 17, 45, 29),
    (10, 58, 6, 54, 9, 57, 5, 53),
    (42, 26, 38, 22, 41, 25, 37, 21),
)


def font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    candidates = (
        Path(r"C:\Windows\Fonts\segoeui.ttf"),
        Path(r"C:\Windows\Fonts\arial.ttf"),
    )
    for candidate in candidates:
        if candidate.exists():
            return ImageFont.truetype(str(candidate), size)
    return ImageFont.load_default()


def label(draw: ImageDraw.ImageDraw, xy: tuple[int, int], text: str, size: int = 30) -> None:
    draw.text(xy, text, fill=INK, font=font(size))


def ordered_tone(
    image: Image.Image,
    box: tuple[int, int, int, int],
    density_at: callable,
    period: int = 8,
) -> None:
    pixels = image.load()
    left, top, right, bottom = box
    for y in range(top, bottom):
        for x in range(left, right):
            density = max(0.0, min(1.0, float(density_at(x, y))))
            threshold = (BAYER_8[(y - top) % 8][(x - left) % 8] + 0.5) / 64.0
            if period != 8:
                threshold = (
                    BAYER_8[((y - top) * 8 // period) % 8][((x - left) * 8 // period) % 8]
                    + 0.5
                ) / 64.0
            pixels[x, y] = INK if threshold < density else PAPER


def dot_tone(
    image: Image.Image,
    box: tuple[int, int, int, int],
    period: int,
    radius: float,
    angle_sign: int = 1,
) -> None:
    pixels = image.load()
    left, top, right, bottom = box
    center = (period - 1) * 0.5
    radius_sq = radius * radius
    for y in range(top, bottom):
        for x in range(left, right):
            local_x = (x - left + angle_sign * (y - top) // 2) % period
            local_y = (y - top) % period
            distance_sq = (local_x - center) ** 2 + (local_y - center) ** 2
            pixels[x, y] = INK if distance_sq <= radius_sq else PAPER


def make_source() -> Image.Image:
    image = Image.new("L", (WIDTH, HEIGHT), PAPER)
    draw = ImageDraw.Draw(image)
    title_font = font(46)
    draw.text((56, 34), "Screentone algorithm comparison", fill=INK, font=title_font)
    label(draw, (58, 92), "Long edge: 2048 px / Compare at detection scale 1.0", 26)

    panels = [
        (56, 160, 650, 540),
        (727, 160, 1321, 540),
        (1398, 160, 1992, 540),
        (56, 620, 986, 1030),
        (1062, 620, 1992, 1030),
        (56, 1110, 1992, 1378),
    ]
    for panel in panels:
        draw.rounded_rectangle(panel, radius=14, outline=70, width=3)

    label(draw, (78, 180), "Fine dots: period 3 px", 28)
    dot_tone(image, (80, 230, 626, 510), period=3, radius=0.75)

    label(draw, (749, 180), "Medium dots: period 5 px", 28)
    dot_tone(image, (751, 230, 1297, 510), period=5, radius=1.45)

    label(draw, (1420, 180), "Mixed tone boundary", 28)
    dot_tone(image, (1422, 230, 1694, 510), period=4, radius=0.9)
    dot_tone(image, (1694, 230, 1968, 510), period=4, radius=1.55, angle_sign=-1)
    draw.line((1694, 230, 1694, 510), fill=INK, width=2)

    label(draw, (78, 640), "Ordered-tone gradient", 28)
    ordered_tone(
        image,
        (80, 690, 962, 1002),
        lambda x, _y: 0.08 + 0.82 * (x - 80) / (962 - 80),
    )

    label(draw, (1084, 640), "Tone + hard shapes / thin lines", 28)
    ordered_tone(image, (1086, 690, 1968, 1002), lambda _x, _y: 0.42)
    draw.rectangle((1160, 742, 1470, 946), fill=PAPER, outline=INK, width=3)
    draw.ellipse((1530, 726, 1840, 962), fill=PAPER, outline=INK, width=5)
    for offset, line_width in ((0, 1), (34, 2), (72, 4), (116, 7)):
        draw.line((1120, 720 + offset, 1920, 850 + offset), fill=INK, width=line_width)

    label(draw, (78, 1128), "Text and ink strokes (blur visibility reference)", 28)
    draw.text((94, 1190), "Aa  12  TEXT  Manga  1px  2px  4px", fill=INK, font=font(64))
    for x, line_width in ((102, 1), (260, 2), (430, 4), (620, 8)):
        draw.line((x, 1300, x + 120, 1346), fill=INK, width=line_width)
    draw.arc((850, 1190, 1120, 1360), start=195, end=520, fill=INK, width=3)
    draw.polygon(((1260, 1328), (1430, 1186), (1600, 1328)), outline=INK)
    draw.ellipse((1730, 1188, 1920, 1350), outline=INK, width=2)
    return image


def make_reference_outputs(source: Image.Image) -> tuple[Image.Image, Image.Image, Image.Image]:
    # mImageViewer scale 1.0 at a 2048px long edge:
    # weak = one radius-1 box pass, strong = three radius-1 box passes.
    weak = source.filter(ImageFilter.BoxBlur(1))
    strong = source
    for _ in range(3):
        strong = strong.filter(ImageFilter.BoxBlur(1))
    difference = ImageChops.difference(weak, strong)
    difference = ImageEnhance.Contrast(difference).enhance(4.0)
    return weak, strong, difference


def main() -> None:
    OUTPUT_DIR.mkdir(parents=True, exist_ok=True)
    source = make_source()
    weak, strong, difference = make_reference_outputs(source)
    source.save(OUTPUT_DIR / "01_source.png", optimize=True)
    weak.save(OUTPUT_DIR / "02_reference_weak_local_mean_scale1.png", optimize=True)
    strong.save(OUTPUT_DIR / "03_reference_strong_gaussian_scale1.png", optimize=True)
    difference.save(OUTPUT_DIR / "04_weak_vs_strong_difference_x4.png", optimize=True)
    print(f"generated: {OUTPUT_DIR}")


if __name__ == "__main__":
    main()
