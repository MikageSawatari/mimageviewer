#!/usr/bin/env python3
"""Draw the master frames for the video scaling test clips.

Two charts, each built so a person can tell right from wrong by looking,
without measuring anything.

``downscale`` (3840x2160) is for judging a 4K video shown in a smaller window.
Each panel pairs fine detail with a flat patch of the grey that detail averages
to.  Filter the detail away correctly and the seam between the two halves
disappears; alias it and the detail half breaks into bands, swirls or rings
that were never in the source.  The periods are deliberately not multiples of
two, so an exact 4K->1080p halving still beats rather than landing on a
constant.

``upscale`` (960x540) is for judging a small video shown large.  It carries
what upscalers actually disagree about: flat colour with hard edges, thin
diagonals at many angles, curves, small text, and a gradient.

Fine detail is greyscale so 4:2:0 chroma subsampling cannot be mistaken for a
scaler's doing.  The flat-colour panel is in colour on purpose: that is the
case line-art upscalers are built for.
"""

import argparse
import math
import os

import numpy as np
from PIL import Image, ImageDraw, ImageFont

FONT_CANDIDATES = [
    r"C:\Windows\Fonts\YuGothM.ttc",
    r"C:\Windows\Fonts\meiryo.ttc",
    r"C:\Windows\Fonts\msgothic.ttc",
]

# Background for the parts of a panel that carry no pattern.
MID = 128


def flat_match(pattern):
    """The code value a correct downscale of `pattern` converges to.

    Taken from the pattern itself rather than assumed, because a period of 3
    is two dark pixels to one light one, not an even split.  Getting this from
    the data is what lets the seam vanish when the filtering is right.
    """
    return int(round(float(pattern.mean())))


def load_font(size):
    for path in FONT_CANDIDATES:
        if os.path.exists(path):
            try:
                return ImageFont.truetype(path, size)
            except OSError:
                continue
    return ImageFont.load_default()


def label(draw, xy, text, size=34, fill=(255, 255, 255)):
    draw.text(xy, text, font=load_font(size), fill=fill, stroke_width=3, stroke_fill=(0, 0, 0))


# ---------------------------------------------------------------- patterns


def grating(w, h, period, angle_deg):
    yy, xx = np.mgrid[0:h, 0:w]
    a = math.radians(angle_deg)
    proj = xx * math.cos(a) + yy * math.sin(a)
    return np.where((proj % period) < (period / 2.0), 0, 255).astype(np.uint8)


