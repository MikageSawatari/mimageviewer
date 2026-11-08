"""CPU reference of the planned depth-parallax shader (spike only).

Depth here is "disparity": 1 = nearest, 0 = farthest. The renderer is a
backward per-pixel ray march (steep parallax + binary refinement), the same
algorithm the wgpu fragment shader would run.
"""
from __future__ import annotations

import math

import cv2
import numpy as np


def normalize_disparity(d: np.ndarray, lo_pct=1.0, hi_pct=99.0) -> np.ndarray:
    lo, hi = np.percentile(d, [lo_pct, hi_pct])
    if hi - lo < 1e-6:
        return np.zeros_like(d, dtype=np.float32)
    return np.clip((d - lo) / (hi - lo), 0.0, 1.0).astype(np.float32)


def guided_filter(guide: np.ndarray, src: np.ndarray, r: int, eps: float) -> np.ndarray:
    """He et al. guided filter with a grayscale guide (both float32 in 0..1)."""
    k = (2 * r + 1, 2 * r + 1)
    mean = lambda x: cv2.boxFilter(x, -1, k, borderType=cv2.BORDER_REFLECT)
    mi, ms = mean(guide), mean(src)
    cov = mean(guide * src) - mi * ms
    var = mean(guide * guide) - mi * mi
    a = cov / (var + eps)
    b = ms - a * mi
    return mean(a) * guide + mean(b)


def prepare_depth(
    disp_lowres: np.ndarray,
    rgb: np.ndarray,
    *,
    gamma: float = 1.0,
    dilate_px: int = 3,
    blur_px: float = 1.5,
    guided: bool = True,
) -> np.ndarray:
    """Normalize, upsample to the image size along RGB edges, grow the near side."""
    h, w = rgb.shape[:2]
    d = normalize_disparity(disp_lowres)
    d = cv2.resize(d, (w, h), interpolation=cv2.INTER_CUBIC)
    if guided:
        gray = cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY).astype(np.float32) / 255.0
        r = max(2, round(max(h, w) / 256))
        d = guided_filter(gray, d, r, 1e-3)
    d = np.clip(d, 0.0, 1.0)
    if gamma != 1.0:
        d = d ** gamma
    if dilate_px > 0:
        k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * dilate_px + 1,) * 2)
        d = cv2.dilate(d, k)
    if blur_px > 0:
        d = cv2.GaussianBlur(d, (0, 0), blur_px)
    return d.astype(np.float32)


def render_view(
    rgb: np.ndarray,
    disp: np.ndarray,
    offset: tuple[float, float],
    *,
    focus: float = 0.4,
    zoom: float = 1.05,
    steps: int = 40,
    refine: int = 5,
) -> np.ndarray:
    """Render one view. `offset` is the shift of the nearest layer relative to
    the focus plane, in fractions of the image width (e.g. 0.02 = 2%)."""
    h, w = disp.shape
    ox, oy = offset[0] * w, offset[1] * w
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float32)
    cx, cy = (w - 1) / 2, (h - 1) / 2
    px = cx + (xs - cx) / zoom
    py = cy + (ys - cy) / zoom

    def sample_pos(t):
        # A layer at disparity t appears shifted by offset * (t - focus).
        return px - ox * (t - focus), py - oy * (t - focus)

    def sample(img, mx, my):
        return cv2.remap(img, mx, my, cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)

    t_hit = np.zeros((h, w), np.float32)
    t_miss = np.full((h, w), 1.0 + 1.0 / steps, np.float32)
    found = np.zeros((h, w), bool)
    for i in range(steps + 1):
        t = 1.0 - i / steps  # march front to back
        mx, my = sample_pos(t)
        hit = (~found) & (sample(disp, mx, my) >= t)
        t_hit[hit] = t
        found |= hit
        t_miss[~found] = t
    t_hit[~found] = 0.0
    lo, hi = t_hit.copy(), np.minimum(t_miss, 1.0)  # lo: hit (behind), hi: miss
    for _ in range(refine):
        mid = (lo + hi) * 0.5
        mx = px - ox * (mid - focus)
        my = py - oy * (mid - focus)
        h_mid = sample(disp, mx, my) >= mid
        lo = np.where(h_mid, mid, lo)
        hi = np.where(h_mid, hi, mid)
    mx = px - ox * (lo - focus)
    my = py - oy * (lo - focus)
    return sample(rgb, mx, my)


def motion_offsets(kind: str, amount: float, frames: int):
    for i in range(frames):
        a = 2 * math.pi * i / frames
        if kind == "swing":
            yield (amount * math.sin(a), 0.0)
        elif kind == "circle":
            yield (amount * math.cos(a), amount * 0.6 * math.sin(a))
        else:
            raise ValueError(kind)
