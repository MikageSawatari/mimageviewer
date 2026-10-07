"""Noninteractive checks for the isolated rating-folder-back fixture."""

import sqlite3
import struct
import shutil
import unittest
import uuid
import zipfile
from contextlib import closing
from pathlib import Path

from generate_rating_folder_back_fixture import MARKER, seed


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

    def test_images_archives_and_ratings_use_the_same_fifteen_container_keys(self):
        seed(self.fixture, self.data)
        folders = sorted(path for path in self.fixture.iterdir() if path.is_dir())
        archives = sorted(self.fixture.glob("*.zip"))
        self.assertEqual(len(folders), 12)
        self.assertEqual(len(archives), 3)
        pages = [f"page-{page:02}.png" for page in range(1, 9)]
        for folder in folders:
            self.assertEqual(sorted(path.name for path in folder.iterdir()), pages)
            for page in folder.iterdir():
                self.assertTrue(page.read_bytes().startswith(b"\x89PNG\r\n\x1a\n"))
                self.assertEqual(struct.unpack(">II", page.read_bytes()[16:24]), (96, 128))
        for book in archives:
            with zipfile.ZipFile(book) as archive:
                self.assertEqual(archive.namelist(), pages)
                self.assertIsNone(archive.testzip())
                for page in pages:
                    self.assertEqual(struct.unpack(">II", archive.read(page)[16:24]), (96, 128))
        with closing(sqlite3.connect(self.data / "rating.db")) as db:
            rows = db.execute("SELECT path, stars FROM ratings ORDER BY path").fetchall()
        expected = sorted(str(path).lower().replace("\\", "/") for path in folders + archives)
        self.assertEqual(rows, [(path, 3) for path in expected])

    def test_unmarked_data_is_rejected_before_creating_fixture_or_database(self):
        (self.data / ".disposable-smoke-data").write_text("wrong", encoding="ascii")
        with self.assertRaisesRegex(ValueError, "marker"):
            seed(self.fixture, self.data)
        self.assertEqual(list(self.fixture.iterdir()), [])
        self.assertFalse((self.data / "rating.db").exists())

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
