"""Seed 600 rating rows with mid-list books in marked disposable smoke data."""

import argparse
import json
import sqlite3
import zipfile
from contextlib import closing
from pathlib import Path

from generate_rating_sort_fixture import png


MARKER = "mimageviewer-disposable-smoke-v1;test-script=true"
FOLDER_NUMBERS = range(299, 311)
ZIP_NUMBERS = range(311, 314)
ROOT_ITEMS = 600
FAVORITES = (
    ("5a7fa8f0-e560-4f8e-905d-031000000001", "0310-folder"),
    ("5a7fa8f0-e560-4f8e-905d-031100000002", "0311-book.zip"),
)


def item_name(number):
    if number in FOLDER_NUMBERS:
        return f"{number:04}-folder"
    if number in ZIP_NUMBERS:
        return f"{number:04}-book.zip"
    return f"{number:04}-image.png"


def path_key(path):
    return str(path).lower().replace("\\", "/")


def view_state(columns):
    return {
        "grid_view_mode": "Thumbnail",
        "grid_cols": columns,
        "thumb_aspect": "Square",
        "thumb_aspect_auto": True,
        "grid_display_order": [["folder", "archive", "image", "video_audio"], [], [], []],
        "sort_order": "FileName",
        "default_spread_mode": "Single",
        "default_reading_flow": "Paged",
    }


def seed(fixture: Path, data_dir: Path):
    fixture = fixture.resolve(strict=True)
    data_dir = data_dir.resolve(strict=True)
    if not fixture.is_relative_to(data_dir):
        raise ValueError("fixture must be inside disposable smoke data")
    if (data_dir / ".disposable-smoke-data").read_text(encoding="ascii").strip() != MARKER:
        raise ValueError("disposable test-script marker is missing or invalid")
    if any(fixture.iterdir()) or any(
        (data_dir / name).exists()
        for name in ("rating.db", "adjustment.db", "settings-override.json")
    ):
        raise ValueError("fixture, rating/adjustment DBs and settings override must be fresh")

    rated = []
    for number in FOLDER_NUMBERS:
        folder = fixture / item_name(number)
        folder.mkdir()
        for page in range(1, 9):
            (folder / f"page-{page:02}.png").write_bytes(
                png((number % 256, page * 29, 110), width=96, height=144)
            )
        rated.append(folder)
    for number in ZIP_NUMBERS:
        book = fixture / item_name(number)
        with zipfile.ZipFile(book, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for page in range(1, 9):
                archive.writestr(
                    f"page-{page:02}.png", png((70, page * 29, number % 256), width=96, height=144)
                )
        rated.append(book)
    for number in range(1, ROOT_ITEMS + 1):
        if number in FOLDER_NUMBERS or number in ZIP_NUMBERS:
            continue
        image = fixture / item_name(number)
        image.write_bytes(png((number % 256, number // 256 * 70, 160), width=96, height=128))
        rated.append(image)
    with closing(sqlite3.connect(data_dir / "rating.db")) as db, db:
        db.execute("CREATE TABLE ratings (path TEXT PRIMARY KEY, stars INTEGER NOT NULL)")
        db.executemany(
            "INSERT INTO ratings(path, stars) VALUES (?, 3)",
            [(path_key(path),) for path in rated],
        )
    # Seed the production UUID-owned store, not a runtime column assignment. App startup
    # hydrates these rows after the settings override supplies the matching favorites.
    with closing(sqlite3.connect(data_dir / "adjustment.db")) as db, db:
        db.execute(
            "CREATE TABLE favorite_view_states "
            "(favorite_id TEXT PRIMARY KEY, state_json TEXT NOT NULL)"
        )
        db.executemany(
            "INSERT INTO favorite_view_states VALUES (?, ?)",
            [(favorite_id, json.dumps(view_state(6))) for favorite_id, _ in FAVORITES],
        )
    settings = view_state(10)
    settings.update({
        "rating_view_sort": {"Normal": "FileName"},
        "auto_fullscreen_image_folders": False,
        "auto_fullscreen_zip_pdf": False,
        "grid_open_selected_item_on_click": False,
        "remember_favorite_view_state": True,
        "ring_shortcuts": {"mouse_nav_prompt_done": True},
        "favorites": [
            {
                "id": favorite_id,
                "name": name,
                "path": str(fixture / name),
                "auto_index_structure": False,
                "auto_index_metadata": False,
                "auto_index_thumbs": False,
                "auto_index_similar": False,
            }
            for favorite_id, name in FAVORITES
        ],
    })
    (data_dir / "settings-override.json").write_text(
        json.dumps(settings, indent=2) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("fixture", type=Path)
    parser.add_argument("data_dir", type=Path)
    args = parser.parse_args()
    seed(args.fixture, args.data_dir)
