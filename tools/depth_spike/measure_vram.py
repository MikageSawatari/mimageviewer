"""Depth-parallax spike: measure per-process GPU memory and timing of one model.

Usage:
  python measure_vram.py <spike_dir> <model-name> <device_id> [image]
Run one model per process so the numbers are not mixed. Memory comes from the
Windows "GPU Process Memory" performance counters of this process.
"""
from __future__ import annotations

import os
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import onnxruntime as ort

sys.path.insert(0, str(Path(__file__).parent))
import run_compare as rc  # noqa: E402


def gpu_mem_mb():
    pid = os.getpid()
    ps = (
        "$s = (Get-Counter -Counter '\\GPU Process Memory(pid_%d_*)\\Dedicated Usage',"
        "'\\GPU Process Memory(pid_%d_*)\\Shared Usage' -ErrorAction SilentlyContinue).CounterSamples;"
        "$d = ($s | ? { $_.Path -like '*dedicated*' } | Measure-Object CookedValue -Sum).Sum;"
        "$h = ($s | ? { $_.Path -like '*shared*' } | Measure-Object CookedValue -Sum).Sum;"
        "Write-Output \"$d $h\"" % (pid, pid)
    )
    out = subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True).stdout.split()
    return tuple(float(x) / 2**20 for x in out) if len(out) == 2 else (0.0, 0.0)  # no instance yet = no GPU memory


def main():
    ort.set_default_logger_severity(3)
    root = Path(sys.argv[1])
    name, device = sys.argv[2], int(sys.argv[3])
    image = sys.argv[4] if len(sys.argv) > 4 else "chef-with-trumpet"
    cls, rel = {n: (c, r) for n, c, r in rc.MODELS}[name.removesuffix("-fp16")]
    path = root / "models" / rel
    if name.endswith("-fp16"):
        path = path.with_name(path.stem + "_fp16.onnx")
    before = gpu_mem_mb()
    m = cls(name, path, "dml")
    m.providers = [("DmlExecutionProvider", {"device_id": device})]
    sample = next(p for p in (root / "samples").iterdir() if p.stem == image)
    rgb = rc.load_rgb(sample)
    t0 = time.perf_counter()
    m.run(rgb)  # session create + first run
    first = time.perf_counter() - t0
    times = []
    for _ in range(5):
        s = time.perf_counter()
        m.run(rgb)
        times.append(time.perf_counter() - s)
    after = gpu_mem_mb()
    print(
        f"{name}\tdev{device}\tload+first {first * 1000:.0f} ms\trun {np.median(times) * 1000:.0f} ms\t"
        f"dedicated {after[0] - before[0]:.0f} MB\tshared {after[1] - before[1]:.0f} MB"
    )


if __name__ == "__main__":
    main()
