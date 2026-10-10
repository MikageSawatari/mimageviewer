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
            loads = [paths[0] / f"folder-{i:04d}" for i in range(1, 4)]
            for path in loads:
                path.mkdir()
            perf[mode] = {"many_folders": str(paths[0]), "navigation_order": list(map(str, paths[1:])),
                          "load_order": list(map(str, loads))}
        tray = {name: str(self.fixture / name) for name in ("tray-while", "tray-after")}
        for path in tray.values():
            Path(path).mkdir()
        self.manifest.write_text(json.dumps({"perf": perf, "paths": tray}))
        self.tray_roots = list(map(Path, tray.values()))
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
        self.assertEqual(len(result["opens"]), 22)
        self.assertTrue(all(o["pre_open_absent"] for o in result["opens"]))
        self.assertEqual([entry["folder_path"] for entry in result["opens"][-2:]],
                         list(map(str, self.tray_roots)))
        self.assertFalse((self.data / "cache").exists())

    def test_each_workload_folder_has_stopped_cache_absence_proof(self):
        manifest = json.loads(self.manifest.read_text())
        opens = {o["folder_path"]: o for o in self.check(stopped=True)["opens"]}
        for mode in ("cached", "uncached"):
            for path in manifest["perf"][mode]["load_order"]:
                self.assertTrue(opens[path]["pre_open_absent"])

    def test_tray_parent_catalog_owns_all_tile_keys_and_nonempty_wal_refuses_proof(self):
        for root in self.tray_roots:
            db = catalog_path(self.data, root)
            db.parent.mkdir(parents=True, exist_ok=True)
            with closing(sqlite3.connect(db)) as conn:
                conn.execute("CREATE TABLE thumbnails(filename TEXT)")
                conn.executemany("INSERT INTO thumbnails VALUES (?)",
                                 [(f"folder-{number:04d}",) for number in range(1, 601)])
                conn.commit()
            before = hashlib.sha256(db.read_bytes()).hexdigest()
            proof = next(entry for entry in self.check(stopped=True)["opens"]
                         if entry["folder_path"] == str(root))
            self.assertFalse(proof["pre_open_absent"])
            self.assertEqual(set(proof["catalog_keys"]),
                             {f"folder-{number:04d}" for number in range(1, 601)})
            self.assertEqual(hashlib.sha256(db.read_bytes()).hexdigest(), before)
            wal = db.with_name(db.name + "-wal")
            wal.write_bytes(b"pending tray thumbnail write")
            with self.subTest(root=root), self.assertRaisesRegex(ValueError, "nonempty stopped catalog WAL"):
                self.check(stopped=True)
            wal.unlink()

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
