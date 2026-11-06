# Depth addon preparation

These scripts use local files only and do not publish or download anything.
The input is SpatialHub's original `da3mono_large.onnx`, with its external
tensor file beside it. Conversion preserves float32 input/output, converts
internal weights/calculation to float16 without shape inference, fixes every
input shape (one 504 × 504 view), and embeds tensors in one ONNX.

```powershell
& .\target\depth-spike\venv\Scripts\python.exe tools/depth_model/build_depth_addon.py --input target/depth-spike/models/da3monol/da3mono_large.onnx --output target/depth-addon
```

Outputs: `da3mono_large_fp16_504.onnx`, Apache-2.0 `LICENSE` (the complete
local copy in `vendor/egui/LICENSE-APACHE`), `NOTICE` identifying the sources
and modifications, and `depth-model-index.json`. The index uses the shared
`ModelIndex` schema 1, with one `model`, one `license`, and one `notice` entry;
each records bytes, SHA-256, `Apache-2.0`, and an empty parts list. The model
ID is `da3mono-large-fp16-504-<first 12 model hash characters>`.

Validated environment: Python 3.13, onnx 1.23.2, onnxconverter-common 1.16.0,
onnxruntime-directml 1.24.4, numpy 2.5.4, opencv-python-headless 5.0.0.93.
Exact package pins are in `requirements.txt`. Converter
warnings about small numbers being clamped to float16 bounds are expected.

Run the builder's small structural tests with
`python -m unittest discover -s tools/depth_model -p test_build_depth_addon.py`.

Regenerate the checked-in synthetic numerical fixtures (no model required):

```powershell
& .\target\depth-spike\venv\Scripts\python.exe tools/depth_model/generate_fixtures.py
```

The generator imports `depth_spike.parallax`, captures individual stages, and
compares the complete pipeline at several sizes, including single-row/column
edges and long-edge scaling of dilation/blur. Gamma remains 1. The Rust tests
allow float32 rounding differences and final half-precision quantization.

To also create a local model inference oracle for Rust/Python comparison:

```powershell
& .\target\depth-spike\venv\Scripts\python.exe tools/depth_model/generate_fixtures.py --model target/depth-addon/da3mono_large_fp16_504.onnx
```

`target/depth-addon/reference.json` describes a 504 × 504 synthetic RGBA8 image
(`reference-input.rgba`) and matching raw little-endian float32 disparity
(`reference-disparity.f32`). This stretches a 640 × 360 pattern with bicubic
interpolation, using the stretched path from `check_static.py`,
ImageNet normalization, unknown cameras filled with -1, sky > 0.5 masked to 0,
and reciprocal finite positive depth. Oracle assets are local verification
artifacts and are **not** release assets.
To compare an existing photo instead, add
`--reference-image target/depth-spike/samples/IMG_0001.JPG`. This replaces only
the ignored local oracle files, preserving the checked-in synthetic fixtures.
