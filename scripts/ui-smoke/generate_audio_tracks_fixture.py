#!/usr/bin/env python3
"""Generate small synthetic audio-track fixtures using the system ffmpeg."""

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path


DEFAULT_OUTPUT = Path(__file__).resolve().parents[2] / "testdata" / "audio-tracks"
VIDEO_SOURCE = "testsrc2=size=160x90:rate=10:duration=6"
VIDEO_OPTIONS = ["-c:v", "mpeg4", "-q:v", "5"]


def run_ffmpeg(output: Path, *arguments: str) -> None:
    subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *arguments, str(output)],
        check=True,
    )


def generate_multi_timebase(output: Path) -> None:
    run_ffmpeg(
        output / "multi-timebase.mp4",
        "-f", "lavfi", "-i", VIDEO_SOURCE,
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=6",
        "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=44100:duration=6",
        "-f", "lavfi", "-i", "sine=frequency=1320:sample_rate=32000:duration=6",
        "-filter_complex",
        "[1:a]volume=6dB,aformat=channel_layouts=stereo[a0];"
        "[2:a]volume=0dB,aformat=channel_layouts=5.1[a1];"
        "[3:a]volume=-6dB[a2]",
        "-map", "0:v", "-map", "[a0]", "-map", "[a1]", "-map", "[a2]",
        *VIDEO_OPTIONS,
        "-c:a", "aac",
        "-b:a:0", "48k", "-b:a:1", "128k", "-b:a:2", "48k",
        # The MP4 muxer exposes these as `name` tags, not `title` in ffprobe.
        "-metadata:s:a:0", "language=jpn",
        "-metadata:s:a:0", "title=日本語 440Hz",
        "-metadata:s:a:1", "language=eng",
        "-metadata:s:a:1", "title=English 880Hz",
        "-disposition:a:0", "0",
        "-disposition:a:1", "default",
        "-disposition:a:2", "0",
        "-shortest",
    )


def generate(output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    run_ffmpeg(
        output / "multi.mkv",
        "-f", "lavfi", "-i", VIDEO_SOURCE,
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=6",
        "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=44100:duration=6",
        "-f", "lavfi", "-i", "sine=frequency=1320:sample_rate=32000:duration=6",
        "-filter_complex",
        "[1:a]volume=6dB,aformat=channel_layouts=stereo[a0];"
        "[2:a]volume=0dB,aformat=channel_layouts=5.1[a1];"
        "[3:a]volume=-6dB[a2]",
        "-map", "0:v", "-map", "[a0]", "-map", "[a1]", "-map", "[a2]",
        *VIDEO_OPTIONS,
        "-c:a:0", "aac", "-b:a:0", "48k",
        "-c:a:1", "ac3", "-b:a:1", "192k",
        "-c:a:2", "flac",
        "-metadata:s:a:0", "language=jpn",
        "-metadata:s:a:0", "title=日本語 440Hz",
        "-metadata:s:a:1", "language=eng",
        "-metadata:s:a:1", "title=English 880Hz",
        "-disposition:a:0", "0",
        "-disposition:a:1", "default",
        "-disposition:a:2", "0",
        "-shortest",
    )
    generate_multi_timebase(output)
    run_ffmpeg(
        output / "single.mp4",
        "-f", "lavfi", "-i", VIDEO_SOURCE,
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=6",
        "-map", "0:v", "-map", "1:a", *VIDEO_OPTIONS,
        "-c:a", "aac", "-b:a", "48k", "-shortest",
    )
    run_ffmpeg(
        output / "short-audio.mp4",
        "-f", "lavfi", "-i", VIDEO_SOURCE,
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=1",
        "-map", "0:v", "-map", "1:a", *VIDEO_OPTIONS,
        "-c:a", "aac", "-b:a", "48k",
    )
    run_ffmpeg(
        output / "silent.mp4",
        "-f", "lavfi", "-i", VIDEO_SOURCE,
        "-map", "0:v", *VIDEO_OPTIONS, "-an",
    )
    run_ffmpeg(
        output / "audio-only.flac",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=6",
        "-c:a", "flac",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, nargs="?", default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    generate(args.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
