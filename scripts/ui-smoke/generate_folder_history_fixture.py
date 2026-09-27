"""Create a disposable folder-history fixture and seed Rating/Collection DBs."""

import argparse
import sqlite3
import struct
import uuid
import zlib
from pathlib import Path


COLLECTION_ID = "80f58851-997b-4b80-90bc-f50bb1d2523e"
MARKER = "mimageviewer-disposable-smoke-v1;test-script=true"


def image(rgb):
    width = height = 8
    pixels = b"".join(b"\0" + bytes(rgb) * width for _ in range(height))

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
        + chunk(b"IDAT", zlib.compress(pixels))
        + chunk(b"IEND", b"")
    )


def seed(fixture: Path, data_dir: Path):
    fixture = fixture.resolve(strict=True)
    data_dir = data_dir.resolve(strict=True)
    if not fixture.is_relative_to(data_dir):
        raise ValueError("fixture must be inside disposable smoke data")
    if (data_dir / ".disposable-smoke-data").read_text(encoding="ascii").strip() != MARKER:
        raise ValueError("disposable test-script marker is missing or invalid")

    for name, color in (
        ("F/G/g-page.png", (220, 60, 60)),
        ("F/z-page.png", (200, 100, 60)),
        ("B/D/d-page.png", (60, 100, 220)),
        ("B/z-page.png", (60, 180, 100)),
    ):
        destination = fixture / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(image(color))

    with sqlite3.connect(data_dir / "rating.db") as db:
        db.execute("CREATE TABLE ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL)")
        rating_path = str((fixture / "F").resolve(strict=True)).lower().replace("\\", "/")
        db.execute("INSERT INTO ratings(path, stars) VALUES (?, 1)", (rating_path,))

    collection_id = uuid.UUID(COLLECTION_ID)
    source_path = str((fixture / "B").resolve(strict=True))
    source_key = source_path.lower().replace("\\", "/")
    with sqlite3.connect(data_dir / "collection.db") as db:
        db.execute("PRAGMA foreign_keys = ON")
        db.executescript(
            """
            CREATE TABLE collection_meta (
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                catalog_revision INTEGER NOT NULL
            );
            CREATE TABLE collections (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                order_mode TEXT NOT NULL,
                sort_order TEXT NOT NULL,
                shuffle_seed TEXT NOT NULL DEFAULT '0000000000000000',
                revision INTEGER NOT NULL,
                catalog_position INTEGER NOT NULL UNIQUE,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );
            CREATE TABLE collection_entries (
                id TEXT PRIMARY KEY,
                collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
                source_namespace TEXT NOT NULL,
                source_path TEXT NOT NULL,
                normalized_path TEXT NOT NULL,
                resolved_kind TEXT NOT NULL,
                manual_position INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL,
                UNIQUE(collection_id, source_namespace, normalized_path),
                UNIQUE(collection_id, manual_position)
            );
            CREATE INDEX idx_collection_entries_collection
                ON collection_entries(collection_id, manual_position);
            PRAGMA user_version = 2;
            """
        )
        db.execute("INSERT INTO collection_meta VALUES (1, 1)")
        db.execute(
            """INSERT INTO collections
               (id, name, order_mode, sort_order, shuffle_seed, revision,
                catalog_position, created_at_ms, updated_at_ms)
               VALUES (?, 'FolderHistorySmoke', 'manual', 'file_name',
                       '0000000000000000', 1, 0, 0, 0)""",
            (COLLECTION_ID,),
        )
        db.execute(
            """INSERT INTO collection_entries
               (id, collection_id, source_namespace, source_path, normalized_path,
                resolved_kind, manual_position, created_at_ms)
               VALUES (?, ?, 'filesystem_path', ?, ?, 'folder', 0, 0)""",
            (str(uuid.uuid5(collection_id, "B")), COLLECTION_ID, source_path, source_key),
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture", type=Path)
    parser.add_argument("data_dir", type=Path)
    args = parser.parse_args()
    seed(args.fixture, args.data_dir)
