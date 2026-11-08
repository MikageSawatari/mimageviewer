"""Depth-parallax spike: compare fp16 and fp32 DA3 Mono-Large outputs on DirectML."""
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort

sys.path.insert(0, str(Path(__file__).parent))
import parallax  # noqa: E402
import run_compare as rc  # noqa: E402

ort.set_default_logger_severity(3)
root = Path(sys.argv[1])
d = root / "models" / "da3monol"
fp32 = rc.DA3MonoLarge("fp32", d / "da3mono_large.onnx", "dml")
fp16 = rc.DA3MonoLarge("fp16", d / "da3mono_large_fp16.onnx", "dml")
for stem in sys.argv[2:]:
    rgb = rc.load_rgb(next(p for p in (root / "samples").iterdir() if p.stem == stem))
    a = parallax.normalize_disparity(fp32.run(rgb))
    b = parallax.normalize_disparity(fp16.run(rgb))
    diff = np.abs(a - b)
    print(f"{stem[:30]:30s} corr {np.corrcoef(a.ravel(), b.ravel())[0, 1]:.5f} "
          f"mean {diff.mean():.4f} p99 {np.percentile(diff, 99):.4f} max {diff.max():.4f}")
