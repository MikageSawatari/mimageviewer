"""Create the tiny rating-order UI smoke images or seed a disposable rating DB."""

import argparse
import json
import sqlite3
import struct
import zlib
from pathlib import Path


ROWS = (
    ("01-one.png", (220, 70, 70)),
    ("02-unrated.png", (70, 200, 100)),
    ("03-two.png", (70, 110, 220)),
)


def png(rgb, width=8, height=8):
    raw = b"".join(b"\0" + bytes(rgb) * width for _ in range(height))

    def chunk(kind, payload):
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


def generate(folder):
    folder.mkdir(parents=True, exist_ok=True)
    for name, color in ROWS:
        (folder / name).write_bytes(png(color))


def seed(folder, data_dir):
    data_dir.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(data_dir / "rating.db") as db:
        db.execute(
            "CREATE TABLE IF NOT EXISTS ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL)"
        )
        for name, stars in (("01-one.png", 1), ("03-two.png", 2)):
            path = (folder / name).resolve()
            key = str(path).lower().replace("\\", "/")
            db.execute(
                "INSERT OR REPLACE INTO ratings(path, stars) VALUES (?, ?)", (key, stars)
            )


def verify_saved_settings(data_dir):
    with sqlite3.connect(data_dir / "settings.db") as db:
        row = db.execute(
            "SELECT value FROM settings_kv WHERE key = 'rating_sort_unrated_position'"
        ).fetchone()
    if row is None or json.loads(row[0]) != "BelowAll":
        raise SystemExit("Preferences did not save BelowAll in settings.db")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("folder", type=Path)
    parser.add_argument("--seed-db", type=Path)
    parser.add_argument("--verify-settings", type=Path)
    args = parser.parse_args()
    if args.verify_settings:
        verify_saved_settings(args.verify_settings)
    elif args.seed_db:
        seed(args.folder, args.seed_db)
    else:
        generate(args.folder)
