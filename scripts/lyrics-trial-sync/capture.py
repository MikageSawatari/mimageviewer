"""Capture a screen region and WASAPI loopback audio on the same QPC clock.

usage: python capture.py <seconds> <left> <top> <width> <height> <out_dir>
Writes frames.npz (t_ns, signatures) and loopback.{f32,csv}. Coordinates are physical pixels.
"""
import pathlib
import subprocess
import sys
import time

import mss
import numpy as np

HERE = pathlib.Path(__file__).resolve().parent


def main():
    seconds = float(sys.argv[1])
    left, top, width, height = map(int, sys.argv[2:6])
    out = pathlib.Path(sys.argv[6])
    out.mkdir(parents=True, exist_ok=True)
    recorder = subprocess.Popen([str(HERE / "loopback.exe"), str(seconds + 1.0), str(out / "loopback")])
    time.sleep(0.3)
    times, signatures = [], []
    region = {"left": left, "top": top, "width": width, "height": height}
    end = time.perf_counter_ns() + int(seconds * 1e9)
    with mss.mss() as grabber:
        while time.perf_counter_ns() < end:
            before = time.perf_counter_ns()
            shot = grabber.grab(region)
            after = time.perf_counter_ns()
            pixels = np.frombuffer(shot.raw, dtype=np.uint8).reshape(height, width, 4)
            gray = pixels[:, :, :3].max(axis=2)
            bright = gray[::2, ::2] > 170  # white text, not the soft glow
            times.append((before + after) // 2)
            signatures.append(np.packbits(bright))
    recorder.wait()
    np.savez_compressed(out / "frames.npz", t_ns=np.array(times, dtype=np.int64),
                        signatures=np.array(signatures), shape=np.array(bright.shape))
    intervals = np.diff(np.array(times)) / 1e6
    print(f"frames={len(times)} median_interval_ms={np.median(intervals):.2f} p95={np.percentile(intervals, 95):.2f}")


if __name__ == "__main__":
    main()