def checker(w, h, cell):
    yy, xx = np.mgrid[0:h, 0:w]
    return np.where(((xx // cell) + (yy // cell)) % 2 == 0, 0, 255).astype(np.uint8)


def rings(w, h, period):
    yy, xx = np.mgrid[0:h, 0:w]
    r = np.hypot(xx - w / 2.0, yy - h / 2.0)
    return np.where((r % period) < (period / 2.0), 0, 255).astype(np.uint8)


def siemens_star(w, h, spokes, bg=MID):
    yy, xx = np.mgrid[0:h, 0:w]
    ang = np.arctan2(yy - h / 2.0, xx - w / 2.0)
    v = np.where((ang * spokes / (2 * math.pi)) % 1.0 < 0.5, 0, 255)
    r = np.hypot(xx - w / 2.0, yy - h / 2.0)
    v[r > min(w, h) / 2.0] = bg
    return v.astype(np.uint8)


def zone_plate(w, h, k):
    yy, xx = np.mgrid[0:h, 0:w]
    x = (xx - w / 2.0) / (w / 2.0)
    y = (yy - h / 2.0) / (h / 2.0)
    return (127.5 * (1.0 + np.cos(k * (x * x + y * y)))).astype(np.uint8)


# ---------------------------------------------------------------- downscale


def build_downscale(width=3840, height=2160):
    img = np.full((height, width), 24, np.uint8)
    cols, rows = 3, 2
    pw, ph = width // cols, height // rows
    cap = 96  # caption strip above each panel

    # The first three are finer than 4px, so any halving or more must wipe
    # them out; the seam with the flat half is the whole test.  The last three
    # survive on purpose, and are read for false structure instead.
    panels = [
        ("縦の細線 (3px 周期)", lambda w, h: grating(w, h, 3, 0), "左右の境目が消えれば正しい"),
        ("斜めの細線 (3px 周期・45度)", lambda w, h: grating(w, h, 3, 45), "左右の境目が消えれば正しい"),
        ("市松 (2px)", lambda w, h: checker(w, h, 2), "左右の境目が消えれば正しい"),
        ("同心円 (5px 周期)", lambda w, h: rings(w, h, 5), "うずまき状の縞が出たら偽物"),
        ("放射 (くさび形 180 本)", lambda w, h: siemens_star(w, h, 180), "中心付近の渦が偽物"),
        ("ゾーンプレート", lambda w, h: zone_plate(w, h, 900), "外周の余分なリングが偽物"),
    ]

    for idx, (_, fn, _) in enumerate(panels):
        cx, cy = idx % cols, idx // cols
        x0, y0 = cx * pw, cy * ph
        iw, ih = pw - 24, ph - cap - 24
        px, py = x0 + 12, y0 + cap
        if idx >= 3:  # read whole; these are meant to survive
            img[py : py + ih, px : px + iw] = fn(iw, ih)
            continue
        half = iw // 2
        patch = fn(half, ih)
        img[py : py + ih, px : px + half] = patch
        img[py : py + ih, px + half : px + iw] = flat_match(patch)

    pil = Image.fromarray(img, "L").convert("RGB")
    draw = ImageDraw.Draw(pil)
    for idx, (name, _, criterion) in enumerate(panels):
        cx, cy = idx % cols, idx // cols
        label(draw, (cx * pw + 24, cy * ph + 14), name, size=42)
        label(draw, (cx * pw + 24, cy * ph + 60), criterion, size=30,
              fill=(150, 205, 255))
    return pil


# ---------------------------------------------------------------- upscale


def build_upscale(width=960, height=540):
    img = np.full((height, width), 24, np.uint8)

    # Curves and converging edges: ringing and staircasing show here.
    img[36:336, 24:324] = siemens_star(300, 300, 36, bg=24)

    # Thin diagonals across a spread of angles.
    for i, ang in enumerate((10, 25, 45, 65, 80)):
        img[44 + i * 58 : 96 + i * 58, 348:576] = grating(228, 52, 9, ang)

    pil = Image.fromarray(img, "L").convert("RGB")
    draw = ImageDraw.Draw(pil)

    # Flat colour with hard edges: the case line-art upscaling is built for.
    draw.rectangle((600, 36, 936, 336), fill=(38, 42, 56))
    draw.ellipse((624, 56, 784, 216), fill=(236, 226, 208), outline=(24, 22, 30), width=3)
    draw.polygon([(812, 216), (876, 56), (912, 216)], fill=(214, 96, 104),
                 outline=(24, 22, 30))
    draw.rectangle((624, 236, 912, 288), fill=(96, 168, 214), outline=(24, 22, 30), width=3)
    for i in range(10):  # hairlines converging to a point
        draw.line((624 + i * 32, 300, 700 + i * 22, 330), fill=(240, 240, 245), width=1)

    # Text at several sizes: the most honest readability test there is.
    label(draw, (24, 352), "あいうえお 漢字 ABCdef 0123", size=30)
    label(draw, (24, 394), "あいうえお 漢字 ABCdef 0123", size=20)
    label(draw, (24, 424), "あいうえお 漢字 ABCdef 0123", size=13)

    # Gradient strip: banding shows here.
    arr = np.asarray(pil).copy()
    arr[458:502, 24:584] = np.tile(
        np.linspace(0, 255, 560).astype(np.uint8)[None, :, None], (44, 1, 3)
    )
    pil = Image.fromarray(arr, "RGB")
    draw = ImageDraw.Draw(pil)
    label(draw, (600, 352), "拡大品質テスト 960x540", size=26)
    label(draw, (600, 392), "細線・曲線・文字・平坦色の境界を見る", size=17,
          fill=(170, 200, 255))
    label(draw, (600, 462), "階調の帯に注意", size=20, fill=(170, 200, 255))
    return pil


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", default=".")
    args = ap.parse_args()
    os.makedirs(args.out_dir, exist_ok=True)
    for name, chart in (
        ("chart_downscale_4k.png", build_downscale()),
        ("chart_upscale_540p.png", build_upscale()),
    ):
        path = os.path.join(args.out_dir, name)
        chart.save(path)
        print(f"wrote {path} {chart.size[0]}x{chart.size[1]}")


if __name__ == "__main__":
    main()
