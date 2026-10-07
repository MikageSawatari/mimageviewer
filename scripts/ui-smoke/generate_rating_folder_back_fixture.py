"""Seed 12 folders and three ZIPs in explicitly marked disposable smoke data."""

import argparse
import sqlite3
import zipfile
from contextlib import closing
from pathlib import Path

from generate_rating_sort_fixture import png


MARKER = "mimageviewer-disposable-smoke-v1;test-script=true"


def seed(fixture: Path, data_dir: Path):
    fixture = fixture.resolve(strict=True)
    data_dir = data_dir.resolve(strict=True)
    if not fixture.is_relative_to(data_dir):
        raise ValueError("fixture must be inside disposable smoke data")
    if (data_dir / ".disposable-smoke-data").read_text(encoding="ascii").strip() != MARKER:
        raise ValueError("disposable test-script marker is missing or invalid")
    if any(fixture.iterdir()) or (data_dir / "rating.db").exists():
        raise ValueError("fixture and rating DB must be fresh")

    rated = []
    for number in range(1, 13):
        folder = fixture / f"{number:02}-folder"
        folder.mkdir()
        for page in range(1, 9):
            (folder / f"page-{page:02}.png").write_bytes(
                png((number * 17, page * 29, 110), width=96, height=128)
            )
        rated.append(folder)
    for number in range(21, 24):
        book = fixture / f"{number:02}-book.zip"
        with zipfile.ZipFile(book, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for page in range(1, 9):
                archive.writestr(
                    f"page-{page:02}.png", png((70, page * 29, number * 10), width=96, height=128)
                )
        rated.append(book)
    with closing(sqlite3.connect(data_dir / "rating.db")) as db, db:
        db.execute("CREATE TABLE ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL)")
        db.executemany(
            "INSERT INTO ratings(path, stars) VALUES (?, 3)",
            [(str(path).lower().replace("\\", "/"),) for path in rated],
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture", type=Path)
    parser.add_argument("data_dir", type=Path)
    args = parser.parse_args()
    seed(args.fixture, args.data_dir)
