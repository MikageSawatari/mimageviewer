"""Depth-parallax spike: render parallax animations from run_compare.py output.

Usage:
  python make_anim.py <spike_dir> grid  [stems...]   2x2 model comparison MP4 per sample
  python make_anim.py <spike_dir> export <model> [stems...]
        single-model GIF / APNG / MP4 (swing and circle) to compare size and look
"""
from __future__ import annotations

import math
import sys
import time
from pathlib import Path

import cv2
import imageio.v2 as imageio
import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).parent))
import parallax  # noqa: E402
import run_compare as rc  # noqa: E402

FRAMES = 60
FPS = 30
AMOUNT = 0.02  # nearest-layer shift vs focus plane, fraction of width
LONG_SIDE = 720


def shape_depth(d):
    """Put the median disparity at 0.5 so a near floor does not flatten the rest."""
    med = float(np.median(d))
    if 0.02 < med < 0.98:
        g = min(1.5, max(0.3, math.log(0.5) / math.log(med)))
        d = d ** g
    return d.astype(np.float32), 0.5


def fit(img, long_side, interp=cv2.INTER_AREA):
    h, w = img.shape[:2]
    s = long_side / max(h, w)
    return cv2.resize(img, (round(w * s) // 2 * 2, round(h * s) // 2 * 2), interpolation=interp)


def render_frames(rgb, disp, kind, amount=AMOUNT, frames=FRAMES):
    disp, focus = shape_depth(disp)
    out = []
    for off in parallax.motion_offsets(kind, amount, frames):
        out.append(parallax.render_view(rgb, disp, off, focus=focus, zoom=1.0 + 2.2 * amount))
    return out


def load(root, stem):
    sample = next(p for p in (root / "samples").iterdir() if p.stem == stem)
    rgb = rc.load_rgb(sample)
    depths = dict(np.load(root / "out" / f"{stem}_depth.npz"))
    rgb_s = fit(rgb, LONG_SIDE)
    h, w = rgb_s.shape[:2]
    return rgb_s, {k: cv2.resize(v, (w, h), interpolation=cv2.INTER_AREA) for k, v in depths.items()}


def grid(root, stems):
    for stem in stems:
        rgb, depths = load(root, stem)
        names = list(depths)
        t0 = time.perf_counter()
        per_model = {n: render_frames(rgb, depths[n], "swing") for n in names}
        print(f"{stem}: {(time.perf_counter() - t0) / (FRAMES * len(names)) * 1000:.0f} ms/frame (CPU ref)")
        tiles = []
        for i in range(FRAMES):
            cells = [rc.label(per_model[n][i], n) for n in names]
            while len(cells) < 4:
                cells.append(np.zeros_like(cells[0]))
            tiles.append(np.concatenate([np.concatenate(cells[:2], 1), np.concatenate(cells[2:4], 1)], 0))
        imageio.mimwrite(root / "out" / f"{stem}_grid.mp4", tiles * 2, fps=FPS, quality=8, macro_block_size=2)


def export(root, model, stems):
    for stem in stems:
        rgb, depths = load(root, stem)
        for kind in ("swing", "circle"):
            frames = render_frames(rgb, depths[model], kind)
            base = root / "out" / f"{stem}_{model}_{kind}"
            imageio.mimwrite(f"{base}.mp4", frames * 3, fps=FPS, quality=8, macro_block_size=2)
            pil = [Image.fromarray(f) for f in frames[::2]]  # GIF at 15 fps
            pil[0].save(f"{base}.gif", save_all=True, append_images=pil[1:], duration=1000 // 15, loop=0, optimize=True)
            pil[0].save(f"{base}.png", save_all=True, append_images=pil[1:], duration=1000 // 15, loop=0, format="PNG")
            sizes = {ext: Path(f"{base}.{ext}").stat().st_size / 1e6 for ext in ("mp4", "gif", "png")}
            print(stem, kind, {k: f"{v:.1f} MB" for k, v in sizes.items()})


def main():
    root = Path(sys.argv[1])
    mode = sys.argv[2]
    if mode == "grid":
        grid(root, sys.argv[3:])
    elif mode == "export":
        export(root, sys.argv[3], sys.argv[4:])


if __name__ == "__main__":
    main()
