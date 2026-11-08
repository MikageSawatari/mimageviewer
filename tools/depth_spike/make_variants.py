"""Depth-parallax spike: render parameter variations for side-by-side viewing in mIV.

Usage:
  python make_variants.py <spike_dir> [stems...]
Writes <spike_dir>/compare/<stem>/<NN>_<label>.mp4. Each clip is several seamless
loops so it can be flipped through in fullscreen with loop playback on.
"""
from __future__ import annotations

import math
import sys
from multiprocessing import Pool
from pathlib import Path

import cv2
import imageio.v2 as imageio
import numpy as np
from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(Path(__file__).parent))
import parallax  # noqa: E402
import run_compare as rc  # noqa: E402

FPS = 30
LONG_SIDE = 1080
CLIP_SECONDS = 12
FONT = "C:/Windows/Fonts/meiryo.ttc"

BASE = dict(model="DA3Mono-L", amount=0.02, motion="swing", period=3.0, focus="median", shape=True)

# (file label, overrides). The first group member equal to BASE is the reference.
VARIANTS = [
    ("1強さ_1.0%", dict(amount=0.010)),
    ("1強さ_2.0%_基準", dict()),
    ("1強さ_3.0%", dict(amount=0.030)),
    ("1強さ_4.5%", dict(amount=0.045)),
    ("2動き_左右", dict()),
    ("2動き_円", dict(motion="circle")),
    ("2動き_8の字", dict(motion="eight")),
    ("2動き_上下", dict(motion="vertical")),
    ("3周期_1.5秒", dict(period=1.5)),
    ("3周期_3秒", dict()),
    ("3周期_6秒", dict(period=6.0)),
    ("4焦点_手前が止まる", dict(focus=0.85)),
    ("4焦点_中間が止まる", dict()),
    ("4焦点_奥が止まる", dict(focus=0.1)),
    ("5モデル_DA3Mono-L", dict()),
    ("5モデル_DAv2-S", dict(model="DAv2-S")),
    ("6奥行き整形_あり", dict()),
    ("6奥行き整形_なし", dict(shape=False)),
]


def offsets(motion, amount, frames):
    for i in range(frames):
        a = 2 * math.pi * i / frames
        if motion == "swing":
            yield amount * math.sin(a), 0.0
        elif motion == "circle":
            yield amount * math.cos(a), amount * 0.6 * math.sin(a)
        elif motion == "eight":
            yield amount * math.sin(a), amount * 0.5 * math.sin(2 * a)
        elif motion == "vertical":
            yield 0.0, amount * 0.7 * math.sin(a)
        else:
            raise ValueError(motion)


def shape(d, enabled):
    med = float(np.median(d))
    if enabled and 0.02 < med < 0.98:
        g = min(1.5, max(0.3, math.log(0.5) / math.log(med)))
        d = d ** g
        med = 0.5
    return d.astype(np.float32), med


def caption(img, text):
    im = Image.fromarray(img)
    dr = ImageDraw.Draw(im, "RGBA")
    font = ImageFont.truetype(FONT, max(16, img.shape[1] // 45))
    box = dr.textbbox((10, 6), text, font=font)
    dr.rectangle([box[0] - 6, box[1] - 4, box[2] + 6, box[3] + 4], fill=(0, 0, 0, 150))
    dr.text((10, 6), text, fill=(255, 255, 255, 255), font=font)
    return np.asarray(im)


def render(job):
    root, stem, idx, label, params = job
    cv2.setNumThreads(1)
    sample = next(p for p in (root / "samples").iterdir() if p.stem == stem)
    rgb = rc.load_rgb(sample)
    h0, w0 = rgb.shape[:2]
    s = LONG_SIDE / max(h0, w0)
    size = (round(w0 * s) // 2 * 2, round(h0 * s) // 2 * 2)
    rgb = cv2.resize(rgb, size, interpolation=cv2.INTER_AREA)
    depths = np.load(root / "out" / f"{stem}_depth.npz")
    if params["model"] not in depths:
        return f"skip {stem} {label}: no {params['model']}"
    disp = cv2.resize(depths[params["model"]], size, interpolation=cv2.INTER_AREA)
    disp, med = shape(disp, params["shape"])
    focus = med if params["focus"] == "median" else params["focus"]
    frames = round(params["period"] * FPS)
    amount = params["amount"]
    text = (f"{params['model']}  強さ {amount * 100:.1f}%  {label.split('_', 1)[0][1:]}: "
            f"{label.split('_', 1)[1].replace('_基準', '')}")
    loop = [caption(parallax.render_view(rgb, disp, off, focus=focus, zoom=1.0 + 2.2 * amount), text)
            for off in offsets(params["motion"], amount, frames)]
    reps = max(1, round(CLIP_SECONDS / params["period"]))
    out = root / "compare" / stem / f"{idx:02d}_{label}.mp4"
    out.parent.mkdir(parents=True, exist_ok=True)
    imageio.mimwrite(out, loop * reps, fps=FPS, quality=8, macro_block_size=2)
    return f"ok {out.relative_to(root)}"


def main():
    root = Path(sys.argv[1])
    stems = sys.argv[2:]
    jobs = [(root, stem, i + 1, label, {**BASE, **over})
            for stem in stems for i, (label, over) in enumerate(VARIANTS)]
    with Pool(20) as pool:
        for msg in pool.imap_unordered(render, jobs):
            print(msg, flush=True)


if __name__ == "__main__":
    main()
