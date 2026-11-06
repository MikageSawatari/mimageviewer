"""Depth-parallax spike: run candidate depth models on sample images.

Usage:
  python run_compare.py <spike_dir>
    <spike_dir>/models/<name>/...onnx, <spike_dir>/samples/*, writes <spike_dir>/out/

Outputs per sample:
  <stem>_depth.jpg       original + each model's disparity map (with timings)
  <stem>_parallax.mp4    2x2 grid: each model's parallax animation in sync
  timings.tsv            DirectML / CPU inference times
"""
from __future__ import annotations

import sys
import time
from pathlib import Path

import cv2
import numpy as np
import onnxruntime as ort
from PIL import Image, ImageDraw, ImageFont, ImageOps

try:
    from pillow_heif import register_heif_opener

    register_heif_opener()
except ImportError:
    pass

sys.path.insert(0, str(Path(__file__).parent))
import parallax  # noqa: E402

MEAN = np.array([0.485, 0.456, 0.406], np.float32)
STD = np.array([0.229, 0.224, 0.225], np.float32)


def resize_multiple(rgb, target, mode, multiple=14):
    h, w = rgb.shape[:2]
    s = target / min(h, w) if mode == "lower" else target / max(h, w)
    nh = max(multiple, round(h * s / multiple) * multiple)
    nw = max(multiple, round(w * s / multiple) * multiple)
    return cv2.resize(rgb, (nw, nh), interpolation=cv2.INTER_CUBIC)


def imagenet(rgb):
    x = (rgb.astype(np.float32) / 255.0 - MEAN) / STD
    return x.transpose(2, 0, 1)[None]


def safe_inverse(depth):
    d = np.where(np.isfinite(depth) & (depth > 0), depth, np.nan)
    disp = 1.0 / d
    return np.nan_to_num(disp, nan=0.0)


class Model:
    def __init__(self, name, path, provider):
        self.name = name
        self.path = path
        self.providers = ["DmlExecutionProvider"] if provider == "dml" else ["CPUExecutionProvider"]
        self.sessions = {}
        # Probe once for input names / symbolic dims.
        self.probe = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])

    def sess_for(self, h, w):
        """One session per input size with all symbolic dims fixed.
        DirectML is ~8x faster with static shapes (spike measurement)."""
        key = (h, w)
        if key not in self.sessions:
            so = ort.SessionOptions()
            so.enable_mem_pattern = False
            for i in self.probe.get_inputs():
                for d in i.shape:
                    if isinstance(d, str):
                        so.add_free_dimension_override_by_name(
                            d, {"height": h, "width": w}.get(d, 1)
                        )
            self.sessions[key] = ort.InferenceSession(str(self.path), so, providers=self.providers)
        return self.sessions[key]

    def run(self, rgb) -> np.ndarray:  # returns disparity (near = large)
        raise NotImplementedError


class DAv2(Model):
    def run(self, rgb):
        x = imagenet(resize_multiple(rgb, 518, "lower"))
        return self.sess_for(*x.shape[-2:]).run(None, {"pixel_values": x})[0][0]


class DA3Small(Model):
    def run(self, rgb):
        x = imagenet(resize_multiple(rgb, 504, "upper"))[:, None]
        depth = self.sess_for(*x.shape[-2:]).run(["predicted_depth"], {"pixel_values": x})[0][0, 0]
        return safe_inverse(depth)


class DA3MonoLarge(Model):
    def run(self, rgb):
        # SpatialHub export: camera inputs filled with -1 when unknown.
        x = imagenet(resize_multiple(rgb, 504, "upper"))[:, None]
        feed = {
            "image": x,
            "extrinsics_in": np.full((1, 1, 4, 4), -1.0, np.float32),
            "intrinsics_in": np.full((1, 1, 3, 3), -1.0, np.float32),
        }
        depth, sky = self.sess_for(*x.shape[-2:]).run(["depth", "sky"], feed)
        depth = depth[0, 0]
        disp = safe_inverse(depth)
        return np.where(sky[0, 0] > 0.5, 0.0, disp)


