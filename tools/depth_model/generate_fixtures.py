"""Regenerate small OpenCV oracle fixtures, optionally a local model oracle.

Only synthetic pixels enter the checked-in fixture; no photo/model is committed.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import cv2
import numpy as np

sys.path.insert(0, str(Path(__file__).parents[1] / "depth_spike"))
import parallax


def rgb_pattern(width, height):
    y, x = np.mgrid[:height, :width]
    return np.stack(((x * 37 + y * 13) % 256,
                     (x * 19 + y * 71) % 256,
                     (x * 83 + y * 7) % 256), axis=2).astype(np.uint8)


def floats(array):
    return array.astype(np.float32).ravel().tolist()


def full_size_disparity():
    # Stateless seeded integer noise plus two-axis ramps/steps. Binary fractions
    # make the input exactly reproducible in Rust, independent of RNG libraries.
    y, x = np.mgrid[:504, :504]
    n = (y * 504 + x).astype(np.uint32) ^ np.uint32(0x5A17C9E3)
    n = (n ^ (n >> 16)) * np.uint32(0x7FEB352D)
    n = (n ^ (n >> 15)) * np.uint32(0x846CA68B)
    n ^= n >> 16
    return (((3 * x + 5 * y) % 1024).astype(np.float32) / 1024
            + (n & 255).astype(np.float32) / 4096
            + ((x >= 200) & (x < 320) & (y >= 140) & (y < 360)).astype(np.float32) * 0.75)


def full_size_case():
    width, height = 1536, 1024
    result = parallax.prepare_depth(full_size_disparity(), rgb_pattern(width, height),
                                   dilate_px=round(3 * width / 1600), blur_px=1.5 * width / 1600)
    # Stratified coverage, each edge, and both sides of the source rectangle's
    # scaled steps. Expectations remain tens of KB rather than a multi-MB map.
    points = {(y * height // 16, x * width // 32) for y in range(16) for x in range(32)}
    xs = {0, 1, 2, width // 2, width - 3, width - 2, width - 1}
    ys = {0, 1, 2, height // 2, height - 3, height - 2, height - 1}
    xs.update(round(x * width / 504) + d for x in (200, 320) for d in (-4, -1, 0, 1, 4))
    ys.update(round(y * height / 504) + d for y in (140, 360) for d in (-4, -1, 0, 1, 4))
    points.update((y, x) for y in ys for x in xs)
    indices = sorted(y * width + x for y, x in points)
    return dict(width=width, height=height, sample_indices=indices,
                values=floats(result.ravel()[indices]), median=float(np.median(result)))


def generate(destination):
    # Outliers exercise interpolated percentiles; a step and impulse exercise edges.
    source = np.array([-7, 2, 3, 3, 10, 4, 0, 7, 4, 8, 11, 90], np.float32).reshape(3, 4)
    normalized = parallax.normalize_disparity(source)
    resized = cv2.resize(normalized, (13, 9), interpolation=cv2.INTER_CUBIC)
    rgb = rgb_pattern(13, 9)
    gray = cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY).astype(np.float32) / 255
    guided = parallax.guided_filter(gray, resized, 2, 1e-3)
    clipped = np.clip(guided, 0, 1)
    dilated = cv2.dilate(clipped, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (7, 7)))
    blurred = cv2.GaussianBlur(dilated, (0, 0), 1.5)
    stages = dict(source=floats(source), normalized=floats(normalized),
                  resized=floats(resized), gray=floats(gray), guided=floats(guided),
                  dilated=floats(dilated), blurred=floats(blurred))
    cases = []
    for width, height in [(13, 9), (1, 11), (17, 1), (640, 4), (1600, 1)]:
        rgb = rgb_pattern(width, height)
        scale = max(width, height) / 1600
        result = parallax.prepare_depth(source, rgb, dilate_px=round(3 * scale), blur_px=1.5 * scale)
        if width * height <= 256:
            indices = list(range(width * height))
        else:
            # Keep boundary/central samples after processing every pixel. The
            # deterministic RGB pattern is recreated by the Rust test.
            xs = {0, 1, 2, 3, width // 4, width // 2, 3 * width // 4,
                  width - 4, width - 3, width - 2, width - 1}
            ys = {0, 1, height // 2, height - 2, height - 1}
            indices = sorted(y * width + x for y in ys for x in xs
                             if 0 <= x < width and 0 <= y < height)
        cases.append(dict(width=width, height=height, sample_indices=indices,
                          values=floats(result.ravel()[indices]), median=float(np.median(result))))
    data = dict(reference="tools/depth_spike/parallax.py; gamma=1; scaled dilation/blur",
                opencv=cv2.__version__, numpy=np.__version__, stages=stages, cases=cases,
                full_size=full_size_case())
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(data, separators=(",", ":")) + "\n", encoding="utf-8")


def model_oracle(model, output, image=None):
    import onnxruntime as ort
    # Same stretched path as check_static.py, with reproducible synthetic RGB.
    if image:
        import run_compare
        original = run_compare.load_rgb(image)
        source = str(image)
    else:
        original = rgb_pattern(640, 360)
        source = "synthetic rgb_pattern(640,360); no photograph"
    rgb = cv2.resize(original, (504, 504), interpolation=cv2.INTER_CUBIC)
    rgba = np.concatenate((rgb, np.full((504, 504, 1), 255, np.uint8)), axis=2)
    mean = np.array([0.485, 0.456, 0.406], np.float32)
    std = np.array([0.229, 0.224, 0.225], np.float32)
    x = ((rgb.astype(np.float32) / 255 - mean) / std).transpose(2, 0, 1)[None, None]
    options = ort.SessionOptions()
    options.enable_mem_pattern = False
    options.intra_op_num_threads = 4
    options.log_severity_level = 3
    session = ort.InferenceSession(str(model), options, providers=["DmlExecutionProvider", "CPUExecutionProvider"])
    depth, sky = session.run(["depth", "sky"], dict(
        image=x, extrinsics_in=np.full((1, 1, 4, 4), -1, np.float32),
        intrinsics_in=np.full((1, 1, 3, 3), -1, np.float32)))
    disparity = np.zeros((504, 504), np.float32)
    depth = depth[0, 0]
    valid = np.isfinite(depth) & (depth > 0) & (sky[0, 0] <= 0.5)
    np.divide(1, depth, out=disparity, where=valid)
    output.mkdir(parents=True, exist_ok=True)
    rgba.tofile(output / "reference-input.rgba")
    disparity.astype("<f4").tofile(output / "reference-disparity.f32")
    # Local-only full-image oracle. Match the production guide rule (long edge
    # <=1536 and never enlarge the original), then retain every expected pixel.
    scale = min(1.0, 1536 / max(original.shape[:2]))
    gh, gw = (round(original.shape[0] * scale), round(original.shape[1] * scale))
    guide = cv2.resize(original, (gw, gh), interpolation=cv2.INTER_CUBIC)
    guide_rgba = np.concatenate((guide, np.full((gh, gw, 1), 255, np.uint8)), axis=2)
    prepared = parallax.prepare_depth(disparity, guide,
                                     dilate_px=round(3 * max(gw, gh) / 1600),
                                     blur_px=1.5 * max(gw, gh) / 1600)
    guide_rgba.tofile(output / "reference-guide.rgba")
    prepared.astype("<f4").tofile(output / "reference-prepared.f32")
    (output / "reference.json").write_text(json.dumps(dict(
        width=504, height=504, source=source, source_width=original.shape[1], source_height=original.shape[0],
        image="reference-input.rgba", disparity="reference-disparity.f32",
        guide_width=gw, guide_height=gh, guide="reference-guide.rgba",
        prepared="reference-prepared.f32", median=float(np.median(prepared)),
        provider=session.get_providers(), reference="check_static.py stretched; cv2.INTER_CUBIC; sRGB RGBA; f32 LE disparity"), indent=2) + "\n", encoding="utf-8")
    print(f"Model oracle: disparity range {disparity.min()}..{disparity.max()}", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("tests/fixtures/depth/preprocess.json"))
    parser.add_argument("--model", type=Path)
    parser.add_argument("--oracle-dir", type=Path, default=Path("target/depth-addon"))
    parser.add_argument("--reference-image", type=Path, help="Optional local photo for the model oracle")
    args = parser.parse_args()
    cv2.setNumThreads(1)
    generate(args.output)
    if args.model:
        model_oracle(args.model, args.oracle_dir, args.reference_image)
