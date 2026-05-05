#!/usr/bin/env python3
"""Run a native-presenter CPU fallback pixel probe for one video.

This is intentionally not a screenshot test. It forces the dcomp presenter test
through software decode, asks NativeVideoPresenter to compare its CPU RGBA input
against the D3D11 BGRA backbuffer, then checks the perf JSONL for a successful
pixel_probe_match event.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


def read_events(path: Path) -> list[dict]:
    events: list[dict] = []
    if not path.exists():
        return events
    with path.open("r", encoding="utf-8", errors="replace") as f:
        for line in f:
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return events


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("video", type=Path)
    parser.add_argument("--exe", type=Path, default=Path("target/release/mimageviewer-core.exe"))
    parser.add_argument("--duration", type=float, default=2.0)
    parser.add_argument("--start", type=float, default=0.0)
    parser.add_argument("--window-size", default="640x360")
    parser.add_argument("--out", type=Path, default=Path("video-color-probe.jsonl"))
    args = parser.parse_args()

    if not args.exe.is_file():
        print(f"executable not found: {args.exe}", file=sys.stderr)
        return 2
    if not args.video.is_file():
        print(f"video not found: {args.video}", file=sys.stderr)
        return 2

    args.out.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        str(args.exe),
        "--dcomp-presenter-test",
        str(args.video),
        "--dcomp-duration",
        str(max(args.duration, 0.1)),
        "--dcomp-start",
        str(max(args.start, 0.0)),
        "--dcomp-window-size",
        args.window_size,
        "--dcomp-force-sw",
        "--dcomp-pixel-probe-strict",
        "--perf-log",
        str(args.out),
    ]
    proc = subprocess.run(cmd)

    events = read_events(args.out)
    matches = [
        e
        for e in events
        if e.get("cat") == "native_presenter"
        and e.get("kind") == "pixel_probe_match"
        and e.get("path") == "cpu_upload"
    ]
    mismatches = [
        e
        for e in events
        if e.get("cat") == "native_presenter" and e.get("kind") == "pixel_probe_mismatch"
    ]
    summaries = [
        e
        for e in events
        if e.get("cat") == "native_presenter" and e.get("kind") == "summary"
    ]
    latest_summary = summaries[-1] if summaries else {}
    cpu_frames = int(latest_summary.get("cpu_frames", 0) or 0)
    gpu_frames = int(latest_summary.get("gpu_frames", 0) or 0)

    if proc.returncode != 0:
        print(f"probe process failed with exit code {proc.returncode}; log={args.out}", file=sys.stderr)
        return proc.returncode
    if mismatches:
        print(f"pixel probe mismatch; log={args.out}", file=sys.stderr)
        return 1
    if not matches:
        print(f"no cpu_upload pixel_probe_match event; log={args.out}", file=sys.stderr)
        return 1
    if cpu_frames <= 0 or gpu_frames != 0:
        print(
            f"unexpected decode path: cpu_frames={cpu_frames} gpu_frames={gpu_frames}; log={args.out}",
            file=sys.stderr,
        )
        return 1

    sample = matches[-1]
    print(
        "OK cpu_upload pixel probe "
        f"BGRA=({sample.get('b')},{sample.get('g')},{sample.get('r')},{sample.get('a')}) "
        f"cpu_frames={cpu_frames} log={args.out}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
