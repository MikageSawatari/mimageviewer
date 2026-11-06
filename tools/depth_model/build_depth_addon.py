"""Build the independently distributed DA3 Mono-Large depth addon.

Uses an already downloaded SpatialHub fp32 ONNX (including its external data).
No network access is performed. See README.md for the reproducible invocation.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from pathlib import Path

import onnx
from onnxconverter_common import float16

MODEL_NAME = "da3mono_large_fp16_504.onnx"
INPUT_SHAPES = {
    "image": (1, 1, 3, 504, 504),
    "extrinsics_in": (1, 1, 4, 4),
    "intrinsics_in": (1, 1, 3, 3),
}
NOTICE = """Depth Anything 3 Mono-Large (DA3-MONO-LARGE)
Original model: https://huggingface.co/depth-anything/DA3-MONO-LARGE
Original project: https://github.com/ByteDance-Seed/Depth-Anything-3
ONNX export: https://huggingface.co/SpatialHub/depth-anything-3-onnx
Source file: da3mono_large.onnx (with its external tensor data)
License: Apache License, Version 2.0 (see LICENSE).

Modified for mImageViewer: converted fp32 weights and internal computation to
fp16 using onnxconverter-common, preserving fp32 inputs and outputs, with shape
inference disabled during conversion. Fixed all graph inputs to one view and
504 x 504 pixels, and embedded external tensors into a single ONNX file.
No retraining was performed.
"""


def file_entry(path: Path, kind: str) -> dict:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return dict(name=path.name, kind=kind, bytes=path.stat().st_size,
                sha256=digest.hexdigest(), license="Apache-2.0", parts=[])


def build(source: Path, output: Path) -> dict:
    output.mkdir(parents=True, exist_ok=True)
    destination = output / MODEL_NAME
    if source.resolve() == destination.resolve():
        raise ValueError("Input and output ONNX must be different files")
    model = onnx.load(str(source), load_external_data=True)
    if {value.name for value in model.graph.input} != set(INPUT_SHAPES):
        raise ValueError("Expected the SpatialHub DA3 Mono-Large three-input export")
    model = float16.convert_float_to_float16(
        model, keep_io_types=True, disable_shape_infer=True)
    for value in model.graph.input:
        dimensions = value.type.tensor_type.shape.dim
        shape = INPUT_SHAPES[value.name]
        if len(dimensions) != len(shape):
            raise ValueError(f"Unexpected input rank: {value.name}")
        for dimension, size in zip(dimensions, shape):
            dimension.dim_value = size  # replaces the symbolic dimension too
    onnx.save_model(model, str(destination), save_as_external_data=False)
    del model
    shutil.copyfile(Path(__file__).parents[2] / "vendor/egui/LICENSE-APACHE", output / "LICENSE")
    (output / "NOTICE").write_text(NOTICE, encoding="utf-8", newline="\n")
    files = [file_entry(output / name, kind) for name, kind in
             [(MODEL_NAME, "model"), ("LICENSE", "license"), ("NOTICE", "notice")]]
    index = dict(schema=1, model_id=f"da3mono-large-fp16-504-{files[0]['sha256'][:12]}", files=files)
    (output / "depth-model-index.json").write_text(
        json.dumps(index, indent=2) + "\n", encoding="utf-8", newline="\n")
    return index


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path, help="Original fp32 ONNX path")
    parser.add_argument("--output", required=True, type=Path, help="Addon assets directory")
    args = parser.parse_args()
    print(json.dumps(build(args.input, args.output), indent=2), flush=True)
