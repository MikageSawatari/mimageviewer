"""Build a sample book for checking auto-trim against the landscape split.

The point of the sample is that the margin is *visible*: every page has a wide
white border around a coloured block, and the block carries a printed label so
you can tell which half you are looking at without counting.

Pages:
  001  portrait  with margin      -- trim has something to do
  002  landscape with margin      -- the split page; halves are labelled L / R
  003  portrait  with margin
  004  landscape with margin      -- a second split page, different aspect
  005  portrait  no margin        -- trim should do nothing here

Run:  python make_trim_sample.py <output-dir>
"""
import os
import sys

from PIL import Image, ImageDraw, ImageFont


def font(size):
    for name in ("meiryo.ttc", "YuGothM.ttc", "arial.ttf"):
        path = os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "Fonts", name)
        if os.path.exists(path):
            try:
                return ImageFont.truetype(path, size)
            except OSError:
                pass
    return ImageFont.load_default()


def label(draw, box, text, size):
    x0, y0, x1, y1 = box
    f = font(size)
    left, top, right, bottom = draw.textbbox((0, 0), text, font=f)
    draw.text(
        ((x0 + x1 - (right - left)) / 2, (y0 + y1 - (bottom - top)) / 2),
        text,
        fill=(255, 255, 255),
        font=f,
    )


def page(width, height, margin, blocks):
    """blocks: list of (fraction_start, fraction_end, rgb, text)."""
    image = Image.new("RGB", (width, height), (255, 255, 255))
    draw = ImageDraw.Draw(image)
    top, bottom = margin, height - margin
    for start, end, color, text in blocks:
        x0 = margin + (width - 2 * margin) * start
        x1 = margin + (width - 2 * margin) * end
        draw.rectangle([x0, top, x1, bottom], fill=color)
        label(draw, (x0, top, x1, bottom), text, max(24, height // 12))
    # A thin frame on the true page edge, so you can see where the margin ends.
    draw.rectangle([0, 0, width - 1, height - 1], outline=(200, 200, 200))
    return image


def main():
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    m = 140  # margin in pixels -- deliberately wide so trimming is obvious

    page(1200, 1700, m, [(0.0, 1.0, (40, 90, 170), "1")]).save(
        os.path.join(out, "001.png")
    )
    page(2400, 1700, m, [
        (0.0, 0.5, (170, 60, 60), "2 L"),
        (0.5, 1.0, (60, 140, 80), "2 R"),
    ]).save(os.path.join(out, "002.png"))
    page(1200, 1700, m, [(0.0, 1.0, (40, 90, 170), "3")]).save(
        os.path.join(out, "003.png")
    )
    page(2800, 1500, m, [
        (0.0, 0.5, (150, 100, 30), "4 L"),
        (0.5, 1.0, (90, 70, 160), "4 R"),
    ]).save(os.path.join(out, "004.png"))
    page(1200, 1700, 0, [(0.0, 1.0, (70, 70, 70), "5")]).save(
        os.path.join(out, "005.png")
    )

    print("wrote 5 pages to", out)
    for name in sorted(os.listdir(out)):
        with Image.open(os.path.join(out, name)) as image:
            print(f"  {name}  {image.width}x{image.height}")


main()
