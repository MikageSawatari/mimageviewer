"""Noninteractive checks for the isolated rating-folder-back fixture."""

import json
import sqlite3
import struct
import shutil
import unittest
import uuid
import zipfile
from contextlib import closing
from pathlib import Path

from generate_rating_folder_back_fixture import MARKER, ROOT_ITEMS, item_name, seed


class RatingFolderBackFixtureTests(unittest.TestCase):
    def setUp(self):
        test_root = Path(__file__).resolve().parents[2] / "target"
        test_root.mkdir(exist_ok=True)
        # Python 3.13 TemporaryDirectory uses a private Windows ACL (mode 0700), which
        # excludes the restricted agent token. Use an ordinary new workspace directory.
        self.temp = test_root / f"rating-folder-back-test-{uuid.uuid4().hex}"
        self.temp.mkdir()
        self.assertTrue(self.temp.resolve().is_relative_to(test_root.resolve()))
        self.addCleanup(shutil.rmtree, self.temp)
        self.data = self.temp / "data"
        self.fixture = self.data / "rating-folder-back" / "library"
        self.fixture.mkdir(parents=True)
        (self.data / ".disposable-smoke-data").write_text(MARKER, encoding="ascii")

    def test_six_hundred_rating_rows_with_mid_list_books_and_distinct_child_aspect(self):
        seed(self.fixture, self.data)
        folders = sorted(path for path in self.fixture.iterdir() if path.is_dir())
        archives = sorted(self.fixture.glob("*.zip"))
        images = sorted(self.fixture.glob("*.png"))
        self.assertEqual(len(folders), 12)
        self.assertEqual(len(archives), 3)
        self.assertEqual(len(images), 585)
        names = sorted(path.name for path in self.fixture.iterdir())
        self.assertEqual(names, [item_name(number) for number in range(1, ROOT_ITEMS + 1)])
        self.assertEqual(names.index("0310-folder"), 309)
        self.assertEqual(names.index("0311-book.zip"), 310)
        self.assertGreaterEqual(names.index("0310-folder") // 10, 25)
        for image in images:
            self.assertEqual(struct.unpack(">II", image.read_bytes()[16:24]), (96, 128))
        pages = [f"page-{page:02}.png" for page in range(1, 9)]
        for folder in folders:
            self.assertEqual(sorted(path.name for path in folder.iterdir()), pages)
            for page in folder.iterdir():
                self.assertTrue(page.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"))
                self.assertEqual(struct.unpack(">II", page.read_bytes()[16:24]), (96, 144))
        for book in archives:
            with zipfile.ZipFile(book) as archive:
                self.assertEqual(archive.namelist(), pages)
                self.assertIsNone(archive.testzip())
                for page in pages:
                    self.assertEqual(struct.unpack(">II", archive.read(page)[16:24]), (96, 144))
        with closing(sqlite3.connect(self.data / "rating.db")) as db:
            rows = db.execute("SELECT path, stars FROM ratings ORDER BY path").fetchall()
        expected = sorted(str(path).lower().replace("\\", "/") for path in folders + archives + images)
        self.assertEqual(len(rows), ROOT_ITEMS)
        self.assertEqual(rows, [(path, 3) for path in expected])

    def test_unmarked_data_is_rejected_before_creating_fixture_or_database(self):
        (self.data / ".disposable-smoke-data").write_text("wrong", encoding="ascii")
        with self.assertRaisesRegex(ValueError, "marker"):
            seed(self.fixture, self.data)
        self.assertEqual(list(self.fixture.iterdir()), [])
        self.assertFalse((self.data / "rating.db").exists())

    def test_common_ten_columns_and_two_uuid_owned_six_column_favorites(self):
        seed(self.fixture, self.data)
        settings = json.loads((self.data / "settings-override.json").read_text(encoding="utf-8"))
        mixed_order = [["folder", "archive", "image", "video_audio"], [], [], []]
        self.assertEqual(settings["grid_cols"], 10)
        self.assertEqual(settings["rating_view_sort"], {"Normal": "FileName"})
        self.assertTrue(settings["remember_favorite_view_state"])
        self.assertTrue(settings["thumb_aspect_auto"])
        self.assertEqual(settings["grid_display_order"], mixed_order)
        favorites = settings["favorites"]
        self.assertEqual([favorite["name"] for favorite in favorites], ["0310-folder", "0311-book.zip"])
        self.assertEqual(len({favorite["id"] for favorite in favorites}), 2)
        for favorite in favorites:
            self.assertEqual(str(uuid.UUID(favorite["id"])), favorite["id"])
            self.assertEqual(Path(favorite["path"]), self.fixture / favorite["name"])
            self.assertNotEqual(Path(favorite["path"]), self.fixture)
            for key in ("auto_index_structure", "auto_index_metadata", "auto_index_thumbs", "auto_index_similar"):
                self.assertFalse(favorite[key])
        with closing(sqlite3.connect(self.data / "adjustment.db")) as db:
            rows = db.execute("SELECT favorite_id, state_json FROM favorite_view_states").fetchall()
        self.assertEqual({row[0] for row in rows}, {favorite["id"] for favorite in favorites})
        expected = {
            "grid_view_mode": "Thumbnail", "grid_cols": 6, "thumb_aspect": "Square",
            "thumb_aspect_auto": True, "grid_display_order": mixed_order,
            "sort_order": "FileName", "default_spread_mode": "Single", "default_reading_flow": "Paged",
        }
        for _, state_json in rows:
            self.assertEqual(json.loads(state_json), expected)

    def test_existing_favorite_seed_outputs_are_rejected_before_any_mutation(self):
        for name in ("adjustment.db", "settings-override.json"):
            with self.subTest(name=name):
                existing = self.data / name
                existing.write_bytes(b"preserve this output")
                try:
                    with self.assertRaisesRegex(ValueError, "fresh"):
                        seed(self.fixture, self.data)
                    self.assertEqual(existing.read_bytes(), b"preserve this output")
                    self.assertEqual(list(self.fixture.iterdir()), [])
                    self.assertFalse((self.data / "rating.db").exists())
                finally:
                    existing.unlink()

    def test_fixture_outside_marked_data_is_rejected(self):
        outside = self.temp / "outside"
        outside.mkdir()
        with self.assertRaisesRegex(ValueError, "inside"):
            seed(outside, self.data)
        self.assertEqual(list(outside.iterdir()), [])

    def test_existing_database_is_rejected_without_changing_it(self):
        db = self.data / "rating.db"
        db.write_bytes(b"preserve this database")
        with self.assertRaisesRegex(ValueError, "fresh"):
            seed(self.fixture, self.data)
        self.assertEqual(db.read_bytes(), b"preserve this database")
        self.assertEqual(list(self.fixture.iterdir()), [])

    def test_existing_fixture_is_rejected_without_changing_it(self):
        original = self.fixture / "original.png"
        original.write_bytes(b"preserve this image")
        with self.assertRaisesRegex(ValueError, "fresh"):
            seed(self.fixture, self.data)
        self.assertEqual(original.read_bytes(), b"preserve this image")
        self.assertFalse((self.data / "rating.db").exists())


if __name__ == "__main__":
    unittest.main()
