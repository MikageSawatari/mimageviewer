#!/usr/bin/env python3
"""Run mIV playback soak tests, one process per video.

Example:
  python scripts/video_soak.py --exe target/release/mimageviewer.exe \
      --duration 30 --out-dir soak-logs D:/videos E:/more-videos

Each run writes a per-video perf JSONL and a Markdown summary.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path


VIDEO_EXTS = {
    ".3gp",
    ".avi",
    ".flv",
    ".m2ts",
    ".m4v",
    ".mkv",
    ".mov",
    ".mp4",
    ".mpeg",
    ".mpg",
    ".mts",
    ".ts",
    ".webm",
    ".wmv",
}


@dataclass
class Mode:
    name: str
    env: dict[str, str]


def safe_stem(path: Path, max_len: int = 80) -> str:
    raw = re.sub(r"[^A-Za-z0-9._-]+", "_", path.stem).strip("._")
    return (raw or "video")[:max_len]


def parse_mode(raw: str) -> Mode:
    if ":" not in raw:
        return Mode(raw, {})
    name, rest = raw.split(":", 1)
    env: dict[str, str] = {}
    for part in rest.split(","):
        if not part:
            continue
        if "=" not in part:
            raise SystemExit(f"invalid --mode entry {raw!r}: {part!r} is missing '='")
        key, value = part.split("=", 1)
        env[key] = value
    return Mode(name, env)


def iter_videos(roots: list[Path]) -> list[Path]:
    videos: list[Path] = []
    for root in roots:
        if root.is_file() and root.suffix.lower() in VIDEO_EXTS:
            videos.append(root)
            continue
        for path in root.rglob("*"):
            if path.is_file() and path.suffix.lower() in VIDEO_EXTS:
                videos.append(path)
    return videos


def analyze_perf(path: Path) -> dict[str, float | int]:
    counts: dict[str, int] = {}
    max_values: dict[str, float] = {}
    total = 0
    if not path.exists():
        return {"events": 0, "missing_log": 1}
    with path.open("r", encoding="utf-8", errors="replace") as f:
        for line in f:
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            total += 1
            cat = str(event.get("cat", ""))
            kind = str(event.get("kind", ""))
            key = f"{cat}/{kind}"
            counts[key] = counts.get(key, 0) + 1
            for field in ("gap_ms", "miss_ms", "lateness_ms", "ms", "total_ms"):
                value = event.get(field)
                if isinstance(value, (int, float)):
                    max_values[f"max_{field}"] = max(max_values.get(f"max_{field}", 0.0), float(value))
    return {
        "events": total,
        "display_miss": counts.get("video/display_miss", 0),
        "frame_gap": counts.get("ui/frame_gap", 0),
        "dropped_full": counts.get("video/dropped_full", 0),
        "dropped_past": counts.get("video/dropped_past", 0),
        "packet_wait": counts.get("demux/packet_send_wait", 0),
        "play_completed": counts.get("play_test/completed", 0),
        **max_values,
    }


def status_for(metrics: dict[str, float | int], proc_code: int | None, timed_out: bool) -> str:
    if timed_out:
        return "TIMEOUT"
    if proc_code not in (0, None):
        return f"EXIT_{proc_code}"
    if metrics.get("play_completed", 0) < 1:
        return "NO_COMPLETE"
    if metrics.get("dropped_full", 0) or metrics.get("dropped_past", 0):
        return "DROP"
    if metrics.get("max_gap_ms", 0.0) >= 100.0:
        return "HITCH"
    return "OK"


def run_one(
    exe: Path,
    video: Path,
    mode: Mode,
    out_dir: Path,
    index: int,
    duration: float,
    start: float | None,
    timeout: float,
    window_size: str,
    mute: bool,
    skip_vst3: bool,
) -> tuple[str, Path, dict[str, float | int], float]:
    log_path = out_dir / f"{index:04d}_{mode.name}_{safe_stem(video)}.jsonl"
    env = os.environ.copy()
    env.update(mode.env)
    cmd = [
        str(exe),
        "--play-test",
        str(video),
        "--play-duration",
        str(duration),
        "--perf-log",
        str(log_path),
        "--window-size",
        window_size,
    ]
    if start is not None:
        cmd.extend(["--play-test-start", str(start)])
    if mute:
        cmd.append("--play-muted")
    if skip_vst3:
        cmd.append("--play-test-skip-vst3")
    started = time.monotonic()
    timed_out = False
    try:
        proc = subprocess.run(cmd, env=env, timeout=timeout)
        code = proc.returncode
    except subprocess.TimeoutExpired:
        timed_out = True
        code = None
    elapsed = time.monotonic() - started
    metrics = analyze_perf(log_path)
    return status_for(metrics, code, timed_out), log_path, metrics, elapsed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("roots", nargs="+", type=Path, help="Folders or files to test")
    parser.add_argument("--exe", type=Path, default=Path("target/release/mimageviewer.exe"))
    parser.add_argument("--duration", type=float, default=30.0)
    parser.add_argument(
        "--start",
        type=float,
        default=0.0,
        help="Playback start position in seconds for --play-test (default: 0; use --resume to keep saved resume)",
    )
    parser.add_argument("--resume", action="store_true", help="Do not pass --play-test-start")
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("--out-dir", type=Path, default=Path("video-soak-results"))
    parser.add_argument("--limit", type=int, default=0, help="Maximum videos to run (0 = all)")
    parser.add_argument("--seed", type=int, default=None)
    parser.add_argument("--window-size", default="1280x720")
    parser.add_argument("--no-mute", action="store_true", help="Do not pass --play-muted")
    parser.add_argument(
        "--skip-vst3",
        action="store_true",
        help="Pass --play-test-skip-vst3 to isolate video playback from VST3 startup/processing",
    )
    parser.add_argument(
        "--mode",
        action="append",
        default=[],
        help="Mode name or name:ENV=VALUE,ENV2=VALUE2. Can be repeated.",
    )
    args = parser.parse_args()

    videos = iter_videos(args.roots)
    rng = random.Random(args.seed)
    rng.shuffle(videos)
    if args.limit > 0:
        videos = videos[: args.limit]
    if not videos:
        print("No videos found.", file=sys.stderr)
        return 2

    modes = [parse_mode(m) for m in args.mode] or [Mode("default", {})]
    start = None if args.resume else max(0.0, args.start)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    report_path = args.out_dir / "report.md"
    rows: list[str] = [
        "# mIV Video Soak Report",
        "",
        f"- videos: {len(videos)}",
        f"- modes: {', '.join(m.name for m in modes)}",
        f"- duration: {args.duration}s",
        "",
        "| status | mode | seconds | display_miss | frame_gap | drops | max_gap_ms | log | video |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |",
    ]

    failures = 0
    run_index = 0
    for video in videos:
        for mode in modes:
            run_index += 1
            status, log_path, metrics, elapsed = run_one(
                args.exe,
                video,
                mode,
                args.out_dir,
                run_index,
                args.duration,
                start,
                args.timeout,
                args.window_size,
                not args.no_mute,
                args.skip_vst3,
            )
            if status != "OK":
                failures += 1
            drops = int(metrics.get("dropped_full", 0)) + int(metrics.get("dropped_past", 0))
            row = (
                f"| {status} | {mode.name} | {elapsed:.1f} | "
                f"{int(metrics.get('display_miss', 0))} | "
                f"{int(metrics.get('frame_gap', 0))} | {drops} | "
                f"{float(metrics.get('max_gap_ms', 0.0)):.1f} | "
                f"`{log_path.name}` | `{video}` |"
            )
            print(row)
            rows.append(row)
            report_path.write_text("\n".join(rows) + "\n", encoding="utf-8")

    print(f"\nReport: {report_path}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