class MoGe2(Model):
    def run(self, rgb):
        small = resize_multiple(rgb, 1036, "upper")
        x = (small.astype(np.float32) / 255.0).transpose(2, 0, 1)[None]
        points, _normal, mask, _scale = self.sess_for(*x.shape[-2:]).run(
            None, {"image": x, "num_tokens": np.array(2500, np.int64)}
        )
        z = points[0, ..., 2]
        z = np.where(mask[0] > 0.5, z, np.inf)
        return safe_inverse(z)


MODELS = [
    ("DAv2-S", DAv2, "dav2s/model.onnx"),
    ("DA3-S", DA3Small, "da3s/model.onnx"),
    ("MoGe2-S", MoGe2, "moge2s/model.onnx"),
    ("DA3Mono-L", DA3MonoLarge, "da3monol/da3mono_large.onnx"),
]


def load_rgb(path, max_side=1600):
    im = ImageOps.exif_transpose(Image.open(path)).convert("RGB")
    im.thumbnail((max_side, max_side), Image.LANCZOS)
    return np.asarray(im)


def colorize(d):
    return cv2.cvtColor(cv2.applyColorMap((d * 255).astype(np.uint8), cv2.COLORMAP_INFERNO), cv2.COLOR_BGR2RGB)


def label(img, text):
    im = Image.fromarray(img)
    dr = ImageDraw.Draw(im)
    try:
        font = ImageFont.truetype("arial.ttf", max(14, img.shape[1] // 30))
    except OSError:
        font = ImageFont.load_default()
    dr.rectangle([0, 0, img.shape[1], font.size + 8], fill=(0, 0, 0))
    dr.text((4, 2), text, fill=(255, 255, 255), font=font)
    return np.asarray(im)


def time_runs(model, rgb, n):
    model.run(rgb)  # warm-up (DirectML compiles per shape)
    t = []
    for _ in range(n):
        s = time.perf_counter()
        out = model.run(rgb)
        t.append(time.perf_counter() - s)
    return out, 1000 * float(np.median(t))


def main():
    root = Path(sys.argv[1])
    out_dir = root / "out"
    out_dir.mkdir(exist_ok=True)
    only = set(sys.argv[2:])
    samples = sorted(p for p in (root / "samples").iterdir() if p.is_file())
    if only:
        samples = [p for p in samples if p.stem in only]

    models = []
    for name, cls, rel in MODELS:
        path = root / "models" / rel
        if not path.exists():
            print("skip (missing)", name)
            continue
        t0 = time.perf_counter()
        m = cls(name, path, "dml")
        print(f"load {name}: {1000 * (time.perf_counter() - t0):.0f} ms")
        models.append(m)

    tsv = open(out_dir / "timings.tsv", "a", encoding="utf-8")
    for sp in samples:
        rgb = load_rgb(sp)
        print("==", sp.name, rgb.shape)
        tiles = [label(rgb, sp.stem[:28])]
        depths = {}
        for m in models:
            try:
                disp, ms = time_runs(m, rgb, 3)
            except Exception as e:  # noqa: BLE001 - spike: record and continue
                print(f"  {m.name}: FAILED {type(e).__name__}: {str(e)[:300]}")
                continue
            print(f"  {m.name}: {ms:.0f} ms  in={disp.shape}")
            tsv.write(f"{sp.name}\t{m.name}\tdml\t{ms:.1f}\t{rgb.shape[1]}x{rgb.shape[0]}\n")
            depths[m.name] = parallax.prepare_depth(disp, rgb)
            tiles.append(label(colorize(depths[m.name]), f"{m.name} {ms:.0f} ms"))
        sheet = np.concatenate(tiles, axis=1 if rgb.shape[0] >= rgb.shape[1] else 0)
        Image.fromarray(sheet).save(out_dir / f"{sp.stem}_depth.jpg", quality=88)
        np.savez_compressed(out_dir / f"{sp.stem}_depth.npz", **depths)
    tsv.close()


if __name__ == "__main__":
    main()
