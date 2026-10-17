"""Pair beep onsets (loopback, QPC) with lyric line changes (screen, QPC) and report the offset.

usage: python analyze.py <run_dir>
offset = display change time - audio onset time. Positive: lyrics appear after the sound reaches
the Windows mixer. Device output latency after the mixer is not included.
"""
import pathlib
import sys

import numpy as np


def load_audio(run):
    lines = (run / "loopback.csv").read_text().splitlines()
    header = dict(part.split("=") for part in lines[0][2:].split())
    rate, channels = int(header["rate"]), int(header["channels"])
    packets = np.array([[int(v) for v in line.split(",")] for line in lines[2:]], dtype=np.int64)
    samples = np.fromfile(run / "loopback.f32", dtype=np.float32).reshape(-1, channels)
    return rate, packets, np.abs(samples).max(axis=1)


def beep_onsets(rate, packets, level):
    above = level > 0.05
    onsets = []
    quiet = int(0.5 * rate)
    last = -quiet
    for index in np.flatnonzero(above):
        if index - last >= quiet:
            onsets.append(index)
        last = index
    # frame index -> QPC (100 ns) via the packet that contains it
    starts = packets[:, 0]
    times = []
    for frame in onsets:
        packet = np.searchsorted(starts, frame, side="right") - 1
        qpc = packets[packet, 2] + (frame - starts[packet]) * 1e7 / rate
        times.append(qpc * 100)  # ns
    return np.array(times)


def display_changes(run):
    data = np.load(run / "frames.npz")
    times, signatures = data["t_ns"], data["signatures"]
    bits = np.unpackbits(signatures, axis=1)
    diff = (bits[1:] != bits[:-1]).sum(axis=1)
    threshold = max(20, int(diff.max() * 0.3))
    changes = []
    last = -10**18
    for index in np.flatnonzero(diff > threshold):
        t = times[index + 1]
        if t - last > 0.5e9:
            changes.append((times[index] + t) / 2)  # change happened between the two frames
        last = t
    interval = np.median(np.diff(times)) / 1e6
    return np.array(changes), interval, threshold


def main():
    run = pathlib.Path(sys.argv[1])
    rate, packets, level = load_audio(run)
    beeps = beep_onsets(rate, packets, level)
    changes, interval, threshold = display_changes(run)
    print(f"beeps={len(beeps)} changes={len(changes)} frame_interval_ms={interval:.2f} threshold={threshold}")
    offsets = []
    for change in changes:
        nearest = beeps[np.argmin(np.abs(beeps - change))] if len(beeps) else None
        if nearest is not None and abs(change - nearest) < 0.9e9:
            offsets.append((change - nearest) / 1e6)
    offsets = np.array(offsets)
    if len(offsets):
        print(f"paired={len(offsets)} offset_ms median={np.median(offsets):.1f} mean={offsets.mean():.1f} "
              f"min={offsets.min():.1f} max={offsets.max():.1f} p10={np.percentile(offsets, 10):.1f} "
              f"p90={np.percentile(offsets, 90):.1f}")
        print("offsets_ms:", " ".join(f"{value:.0f}" for value in offsets))


if __name__ == "__main__":
    main()
