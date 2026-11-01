"""Generate the test assets for the EffeTune lyrics trial (docs/effetune-lyrics-trial-plan.md §6.2).

Trial only. Uses the ffmpeg on PATH. All lyrics are generated text; no commercial lyrics.

usage: python scripts/generate-lyrics-trial-assets.py [output_dir]
default output_dir: target/lyrics-trial/assets
"""

from __future__ import annotations

import pathlib
import subprocess
import sys

BEEP_EVERY = 2.0
BEEP_LEN = 0.05
DURATION = 60.0
VIDEO_DURATION = 30.0
BEEP = f"0.5*sin(2*PI*1000*t)*lt(mod(t\\,{BEEP_EVERY})\\,{BEEP_LEN})"


def stamp(seconds: float) -> str:
    minutes, rest = divmod(seconds, 60.0)
    return f"{int(minutes):02d}:{rest:05.2f}"


def timed_lrc(duration: float, enhanced: bool = True) -> str:
    """One line per beep. The line text is the beep number; dots are word-timed every 0.5 s."""
    lines = ["[ti:lyrics trial beep]", "[ar:mIV trial]"]
    count = int(duration // BEEP_EVERY)
    for index in range(count):
        start = index * BEEP_EVERY
        number = f"{index + 1:02d}"
        if enhanced:
            words = [f"<{stamp(start)}>{number}"]
            words += [f"<{stamp(start + 0.5 * step)}> ." for step in range(1, 4)]
            lines.append(f"[{stamp(start)}]" + "".join(words))
        else:
            lines.append(f"[{stamp(start)}]{number}")
    return "\n".join(lines) + "\n"


def plain_lyrics() -> str:
    return "\n".join(f"plain line {index:02d}" for index in range(1, 11)) + "\n"


def run(args: list[str]) -> None:
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *args], check=True)


def beep_audio(path: pathlib.Path, duration: float, extra: list[str] | None = None) -> None:
    run(["-f", "lavfi", "-i", f"aevalsrc={BEEP}:s=48000:d={duration}", "-ac", "2", *(extra or []), str(path)])


def main() -> int:
    out = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/lyrics-trial/assets")
    out.mkdir(parents=True, exist_ok=True)

    # 1. Measurement track: FLAC without artwork + same-name LRC (UTF-8).
    beep_audio(out / "beep-60s.flac", DURATION)
    (out / "beep-60s.lrc").write_text(timed_lrc(DURATION), encoding="utf-8", newline="\n")

    # 2. Shift_JIS sidecar (Japanese text) and an upper-case sidecar name.
    beep_audio(out / "sjis-test.flac", DURATION)
    sjis = timed_lrc(DURATION, enhanced=False).replace("[ar:mIV trial]", "[ar:試作]")
    sjis = "\n".join(
        line + " 番目" if line.startswith("[0") else line for line in sjis.splitlines()
    ) + "\n"
    (out / "sjis-test.lrc").write_bytes(sjis.encode("shift_jis"))
    beep_audio(out / "upper-case.flac", DURATION)
    (out / "UPPER-CASE.LRC").write_text(timed_lrc(DURATION, enhanced=False), encoding="utf-8", newline="\n")

    # 3. Embedded lyrics (key names confirmed in plan §11.2: MP3/M4A "lyrics", FLAC "LYRICS").
    timed = timed_lrc(DURATION, enhanced=False)
    beep_audio(out / "embedded-timed.mp3", DURATION, ["-metadata", f"lyrics={timed}", "-id3v2_version", "3"])
    beep_audio(out / "embedded-timed.m4a", DURATION, ["-c:a", "aac", "-metadata", f"lyrics={timed}"])
    beep_audio(out / "embedded-plain.flac", DURATION, ["-metadata", f"LYRICS={plain_lyrics()}"])

    # 4. Untimed sidecar only, and a track without lyrics.
    beep_audio(out / "plain-lrc.flac", DURATION)
    (out / "plain-lrc.lrc").write_text(plain_lyrics(), encoding="utf-8", newline="\n")
    beep_audio(out / "no-lyrics.flac", DURATION)

    # 5. Video with the seconds burned in, beeps, and a same-name LRC.
    font = "C\\:/Windows/Fonts/arial.ttf"
    run([
        "-f", "lavfi", "-i", f"color=c=black:s=1280x720:r=30:d={VIDEO_DURATION}",
        "-f", "lavfi", "-i", f"aevalsrc={BEEP}:s=48000:d={VIDEO_DURATION}",
        "-vf", f"drawtext=fontfile='{font}':text='%{{pts\\:flt\\:2}}':fontsize=160:fontcolor=white:x=(w-tw)/2:y=(h-th)/2",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-ac", "2", "-shortest",
        str(out / "beep-video.mp4"),
    ])
    (out / "beep-video.lrc").write_text(timed_lrc(VIDEO_DURATION), encoding="utf-8", newline="\n")

    for path in sorted(out.iterdir()):
        print(f"{path.stat().st_size:>10}  {path.name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
