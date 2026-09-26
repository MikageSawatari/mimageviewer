"""Seed and verify the disposable Collection sort UI smoke database."""

import argparse
import sqlite3
import uuid
from pathlib import Path


COLLECTION_ID = "80f58851-997b-4b80-90bc-f50bb1d2523e"
IMAGE_NAMES = ("01-one.png", "02-unrated.png", "03-two.png")


def expected_path_rows(fixture_dir: Path):
    for position, name in enumerate(IMAGE_NAMES):
        path = (fixture_dir / name).resolve(strict=True)
        if not path.is_file():
            raise ValueError(f"fixture image is missing: {path}")
        source_path = str(path)
        yield (
            str(uuid.uuid5(uuid.UUID(COLLECTION_ID), name)),
            source_path,
            source_path.replace("\\", "/").lower(),
            position,
        )


def validate_disposable_paths(fixture_dir: Path, data_dir: Path):
    data_dir = data_dir.resolve(strict=True)
    fixture_dir = fixture_dir.resolve(strict=True)
    marker = data_dir / ".disposable-smoke-data"
    if not marker.is_file() or marker.read_text(encoding="ascii").strip() != (
        "mimageviewer-disposable-smoke-v1;test-script=true"
    ):
        raise ValueError("disposable test-script smoke marker is missing or invalid")
    if not fixture_dir.is_relative_to(data_dir):
        raise ValueError("fixture directory is outside disposable smoke data")
    return fixture_dir, data_dir


def seed(fixture_dir: Path, data_dir: Path):
    fixture_dir, data_dir = validate_disposable_paths(fixture_dir, data_dir)
    db_path = data_dir / "collection.db"
    if db_path.exists():
        raise ValueError(f"refusing to overwrite Collection database: {db_path}")
    rows = list(expected_path_rows(fixture_dir))
    with sqlite3.connect(db_path) as db:
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
               VALUES (?, 'RatingSortCollectionSmoke', 'manual', 'file_name',
                       '0000000000000000', 1, 0, 0, 0)""",
            (COLLECTION_ID,),
        )
        db.executemany(
            """INSERT INTO collection_entries
               (id, collection_id, source_namespace, source_path, normalized_path,
                resolved_kind, manual_position, created_at_ms)
               VALUES (?, ?, 'filesystem_path', ?, ?, 'image', ?, 0)""",
            [(entry_id, COLLECTION_ID, source, key, position)
             for entry_id, source, key, position in rows],
        )
    verify(fixture_dir, data_dir)


def verify(fixture_dir: Path, data_dir: Path):
    fixture_dir, data_dir = validate_disposable_paths(fixture_dir, data_dir)
    expected = list(expected_path_rows(fixture_dir))
    db_path = data_dir / "collection.db"
    with sqlite3.connect(f"file:{db_path.as_posix()}?mode=ro", uri=True) as db:
        version = db.execute("PRAGMA user_version").fetchone()[0]
        collection = db.execute(
            """SELECT id, name, order_mode, sort_order, revision
               FROM collections"""
        ).fetchall()
        entries = db.execute(
            """SELECT id, source_path, normalized_path, manual_position
               FROM collection_entries ORDER BY manual_position"""
        ).fetchall()
        catalog_revision = db.execute(
            "SELECT catalog_revision FROM collection_meta WHERE singleton = 1"
        ).fetchone()
    if version != 2 or collection != [
        (COLLECTION_ID, "RatingSortCollectionSmoke", "manual", "file_name", 1)
    ] or entries != expected or catalog_revision != (1,):
        raise ValueError(
            f"Collection sort smoke database changed: version={version} "
            f"collection={collection} entries={entries} catalog_revision={catalog_revision}"
        )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture_dir", type=Path)
    parser.add_argument("--seed-db", type=Path)
    parser.add_argument("--verify-db", type=Path)
    args = parser.parse_args()
    if bool(args.seed_db) == bool(args.verify_db):
        parser.error("specify exactly one of --seed-db and --verify-db")
    if args.seed_db:
        seed(args.fixture_dir, args.seed_db)
    else:
        verify(args.fixture_dir, args.verify_db)
