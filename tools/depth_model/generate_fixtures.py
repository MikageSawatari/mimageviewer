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
                opencv=cv2.__version__, numpy=np.__version__, stages=stages, cases=cases)
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
    (output / "reference.json").write_text(json.dumps(dict(
        width=504, height=504, source=source, source_width=original.shape[1], source_height=original.shape[0],
        image="reference-input.rgba", disparity="reference-disparity.f32",
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
