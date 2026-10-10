import hashlib
from contextlib import closing
import json
import sqlite3
import shutil
import uuid
import unittest
from pathlib import Path
from release_checks_preflight import catalog_path, preflight
from generate_release_checks_fixture import isolated_path, reject_reparse


class PreflightTests(unittest.TestCase):
    def setUp(self):
        temporary_root = Path(__file__).resolve().parents[2] / "target" / "release-check-preflight-tests"
        reject_reparse(temporary_root)
        temporary_root.mkdir(parents=True, exist_ok=True)
        self.repo = temporary_root / ("isolated-" + uuid.uuid4().hex)
        self.repo.mkdir()
        def cleanup():
            isolated_path(self.repo, temporary_root)
            shutil.rmtree(self.repo)
        self.addCleanup(cleanup)
        self.data = self.repo / "target/portable-smoke/data"
        self.data.mkdir(parents=True)
        (self.data / ".disposable-smoke-data").write_text("mimageviewer-disposable-smoke-v1;test-script=true")
        self.fixture = self.repo / "target/ui-smoke/runs/run/fixture"
        self.manifest = self.fixture / "fixture-manifest.json"
        perf = {}
        for mode in ("cached", "uncached"):
            paths = [self.fixture / mode / str(i) for i in range(7)]
            for path in paths:
                path.mkdir(parents=True)
            perf[mode] = {"many_folders": str(paths[0]), "navigation_order": list(map(str, paths[1:]))}
        self.manifest.write_text(json.dumps({"perf": perf}))
        self.folder = Path(perf["cached"]["many_folders"])

    def check(self, **kwargs):
        return preflight(self.manifest, self.data, "perf-uncached", repo=self.repo, **kwargs)

    def test_refuses_unstopped_and_other_profile(self):
        with self.assertRaises(ValueError):
            self.check()
        with self.assertRaises(ValueError):
            preflight(self.manifest, self.fixture, "perf-cached", stopped=True, repo=self.repo)

    def test_missing_catalog_is_absent_without_creation(self):
        result = self.check(stopped=True)
        self.assertEqual(len(result["opens"]), 14)
        self.assertTrue(all(o["pre_open_absent"] for o in result["opens"]))
        self.assertFalse((self.data / "cache").exists())

    def test_reads_saved_keys_without_changing_db(self):
        db = catalog_path(self.data, self.folder)
        db.parent.mkdir(parents=True)
        with closing(sqlite3.connect(db)) as conn:
            conn.execute("CREATE TABLE thumbnails(filename TEXT)")
            conn.execute("INSERT INTO thumbnails VALUES ('real-cache-key')")
            conn.commit()
        before = hashlib.sha256(db.read_bytes()).hexdigest()
        self.assertEqual(self.check(stopped=True)["opens"][0]["catalog_keys"], ["real-cache-key"])
        self.assertEqual(hashlib.sha256(db.read_bytes()).hexdigest(), before)
        db.with_name(db.name + "-wal").write_bytes(b"pending")
        with self.assertRaises(ValueError):
            self.check(stopped=True)

    def test_catalog_hash_matches_drive_stripping_convention(self):
        folder = Path(r"C:\Example\Folder")
        digest = hashlib.sha256(b"/example/folder").hexdigest()
        self.assertEqual(catalog_path(Path("data"), folder), Path("data/cache") / digest[:2] / (digest + ".db"))


if __name__ == "__main__":
    unittest.main()
