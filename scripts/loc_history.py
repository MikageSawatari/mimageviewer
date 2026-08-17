#!/usr/bin/env python3
"""Count lines of code at regular checkpoints in git history.

Walks the repository history at a fixed interval (weekly by default), counts the
lines of every text file present in that commit's tree, and writes the result as
JSON/CSV.  Blob contents are read straight out of the object database via
``git cat-file --batch`` so nothing is ever checked out, and line counts are
memoised per blob SHA -- consecutive checkpoints share most of their files.

Usage:
    python scripts/loc_history.py                       # weekly, writes JSON + CSV
    python scripts/loc_history.py --interval-days 1     # daily
    python scripts/loc_history.py --json out.json --csv out.csv
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import subprocess
import sys
from pathlib import Path

# --- what counts as source -------------------------------------------------

# Extension -> language bucket.  Anything not listed here is skipped, which is
# also how binaries (png, dll, onnx, ...) get excluded.
LANGUAGES: dict[str, str] = {
    ".rs": "Rust",
    ".cpp": "C/C++",
    ".cc": "C/C++",
    ".cxx": "C/C++",
    ".c": "C/C++",
    ".h": "C/C++",
    ".hpp": "C/C++",
    ".py": "Python",
    ".ps1": "PowerShell",
    ".psm1": "PowerShell",
    ".sh": "Shell",
    ".js": "JavaScript",
    ".mjs": "JavaScript",
    ".ts": "JavaScript",
    ".css": "CSS",
    ".html": "HTML",
    ".htm": "HTML",
    ".wgsl": "Shader",
    ".hlsl": "Shader",
    ".glsl": "Shader",
    ".md": "Markdown",
    ".iss": "Config/Data",
    ".toml": "Config/Data",
    ".yml": "Config/Data",
    ".yaml": "Config/Data",
    ".json": "Config/Data",
    ".ini": "Config/Data",
}

# Top-level path prefix -> area bucket.
AREAS: list[tuple[str, str]] = [
    ("src/", "src (本体)"),
    ("crates/", "crates"),
    ("tools/", "tools"),
    ("tests/", "tests"),
    ("benches/", "tests"),
    ("docs/", "docs"),
    ("htdocs/", "htdocs (サイト/マニュアル)"),
    ("scripts/", "scripts"),
    ("installer/", "installer"),
]

# Paths never counted: build output, lockfiles and generated pages.  Matched as
# prefixes, or as exact paths when no trailing "/".
EXCLUDE = (
    "target/",
    "target-portable/",
    "Cargo.lock",
    "htdocs/mimageviewer/manual/changelog.html",  # generated from README.md
)

# Directory names that mean "third-party code lives below here", at any depth --
# vendor/eframe at the root, crates/remote-web/web/vendor/ further down.
VENDOR_DIRS = frozenset({"vendor", "third_party", "thirdparty", "node_modules"})


def classify(path: str) -> tuple[str, str] | None:
    """Return (language, area) for a repo-relative path, or None to skip it."""
    for skip in EXCLUDE:
        if skip.endswith("/"):
            if path.startswith(skip):
                return None
        elif path == skip:
            return None
    segments = path.split("/")
    if VENDOR_DIRS.intersection(segments[:-1]):
        return None
    if segments[-1].endswith((".min.js", ".min.css")):
        return None
    ext = path[path.rfind(".") :].lower() if "." in Path(path).name else ""
    lang = LANGUAGES.get(ext)
    if lang is None:
        return None
    area = next((name for prefix, name in AREAS if path.startswith(prefix)), "その他")
    return lang, area


# --- git plumbing ----------------------------------------------------------


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True, encoding="utf-8"
    ).stdout


def checkpoint_commits(interval_days: int) -> list[tuple[dt.date, str]]:
    """Pick one commit per interval, from the first commit up to HEAD."""
    first_iso = git("log", "--reverse", "--format=%aI", "--max-parents=0").splitlines()[0]
    first = dt.datetime.fromisoformat(first_iso).date()
    last_iso = git("log", "-1", "--format=%aI").splitlines()[0]
    last = dt.datetime.fromisoformat(last_iso).date()

    points: list[tuple[dt.date, str]] = []
    seen: set[str] = set()
    day = first
    while True:
        stamp = f"{day.isoformat()}T23:59:59"
        sha = git("rev-list", "-1", f"--before={stamp}", "HEAD").strip()
        if sha and sha not in seen:
            seen.add(sha)
            points.append((day, sha))
        if day >= last:
            break
        day = min(day + dt.timedelta(days=interval_days), last)
    return points


class BlobReader:
    """Persistent `git cat-file --batch` process, counting lines per blob."""

    def __init__(self) -> None:
        self.proc = subprocess.Popen(
            ["git", "cat-file", "--batch"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
        )
        self.cache: dict[str, int] = {}

    def line_count(self, sha: str) -> int:
        hit = self.cache.get(sha)
        if hit is not None:
            return hit
        assert self.proc.stdin and self.proc.stdout
        self.proc.stdin.write(sha.encode() + b"\n")
        self.proc.stdin.flush()
        header = self.proc.stdout.readline().split()
        size = int(header[2])
        body = self.proc.stdout.read(size)
        self.proc.stdout.read(1)  # trailing newline after the object
        lines = body.count(b"\n")
        if body and not body.endswith(b"\n"):
            lines += 1  # file without a final newline still has a last line
        self.cache[sha] = lines
        return lines

    def close(self) -> None:
        assert self.proc.stdin
        self.proc.stdin.close()
        self.proc.wait()


def measure(sha: str, reader: BlobReader) -> tuple[dict[str, int], dict[str, int], int]:
    """Count LOC in one commit's tree, bucketed by language and by area."""
    listing = subprocess.run(
        ["git", "ls-tree", "-r", "-z", sha],
        check=True,
        capture_output=True,
    ).stdout
    by_lang: dict[str, int] = {}
    by_area: dict[str, int] = {}
    files = 0
    for entry in listing.split(b"\0"):
        if not entry:
            continue
        meta, _, raw_path = entry.partition(b"\t")
        fields = meta.split()
        if fields[1] != b"blob":
            continue
        path = raw_path.decode("utf-8", "surrogateescape").replace("\\", "/")
        bucket = classify(path)
        if bucket is None:
            continue
        lang, area = bucket
        n = reader.line_count(fields[2].decode())
        by_lang[lang] = by_lang.get(lang, 0) + n
        by_area[area] = by_area.get(area, 0) + n
        files += 1
    return by_lang, by_area, files


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--interval-days", type=int, default=7)
    ap.add_argument("--json", default="target/loc-history/loc_history.json")
    ap.add_argument("--csv", default="target/loc-history/loc_history.csv")
    args = ap.parse_args()

    points = checkpoint_commits(args.interval_days)
    print(f"{len(points)} checkpoints, every {args.interval_days} day(s)", file=sys.stderr)

    reader = BlobReader()
    rows = []
    try:
        for i, (day, sha) in enumerate(points, 1):
            by_lang, by_area, files = measure(sha, reader)
            total = sum(by_lang.values())
            rows.append(
                {
                    "date": day.isoformat(),
                    "commit": sha[:8],
                    "files": files,
                    "total": total,
                    "by_language": by_lang,
                    "by_area": by_area,
                }
            )
            print(f"  [{i}/{len(points)}] {day} {sha[:8]}  {total:>8,} LOC", file=sys.stderr)
    finally:
        reader.close()

    languages = sorted({k for r in rows for k in r["by_language"]})
    areas = sorted({k for r in rows for k in r["by_area"]})

    json_path = Path(args.json)
    json_path.parent.mkdir(parents=True, exist_ok=True)
    json_path.write_text(
        json.dumps(
            {"languages": languages, "areas": areas, "points": rows},
            ensure_ascii=False,
            indent=1,
        ),
        encoding="utf-8",
    )

    csv_path = Path(args.csv)
    csv_path.parent.mkdir(parents=True, exist_ok=True)
    with csv_path.open("w", encoding="utf-8-sig", newline="") as f:
        f.write("date,commit,files,total," + ",".join(languages + areas) + "\n")
        for r in rows:
            cells = [r["by_language"].get(k, 0) for k in languages]
            cells += [r["by_area"].get(k, 0) for k in areas]
            f.write(
                f"{r['date']},{r['commit']},{r['files']},{r['total']},"
                + ",".join(str(c) for c in cells)
                + "\n"
            )

    print(f"wrote {json_path} and {csv_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
