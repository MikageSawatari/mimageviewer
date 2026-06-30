#!/usr/bin/env python3
"""Generate music_lab vocal teacher labels from a Demucs vocal stem.

This helper is intentionally outside the Rust application. It may use external
models and Python packages that are useful for teacher-data generation but are
not bundled with mIV.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate music_lab vocal interval JSON using Demucs htdemucs."
    )
    parser.add_argument("media", type=Path, help="Input audio/video file.")
    parser.add_argument(
        "--out",
        type=Path,
        required=True,
        help="Output music_lab label JSON.",
    )
    parser.add_argument(
        "--work-dir",
        type=Path,
        default=None,
        help="Temporary/output work directory. Defaults to the JSON parent.",
    )
    parser.add_argument(
        "--track-path",
        choices=["input", "extracted"],
        default="input",
        help=(
            "Path stored in labels.tracks[0].path. Use 'extracted' when running "
            "vocal_eval against the generated WAV."
        ),
    )
    parser.add_argument("--model", default="htdemucs", help="Demucs model name.")
    parser.add_argument("--sample-rate", type=int, default=44_100)
    parser.add_argument("--window", type=float, default=0.35)
    parser.add_argument("--hop", type=float, default=0.05)
    parser.add_argument("--vocal-db", type=float, default=-30.0)
    parser.add_argument("--ratio-db", type=float, default=-20.0)
    parser.add_argument("--mix-db", type=float, default=-45.0)
    parser.add_argument("--merge-gap", type=float, default=0.80)
    parser.add_argument("--min-segment", type=float, default=1.20)
    parser.add_argument("--boundary-ignore", type=float, default=0.35)
    parser.add_argument(
        "--reuse",
        action="store_true",
        help="Reuse extracted WAV / vocal stem if they already exist.",
    )
    return parser.parse_args()


def media_key(path: Path) -> str:
    resolved = str(path.resolve())
    return hashlib.sha1(resolved.encode("utf-8")).hexdigest()[:12]


def run_ffmpeg_extract(media: Path, mix_wav: Path, sample_rate: int, reuse: bool) -> None:
    if reuse and mix_wav.exists():
        print(f"reuse mix: {mix_wav}")
        return
    mix_wav.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        "ffmpeg",
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        str(media),
        "-vn",
        "-ac",
        "2",
        "-ar",
        str(sample_rate),
        str(mix_wav),
    ]
    print("extract:", " ".join(cmd))
    subprocess.run(cmd, check=True)


def run_demucs_vocals(mix_wav: Path, vocals_wav: Path, model_name: str, reuse: bool) -> None:
    if reuse and vocals_wav.exists():
        print(f"reuse vocals: {vocals_wav}")
        return

    # Imported lazily so --help and environment checks do not require Demucs.
    import torch as th
    from demucs.apply import apply_model
    from demucs.pretrained import get_model
    from demucs.separate import load_track

    t0 = time.perf_counter()
    model = get_model(name=model_name)
    model.cpu()
    model.eval()
    print(
        f"model loaded in {time.perf_counter() - t0:.2f}s; "
        f"sr={model.samplerate}; sources={model.sources}"
    )

    wav = load_track(mix_wav, model.audio_channels, model.samplerate)
    ref = wav.mean(0)
    wav = (wav - ref.mean()) / ref.std()
    with th.no_grad():
        t1 = time.perf_counter()
        sources = apply_model(
            model,
            wav[None],
            device="cpu",
            shifts=0,
            split=True,
            overlap=0.25,
            progress=True,
            num_workers=1,
        )[0]
        print(f"demucs inference done in {time.perf_counter() - t1:.2f}s")

    sources = sources * ref.std() + ref.mean()
    vocal_idx = model.sources.index("vocals")
    vocals = sources[vocal_idx].detach().cpu().numpy().T
    vocals_wav.parent.mkdir(parents=True, exist_ok=True)
    sf.write(vocals_wav, vocals, model.samplerate)
    print(f"wrote vocals: {vocals_wav}")


def moving_mean(values: np.ndarray, radius: int) -> np.ndarray:
    if radius <= 0:
        return values.copy()
    kernel = np.ones(radius * 2 + 1, dtype=np.float32) / float(radius * 2 + 1)
    return np.convolve(values, kernel, mode="same")


def mask_to_segments(
    times: np.ndarray,
    active: np.ndarray,
    duration: float,
    hop: float,
    merge_gap: float,
    min_len: float,
) -> list[list[float]]:
    segments: list[list[float]] = []
    start: float | None = None
    last: float | None = None
    for time_sec, is_active in zip(times, active):
        if bool(is_active):
            if start is None:
                start = float(time_sec - hop * 0.5)
            last = float(time_sec + hop * 0.5)
        elif start is not None:
            segments.append([max(0.0, start), min(duration, last or float(time_sec))])
            start = None
            last = None
    if start is not None:
        segments.append([max(0.0, start), min(duration, last or duration)])
    if not segments:
        return []

    merged = [segments[0]]
    for start, end in segments[1:]:
        if start - merged[-1][1] <= merge_gap:
            merged[-1][1] = end
        else:
            merged.append([start, end])
    return [[start, end] for start, end in merged if end - start >= min_len]


def build_labels(args: argparse.Namespace, mix_wav: Path, vocals_wav: Path) -> dict:
    mix, sr = sf.read(mix_wav, always_2d=True, dtype="float32")
    vocals, vocals_sr = sf.read(vocals_wav, always_2d=True, dtype="float32")
    if sr != vocals_sr:
        raise RuntimeError(f"sample rate mismatch: mix={sr}, vocals={vocals_sr}")

    sample_count = min(len(mix), len(vocals))
    mix_mono = mix[:sample_count].mean(axis=1)
    vocals_mono = vocals[:sample_count].mean(axis=1)
    duration = sample_count / sr
    win = max(1, int(sr * args.window))
    hop = max(1, int(sr * args.hop))
    times: list[float] = []
    vocal_rms: list[float] = []
    mix_rms: list[float] = []
    for start in range(0, max(1, sample_count - win + 1), hop):
        end = min(sample_count, start + win)
        if end - start < win // 3:
            break
        vocal_frame = vocals_mono[start:end]
        mix_frame = mix_mono[start:end]
        vocal_rms.append(float(np.sqrt(np.mean(vocal_frame * vocal_frame) + 1e-12)))
        mix_rms.append(float(np.sqrt(np.mean(mix_frame * mix_frame) + 1e-12)))
        times.append((start + (end - start) * 0.5) / sr)

    times_np = np.asarray(times, dtype=np.float32)
    vocal_np = np.asarray(vocal_rms, dtype=np.float32)
    mix_np = np.asarray(mix_rms, dtype=np.float32)
    vocal_ref = float(np.percentile(vocal_np, 95)) + 1e-12
    mix_ref = float(np.percentile(mix_np, 95)) + 1e-12
    vocal_db = 20.0 * np.log10(vocal_np / vocal_ref + 1e-12)
    mix_db = 20.0 * np.log10(mix_np / mix_ref + 1e-12)
    ratio_db = 20.0 * np.log10(vocal_np / (mix_np + 1e-12) + 1e-12)

    smooth_radius = max(1, round(0.15 / args.hop))
    smooth_vocal_db = moving_mean(vocal_db, smooth_radius)
    smooth_ratio_db = moving_mean(ratio_db, smooth_radius)
    active = (
        (smooth_vocal_db > args.vocal_db)
        & (smooth_ratio_db > args.ratio_db)
        & (mix_db > args.mix_db)
    )
    segments = mask_to_segments(
        times_np,
        active,
        duration,
        args.hop,
        args.merge_gap,
        args.min_segment,
    )

    ignore = []
    for start, end in segments:
        ignore.append(
            {
                "start": round(max(0.0, start - args.boundary_ignore), 3),
                "end": round(min(duration, start + args.boundary_ignore), 3),
            }
        )
        ignore.append(
            {
                "start": round(max(0.0, end - args.boundary_ignore), 3),
                "end": round(min(duration, end + args.boundary_ignore), 3),
            }
        )

    track_path = mix_wav if args.track_path == "extracted" else args.media
    labels = {
        "metadata": {
            "teacher": f"Demucs {args.model} vocal stem",
            "generated_by": "tools/music_lab/scripts/demucs_vocal_teacher.py",
            "note": "External teacher data only; model/runtime are not bundled with mIV.",
            "source_media": str(args.media),
            "evaluation_audio": str(mix_wav),
            "vocal_stem": str(vocals_wav),
            "window_seconds": args.window,
            "hop_seconds": args.hop,
            "vocal_db_threshold": args.vocal_db,
            "vocal_to_mix_ratio_db_threshold": args.ratio_db,
            "mix_db_threshold": args.mix_db,
            "merge_gap_seconds": args.merge_gap,
            "min_segment_seconds": args.min_segment,
            "boundary_ignore_seconds": args.boundary_ignore,
        },
        "tracks": [
            {
                "path": str(track_path),
                "vocal": [
                    {"start": round(start, 3), "end": round(end, 3)}
                    for start, end in segments
                ],
                "ignore": ignore,
            }
        ],
    }
    total = sum(end - start for start, end in segments)
    print(f"segments={len(segments)} vocal_total={total:.2f}s duration={duration:.2f}s")
    for start, end in segments:
        print(f"  {start:8.3f} - {end:8.3f}  {end - start:6.3f}s")
    return labels


def main() -> int:
    args = parse_args()
    args.media = args.media.resolve()
    args.out = args.out.resolve()
    work_dir = args.work_dir.resolve() if args.work_dir else args.out.parent
    key = media_key(args.media)
    mix_wav = work_dir / f"{key}_mix_{args.sample_rate}.wav"
    vocals_wav = work_dir / f"{key}_{args.model}_vocals.wav"

    run_ffmpeg_extract(args.media, mix_wav, args.sample_rate, args.reuse)
    run_demucs_vocals(mix_wav, vocals_wav, args.model, args.reuse)
    labels = build_labels(args, mix_wav, vocals_wav)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(labels, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"wrote labels: {args.out}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except subprocess.CalledProcessError as exc:
        print(f"command failed with exit code {exc.returncode}: {exc.cmd}", file=sys.stderr)
        raise SystemExit(exc.returncode)
