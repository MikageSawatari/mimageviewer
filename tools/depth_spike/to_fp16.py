"""Depth-parallax spike: convert an ONNX model to fp16 weights (fp32 inputs/outputs).

Usage:
  python to_fp16.py <in.onnx> <out.onnx>
"""
import sys

import onnx
from onnxconverter_common import float16

src, dst = sys.argv[1], sys.argv[2]
model = onnx.load(src, load_external_data=True)
half = float16.convert_float_to_float16(model, keep_io_types=True, disable_shape_infer=True)
onnx.save(half, dst, save_as_external_data=False)
print("saved", dst)
