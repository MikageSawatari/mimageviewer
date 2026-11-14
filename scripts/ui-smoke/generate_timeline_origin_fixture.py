#!/usr/bin/env python3
"""Generate small timeline-origin fixtures using system ffmpeg and lavfi only."""

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path


DEFAULT_OUTPUT = Path(__file__).resolve().parents[2] / "testdata" / "timeline-origin"
COLOURS = (
    "ff0000", "00ff00", "0000ff", "ffff00", "ff00ff", "00ffff",
    "ffffff", "000000", "ff8000", "8000ff", "008080", "808080",
)
# All-intra isolates the origin contract from the pre-existing TS keyframe seek
# defect tracked in docs/next-release-backlog.md section 1.780.
VIDEO_OPTIONS = [
    "-c:v", "mpeg2video", "-q:v", "3", "-g", "1", "-bf", "0",
    "-flags:v", "+low_delay",
]
VIDEO_SOURCE = ";".join(
    f"color=c=0x{colour}:s=96x64:r=12:d=1[v{index}]"
    for index, colour in enumerate(COLOURS)
) + ";" + "".join(f"[v{index}]" for index in range(12)) + "concat=n=12:v=1:a=0"


def pulse_source(duration: int = 12, late: bool = False) -> str:
    times = (0.5, 2.5, 4.5) if late else (2, 6, 10)
    gate = "+".join(f"between(t,{time},{time + 0.1})" for time in times)
    return f"aevalsrc=exprs='0.5*sin(2*PI*1000*t)*({gate})':s=48000:d={duration}"


def run_ffmpeg(output: Path, *arguments: str, metadata: str | None = None) -> None:
    subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *arguments, str(output)],
        check=True, input=metadata.encode("utf-8") if metadata is not None else None,
    )


def generate_ts(output: Path, name: str, offset: float = 0, *,
                interlaced: bool = False, late_audio: bool = False) -> None:
    arguments = [
        "-f", "lavfi", "-i", VIDEO_SOURCE,
        "-f", "lavfi", "-i", pulse_source(),
    ]
    if late_audio:
        arguments += ["-f", "lavfi", "-i", pulse_source(6, late=True)]
    arguments += ["-map", "0:v", "-map", "1:a"]
    if late_audio:
        arguments += [
            "-filter_complex", "[2:a]asetpts=PTS+4/TB[late]", "-map", "[late]",
            "-metadata:s:a:0", "language=jpn", "-metadata:s:a:1", "language=eng",
            "-disposition:a:0", "default", "-disposition:a:1", "0",
        ]
    arguments += VIDEO_OPTIONS
    if interlaced:
        arguments += ["-vf", "setfield=tff", "-flags:v", "+low_delay+ilme+ildct"]
    arguments += [
        "-c:a", "aac", "-b:a", "32k", "-output_ts_offset", str(offset),
        "-f", "mpegts",
    ]
    run_ffmpeg(output / name, *arguments)


def generate(output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    generate_ts(output, "small-origin.ts")
    generate_ts(output, "large-origin.ts", 36000)
    # Put the 33-bit, 90 kHz PTS wrap inside the 12-second clip. FFmpeg's
    # demuxer unwraps these to one continuous (usually negative-start) clock.
    generate_ts(output, "wrap-origin.ts", (2**33 / 90000) - 6)
    generate_ts(output, "interlaced-origin.ts", 36000, interlaced=True)
    generate_ts(output, "late-audio-origin.ts", 36000, late_audio=True)
    generate_rounded_origin_ts(output)
    run_ffmpeg(
        output / "large-origin.mpg",
        "-f", "lavfi", "-i", VIDEO_SOURCE,
        "-f", "lavfi", "-i", pulse_source(),
        "-map", "0:v", "-map", "1:a", *VIDEO_OPTIONS,
        "-c:a", "mp2", "-b:a", "64k", "-output_ts_offset", "36000",
        # Small PES packets keep the PS duration estimate close to 12 seconds.
        "-packetsize", "128",
        "-f", "mpeg",
    )
    chapters = ";FFMETADATA1\n" + "".join(
        f"[CHAPTER]\nTIMEBASE=1/1000\nSTART={start * 1000}\n"
        f"END={(start + 4) * 1000}\ntitle=Content {start - 10}-{start - 6}s\n"
        for start in (10, 14, 18)
    )
    run_ffmpeg(
        output / "positive-origin.mkv",
        "-f", "lavfi", "-i", VIDEO_SOURCE,
        "-f", "lavfi", "-i", pulse_source(),
        "-f", "ffmetadata", "-i", "pipe:0",
        "-map", "0:v", "-map", "1:a", "-map_chapters", "2",
        *VIDEO_OPTIONS, "-c:a", "flac", "-output_ts_offset", "10",
        metadata=chapters,
    )
    run_ffmpeg(
        output / "encoder-delay.mp3",
        "-f", "lavfi", "-i", pulse_source(),
        "-c:a", "libmp3lame", "-b:a", "32k", "-write_xing", "1",
    )


def generate_rounded_origin_ts(output: Path) -> None:
    # No audio: the first video PTS itself determines format start_time. The
    # offset puts it at 36001 + 2/3 seconds, rounded up to AV_TIME_BASE units.
    run_ffmpeg(
        output / "rounded-origin-video-only.ts",
        "-f", "lavfi", "-i", VIDEO_SOURCE,
        "-map", "0:v", *VIDEO_OPTIONS,
        "-output_ts_offset", "36000.266667", "-f", "mpegts",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path, nargs="?", default=DEFAULT_OUTPUT)
    arguments = parser.parse_args()
    generate(arguments.output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
