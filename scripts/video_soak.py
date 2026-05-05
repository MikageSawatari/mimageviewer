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


def percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    idx = min(len(ordered) - 1, max(0, round((len(ordered) - 1) * pct)))
    return ordered[idx]


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
    native_summary: dict[str, float | int] = {}
    overlay_render_total_ms = 0.0
    overlay_render_count = 0
    overlay_input_presents = 0
    overlay_idle_presents = 0
    overlay_prev_t: float | None = None
    overlay_max_interval_ms = 0.0
    present_copy_ms: list[float] = []
    gpu_copy_ms: list[float] = []
    cpu_copy_ms: list[float] = []
    present_fence_wait_ms: list[float] = []
    present_open_shared_ms: list[float] = []
    present_keyed_mutex_ms: list[float] = []
    present_keyed_mutex_cast_ms: list[float] = []
    present_keyed_mutex_acquire_ms: list[float] = []
    present_copy_call_ms: list[float] = []
    present_total_ms: list[float] = []
    present_shared_handles: set[int] = set()
    present_shared_cache_hits = 0
    present_shared_cache_misses = 0
    shared_output_recover_ms: list[float] = []
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
            if cat == "native_presenter" and kind == "egui_overlay_present":
                t = event.get("t")
                if isinstance(t, (int, float)):
                    if overlay_prev_t is not None:
                        overlay_max_interval_ms = max(
                            overlay_max_interval_ms, (float(t) - overlay_prev_t) * 1000.0
                        )
                    overlay_prev_t = float(t)
                render_ms = event.get("render_ms")
                if isinstance(render_ms, (int, float)):
                    overlay_render_count += 1
                    overlay_render_total_ms += float(render_ms)
                    max_values["max_overlay_render_ms"] = max(
                        max_values.get("max_overlay_render_ms", 0.0), float(render_ms)
                    )
                input_events = int(event.get("input_events", 0) or 0)
                native_events = int(event.get("native_events", 0) or 0)
                if input_events or native_events:
                    overlay_input_presents += 1
                else:
                    overlay_idle_presents += 1
            if cat == "native_presenter" and kind == "summary":
                for field in (
                    "presented",
                    "gpu_frames",
                    "cpu_frames",
                    "late_drop",
                    "wait_timeout",
                    "actual_fps",
                    "max_late_ms",
                    "max_total_ms",
                    "max_interval_ms",
                ):
                    value = event.get(field)
                    if isinstance(value, (int, float)):
                        native_summary[f"native_{field}"] = value
            if cat == "native_presenter" and kind == "fullscreen_present":
                path_value = str(event.get("path", ""))
                shared_handle = event.get("shared_handle")
                if isinstance(shared_handle, int) and shared_handle:
                    present_shared_handles.add(shared_handle)
                shared_cache_hit = event.get("shared_cache_hit")
                if shared_cache_hit is True:
                    present_shared_cache_hits += 1
                elif shared_cache_hit is False and path_value == "d3d11_shared":
                    present_shared_cache_misses += 1
                copy_ms = event.get("copy_ms")
                if isinstance(copy_ms, (int, float)):
                    copy_ms_f = float(copy_ms)
                    present_copy_ms.append(copy_ms_f)
                    if path_value == "d3d11_shared":
                        gpu_copy_ms.append(copy_ms_f)
                    elif path_value == "cpu_upload":
                        cpu_copy_ms.append(copy_ms_f)
                fence_wait_ms = event.get("fence_wait_ms")
                if isinstance(fence_wait_ms, (int, float)):
                    present_fence_wait_ms.append(float(fence_wait_ms))
                open_shared_ms = event.get("open_shared_ms")
                if isinstance(open_shared_ms, (int, float)):
                    present_open_shared_ms.append(float(open_shared_ms))
                keyed_mutex_ms = event.get("keyed_mutex_ms")
                if isinstance(keyed_mutex_ms, (int, float)):
                    present_keyed_mutex_ms.append(float(keyed_mutex_ms))
                keyed_mutex_cast_ms = event.get("keyed_mutex_cast_ms")
                if isinstance(keyed_mutex_cast_ms, (int, float)):
                    present_keyed_mutex_cast_ms.append(float(keyed_mutex_cast_ms))
                keyed_mutex_acquire_ms = event.get("keyed_mutex_acquire_ms")
                if isinstance(keyed_mutex_acquire_ms, (int, float)):
                    present_keyed_mutex_acquire_ms.append(float(keyed_mutex_acquire_ms))
                copy_call_ms = event.get("copy_call_ms")
                if isinstance(copy_call_ms, (int, float)):
                    present_copy_call_ms.append(float(copy_call_ms))
                total_ms = event.get("total_ms")
                if isinstance(total_ms, (int, float)):
                    present_total_ms.append(float(total_ms))
            if cat == "video" and kind == "shared_output_keyed_mutex_recovered":
                recover_ms = event.get("recover_ms")
                if isinstance(recover_ms, (int, float)):
                    shared_output_recover_ms.append(float(recover_ms))
    return {
        "events": total,
        "display_miss": counts.get("video/display_miss", 0),
        "frame_gap": counts.get("ui/frame_gap", 0),
        "dropped_full": counts.get("video/dropped_full", 0),
        "dropped_past": counts.get("video/dropped_past", 0),
        "packet_wait": counts.get("demux/packet_send_wait", 0),
        "play_completed": counts.get("play_test/completed", 0),
        "native_present": counts.get("native_presenter/present", 0),
        "native_late_drop": counts.get("native_presenter/late_drop", 0),
        "native_summary": counts.get("native_presenter/summary", 0),
        "overlay_present": counts.get("native_presenter/egui_overlay_present", 0),
        "overlay_input_present": overlay_input_presents,
        "overlay_idle_present": overlay_idle_presents,
        "overlay_avg_render_ms": (
            overlay_render_total_ms / overlay_render_count if overlay_render_count else 0.0
        ),
        "overlay_max_interval_ms": overlay_max_interval_ms,
        "native_present_logged": counts.get("native_presenter/fullscreen_present", 0),
        "native_shared_handle_unique": len(present_shared_handles),
        "native_shared_cache_hits": present_shared_cache_hits,
        "native_shared_cache_misses": present_shared_cache_misses,
        "native_shared_output_recover_count": counts.get(
            "video/shared_output_keyed_mutex_recovered", 0
        ),
        "native_shared_output_recover_ms_p95": percentile(shared_output_recover_ms, 0.95),
        "native_shared_output_recover_ms_max": max(shared_output_recover_ms, default=0.0),
        "native_shared_output_acquire_timeout": counts.get("video/shared_output_acquire_timeout", 0),
        "native_shared_output_reset_failed": counts.get(
            "video/shared_output_unpresented_reset_failed", 0
        ),
        "native_copy_ms_p50": percentile(present_copy_ms, 0.50),
        "native_copy_ms_p95": percentile(present_copy_ms, 0.95),
        "native_copy_ms_max": max(present_copy_ms, default=0.0),
        "native_gpu_copy_ms_p95": percentile(gpu_copy_ms, 0.95),
        "native_gpu_copy_ms_max": max(gpu_copy_ms, default=0.0),
        "native_cpu_copy_ms_p95": percentile(cpu_copy_ms, 0.95),
        "native_cpu_copy_ms_max": max(cpu_copy_ms, default=0.0),
        "native_fence_wait_ms_p95": percentile(present_fence_wait_ms, 0.95),
        "native_fence_wait_ms_max": max(present_fence_wait_ms, default=0.0),
        "native_open_shared_ms_p95": percentile(present_open_shared_ms, 0.95),
        "native_open_shared_ms_max": max(present_open_shared_ms, default=0.0),
        "native_keyed_mutex_ms_p95": percentile(present_keyed_mutex_ms, 0.95),
        "native_keyed_mutex_ms_max": max(present_keyed_mutex_ms, default=0.0),
        "native_keyed_mutex_cast_ms_p95": percentile(present_keyed_mutex_cast_ms, 0.95),
        "native_keyed_mutex_cast_ms_max": max(present_keyed_mutex_cast_ms, default=0.0),
        "native_keyed_mutex_acquire_ms_p95": percentile(present_keyed_mutex_acquire_ms, 0.95),
        "native_keyed_mutex_acquire_ms_max": max(present_keyed_mutex_acquire_ms, default=0.0),
        "native_copy_call_ms_p95": percentile(present_copy_call_ms, 0.95),
        "native_copy_call_ms_max": max(present_copy_call_ms, default=0.0),
        "native_total_ms_p95": percentile(present_total_ms, 0.95),
        "native_total_ms_max": max(present_total_ms, default=0.0),
        **native_summary,
        **max_values,
    }


