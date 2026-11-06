"""Depth-parallax spike: one fixed 504x504 input (padded or stretched) vs per-aspect input.

Usage:
  python check_static.py <spike_dir> <stem>...
Writes models/da3monol/da3mono_large_fp16_504.onnx with every input dim fixed.
"""
import sys
import time
from pathlib import Path

import cv2
import numpy as np
import onnx
import onnxruntime as ort

sys.path.insert(0, str(Path(__file__).parent))
import parallax  # noqa: E402
import run_compare as rc  # noqa: E402

S = 504
ort.set_default_logger_severity(3)
root = Path(sys.argv[1])
d = root / "models" / "da3monol"
src, static = d / "da3mono_large_fp16.onnx", d / "da3mono_large_fp16_504.onnx"
if not static.exists():
    m = onnx.load(str(src))
    fixed = {"num_views": 1, "height": S, "width": S}
    for i in m.graph.input:
        for dim in i.type.tensor_type.shape.dim:
            if dim.dim_param:
                dim.dim_value = fixed[dim.dim_param]
    onnx.save(m, str(static))
sess = ort.InferenceSession(str(static), providers=["DmlExecutionProvider"])
native = rc.DA3MonoLarge("native", src, "dml")
cam = {"extrinsics_in": np.full((1, 1, 4, 4), -1.0, np.float32),
       "intrinsics_in": np.full((1, 1, 3, 3), -1.0, np.float32)}


def run_static(x):
    depth, sky = sess.run(["depth", "sky"], {"image": x, **cam})
    return np.where(sky[0, 0] > 0.5, 0.0, rc.safe_inverse(depth[0, 0]))


def padded(rgb):
    h, w = rgb.shape[:2]
    s = S / max(h, w)
    nh, nw = round(h * s), round(w * s)
    small = cv2.resize(rgb, (nw, nh), interpolation=cv2.INTER_CUBIC)
    top, left = (S - nh) // 2, (S - nw) // 2
    canvas = cv2.copyMakeBorder(small, top, S - nh - top, left, S - nw - left, cv2.BORDER_REPLICATE)
    return run_static(rc.imagenet(canvas)[:, None])[top:top + nh, left:left + nw]


def stretched(rgb):
    return run_static(rc.imagenet(cv2.resize(rgb, (S, S), interpolation=cv2.INTER_CUBIC))[:, None])


def norm_to(disp, shape):
    return cv2.resize(parallax.normalize_disparity(disp), (shape[1], shape[0]), interpolation=cv2.INTER_LINEAR)


for stem in sys.argv[2:]:
    rgb = rc.load_rgb(next(p for p in (root / "samples").iterdir() if p.stem == stem))
    ref = norm_to(native.run(rgb), rgb.shape)
    row = [f"{stem[:24]:24s} {rgb.shape[1]}x{rgb.shape[0]}"]
    for name, fn in (("pad", padded), ("stretch", stretched)):
        fn(rgb)
        t = time.perf_counter()
        out = norm_to(fn(rgb), rgb.shape)
        ms = (time.perf_counter() - t) * 1000
        diff = np.abs(out - ref)
        row.append(f"{name}: corr {np.corrcoef(out.ravel(), ref.ravel())[0, 1]:.4f} mean {diff.mean():.3f} p99 {np.percentile(diff, 99):.3f} {ms:.0f}ms")
    print("  ".join(row), flush=True)