def status_for(metrics: dict[str, float | int], proc_code: int | None, timed_out: bool) -> str:
    if timed_out:
        return "TIMEOUT"
    if proc_code not in (0, None):
        return f"EXIT_{proc_code}"
    if metrics.get("native_summary", 0) >= 1:
        if metrics.get("native_late_drop", 0):
            return "DROP"
        if metrics.get("native_wait_timeout", 0):
            return "WAIT"
        if metrics.get("native_presented", 0) < 1:
            return "NO_PRESENT"
        return "OK"
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
    dcomp_presenter: bool,
    dcomp_sync_interval: int,
) -> tuple[str, Path, dict[str, float | int], float]:
    log_path = out_dir / f"{index:04d}_{mode.name}_{safe_stem(video)}.jsonl"
    env = os.environ.copy()
    env.update(mode.env)
    if dcomp_presenter:
        cmd = [
            str(exe),
            "--dcomp-presenter-test",
            str(video),
            "--dcomp-duration",
            str(duration),
            "--perf-log",
            str(log_path),
            "--dcomp-window-size",
            window_size,
            "--dcomp-sync-interval",
            str(dcomp_sync_interval),
        ]
        if start is not None:
            cmd.extend(["--dcomp-start", str(start)])
    else:
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
        "--dcomp-presenter",
        action="store_true",
        help="Run --dcomp-presenter-test instead of the normal egui fullscreen --play-test path",
    )
    parser.add_argument(
        "--dcomp-sync-interval",
        type=int,
        default=1,
        help="Sync interval passed to --dcomp-presenter-test (default: 1)",
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
        "| status | mode | seconds | display_miss | frame_gap | drops | max_gap_ms | overlay_present | overlay_max_render_ms | overlay_max_interval_ms | native_present_samples | native_shared_handles | native_cache_hits | native_cache_misses | native_copy_p95_ms | native_copy_max_ms | native_fence_max_ms | native_keyed_acq_max_ms | native_recover_count | native_recover_max_ms | log | video |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |",
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
                args.dcomp_presenter,
                args.dcomp_sync_interval,
            )
            if status != "OK":
                failures += 1
            drops = int(metrics.get("dropped_full", 0)) + int(metrics.get("dropped_past", 0))
            display_miss = int(metrics.get("display_miss", 0))
            frame_gap = int(metrics.get("frame_gap", 0))
            max_gap = float(metrics.get("max_gap_ms", 0.0))
            if args.dcomp_presenter:
                display_miss = int(metrics.get("native_presented", metrics.get("native_present", 0)))
                frame_gap = int(metrics.get("native_late_drop", 0))
                max_gap = float(metrics.get("native_max_interval_ms", 0.0))
            overlay_present = int(metrics.get("overlay_present", 0))
            overlay_max_render_ms = float(metrics.get("max_overlay_render_ms", 0.0))
            overlay_max_interval_ms = float(metrics.get("overlay_max_interval_ms", 0.0))
            native_present_logged = int(metrics.get("native_present_logged", 0))
            native_shared_handles = int(metrics.get("native_shared_handle_unique", 0))
            native_cache_hits = int(metrics.get("native_shared_cache_hits", 0))
            native_cache_misses = int(metrics.get("native_shared_cache_misses", 0))
            native_copy_p95 = float(metrics.get("native_copy_ms_p95", 0.0))
            native_copy_max = float(metrics.get("native_copy_ms_max", 0.0))
            native_fence_max = float(metrics.get("native_fence_wait_ms_max", 0.0))
            native_keyed_acquire_max = float(metrics.get("native_keyed_mutex_acquire_ms_max", 0.0))
            native_recover_count = int(metrics.get("native_shared_output_recover_count", 0))
            native_recover_max = float(metrics.get("native_shared_output_recover_ms_max", 0.0))
            row = (
                f"| {status} | {mode.name} | {elapsed:.1f} | "
                f"{display_miss} | "
                f"{frame_gap} | {drops} | "
                f"{max_gap:.1f} | "
                f"{overlay_present} | "
                f"{overlay_max_render_ms:.2f} | "
                f"{overlay_max_interval_ms:.1f} | "
                f"{native_present_logged} | "
                f"{native_shared_handles} | "
                f"{native_cache_hits} | "
                f"{native_cache_misses} | "
                f"{native_copy_p95:.2f} | "
                f"{native_copy_max:.2f} | "
                f"{native_fence_max:.2f} | "
                f"{native_keyed_acquire_max:.2f} | "
                f"{native_recover_count} | "
                f"{native_recover_max:.2f} | "
                f"`{log_path.name}` | `{video}` |"
            )
            print(row)
            rows.append(row)
            report_path.write_text("\n".join(rows) + "\n", encoding="utf-8")

    print(f"\nReport: {report_path}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
