"""Noninteractive media, settings and isolation tests; no mImageViewer launch."""

import hashlib
import json
import shutil
import stat
import struct
import subprocess
import unittest
import uuid
import zlib
from pathlib import Path
from unittest.mock import patch

import generate_release_checks_fixture as fixture


def fake_ffmpeg(command, **kwargs):
    if kwargs != {"check": True, "timeout": 120,
                  "creationflags": getattr(subprocess, "CREATE_NO_WINDOW", 0)}:
        raise AssertionError("FFmpeg must be finite, checked and noninteractive")
    Path(command[-1]).write_bytes(b"synthetic-ffmpeg-test-output")


def png_chunks(encoded):
    offset = 8
    while offset < len(encoded):
        size = struct.unpack(">I", encoded[offset:offset + 4])[0]
        kind = encoded[offset + 4:offset + 8]
        data = encoded[offset + 8:offset + 8 + size]
        crc = struct.unpack(">I", encoded[offset + 8 + size:offset + 12 + size])[0]
        if crc != zlib.crc32(kind + data) & 0xFFFFFFFF:
            raise AssertionError("PNG CRC mismatch")
        yield kind, data
        offset += size + 12


class ReleaseChecksFixtureTests(unittest.TestCase):
    def setUp(self):
        test_root = Path(__file__).resolve().parents[2] / "target" / "release-fixture-tests"
        fixture.reject_reparse(test_root)
        test_root.mkdir(parents=True, exist_ok=True)
        self.root = test_root / ("isolated-" + uuid.uuid4().hex)
        self.root.mkdir()
        def cleanup():
            fixture.isolated_path(self.root, test_root)
            shutil.rmtree(self.root)
        self.addCleanup(cleanup)
        self.ffmpeg = self.root / "ffmpeg.exe"
        self.ffmpeg.write_bytes(b"fake media generator, not an app")
        self.output = self.root / "run" / "fixture"

    def generate_small(self, **kwargs):
        # Keep the full independent subtree/nav/search shape while avoiding
        # repeated 512/600-folder writes in safety tests. No production size CLI.
        with patch.object(fixture, "FOLDER_COUNT", 2), patch.object(fixture, "TRAY_FOLDER_COUNT", 2):
            return fixture.generate(self.output, self.ffmpeg, allowed_root=self.root,
                                    run_command=fake_ffmpeg, **kwargs)

    def verify_small(self):
        with patch.object(fixture, "FOLDER_COUNT", 2), patch.object(fixture, "TRAY_FOLDER_COUNT", 2):
            return fixture.verify_fixture(self.output, allowed_root=self.root)

    def test_wide_png_pixels_metadata_and_crc_are_deterministic(self):
        first = fixture.fixture_png(256, fixture.SEARCH_TERM)
        self.assertEqual(first, fixture.fixture_png(256, fixture.SEARCH_TERM))
        self.assertNotEqual(first, fixture.fixture_png(257, fixture.SEARCH_TERM))
        chunks = dict(png_chunks(first))
        self.assertEqual(struct.unpack(">II", chunks[b"IHDR"][:8]), (884, 444))
        pixels = zlib.decompress(chunks[b"IDAT"])
        self.assertEqual(pixels, (b"\0" + bytes((0, 1, 0)) * 884) * 444)
        self.assertIn(b"parameters\0" + fixture.SEARCH_TERM.encode(), chunks[b"tEXt"])
        with self.assertRaises(ValueError):
            fixture.fixture_png(1 << 24)

    def test_small_tray_png_pixels_and_crc_are_deterministic(self):
        encoded = fixture.fixture_png(258, width=400, height=300)
        self.assertEqual(encoded, fixture.fixture_png(258, width=400, height=300))
        self.assertNotEqual(encoded, fixture.fixture_png(259, width=400, height=300))
        chunks = dict(png_chunks(encoded))
        self.assertEqual(struct.unpack(">II", chunks[b"IHDR"][:8]), (400, 300))
        self.assertEqual(zlib.decompress(chunks[b"IDAT"]),
                         (b"\0" + bytes((0, 1, 2)) * 400) * 300)
        self.assertNotIn(b"tEXt", chunks)

    def test_recipe_media_inventory_search_and_settings(self):
        manifest = self.generate_small()
        self.assertEqual(self.verify_small(), manifest)
        self.assertEqual(manifest["seed_state"], "not_prepared")
        self.assertEqual(manifest["counts"]["many_subtrees"], 2)
        self.assertEqual(manifest["counts"]["images_per_many_folder"], 4)
        self.assertEqual(manifest["counts"]["tray_subtrees"], 2)
        self.assertEqual(manifest["counts"]["images_per_tray_folder"], 1)
        self.assertEqual(manifest["counts"]["navigation_folders_per_subtree"], 6)
        self.assertEqual(manifest["static"]["requested_px_target"], 374)
        self.assertEqual(manifest["static"]["requested_px_tolerance"], 8)
        self.assertTrue(manifest["static"]["requested_px_must_be_observed"])
        hashes = []
        for entry in manifest["files"]:
            path = Path(entry["path"])
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), entry["sha256"])
            if entry["kind"] == "png":
                hashes.append(entry["sha256"])
                dimensions = (400, 300) if Path(entry["relative_path"]).parts[0] in fixture.TRAY_NAMES else (884, 444)
                self.assertEqual((entry["width"], entry["height"]), dimensions)
                self.assertEqual(struct.unpack(">II", dict(png_chunks(path.read_bytes()))[b"IHDR"][:8]),
                                 dimensions)
        self.assertEqual(len(hashes), len(set(hashes)))
        self.assertEqual(len(manifest["search"]["expected_hits"]), 3)
        hits = set(manifest["search"]["expected_hits"])
        for image in Path(manifest["paths"]["search"]).glob("*.png"):
            self.assertEqual(fixture.SEARCH_TERM.encode() in image.read_bytes(), str(image) in hits)
            self.assertNotIn(fixture.SEARCH_TERM, image.name)
        for case, file in manifest["overlays"].items():
            overlay = json.loads(Path(file).read_text(encoding="utf-8"))
            self.assertEqual(overlay["startup_folder_mode"], "specific")
            self.assertEqual(overlay["startup_window_state"], "normal")
            self.assertEqual(overlay["grid_view_mode"], "Thumbnail")
            self.assertEqual(overlay["thumb_aspect"], "Landscape16x9")
            self.assertEqual(overlay["keymap"]["overrides"], [])
            self.assertTrue(overlay["minimize_to_tray_on_close"])
            self.assertEqual(overlay["favorites"][0]["path"], manifest["paths"]["search"])
            self.assertFalse(overlay["favorites"][0]["auto_index_thumbs"])
            self.assertFalse(overlay["favorites"][0]["auto_index_similar"])
            if case.startswith("tray-"):
                self.assertEqual(overlay["cache_policy"], "Off")
                self.assertEqual(overlay["grid_cols"], 10)
            else:
                self.assertNotEqual(overlay.get("cache_policy"), "Off")
                self.assertEqual(overlay["grid_cols"], 7)
        self.assertFalse(list(self.output.rglob("*.db")))
        self.assertTrue(manifest["seed_requirements"]["app_launch_requires_interactive_approval"])
        self.assertEqual(set(manifest["seed_requirements"]["index"]["roots"]),
                         {manifest["paths"]["search"]})

    def test_full_topology_keeps_perf_shape_and_known_tray_regression_shape(self):
        # The encoder is covered separately; substitute tiny bytes to cheaply
        # exercise the production counts and uniqueness across all 5,366 PNGs.
        def tiny(identity, prompt=None, **dimensions):
            return f"png-id={identity};prompt={prompt};dimensions={dimensions}".encode()
        with patch.object(fixture, "fixture_png", side_effect=tiny):
            manifest = fixture.generate(self.output, self.ffmpeg, allowed_root=self.root,
                                        run_command=fake_ffmpeg)
        fixture.verify_fixture(self.output, allowed_root=self.root)
        self.assertEqual(manifest["counts"]["png_images"], 5366)
        for name in fixture.MANY_NAMES:
            folders = list(Path(manifest["paths"][name]).iterdir())
            self.assertEqual(len(folders), 512)
            self.assertTrue(all(len(list(folder.iterdir())) == 4 for folder in folders))
        for name in fixture.TRAY_NAMES:
            folders = list(Path(manifest["paths"][name]).iterdir())
            self.assertEqual(len(folders), 600)
            self.assertTrue(all(len(list(folder.iterdir())) == 1 for folder in folders))
        self.assertEqual(manifest["tray"], {
            "width": 400, "height": 300, "subtrees": 2, "folders_per_subtree": 600,
            "images_per_folder": 1, "grid_cols": 10})
        hashes = [entry["sha256"] for entry in manifest["files"] if entry["kind"] == "png"]
        self.assertEqual(len(set(hashes)), 5366)

    def test_nonempty_output_refused_without_changes(self):
        self.output.mkdir(parents=True)
        sentinel = self.output / "keep.txt"
        sentinel.write_text("retain", encoding="ascii")
        with self.assertRaises(ValueError):
            self.generate_small()
        self.assertEqual(sentinel.read_text(), "retain")
        self.assertEqual(list(self.output.iterdir()), [sentinel])

    def test_outside_root_root_itself_and_parent_traversal_refused(self):
        for output in (self.root, self.root.parent / "outside-fixture",
                       self.root / ".." / self.root.name / "traversal"):
            with self.subTest(output=output), self.assertRaises(ValueError):
                fixture.generate(output, self.ffmpeg, allowed_root=self.root,
                                 run_command=fake_ffmpeg)
        self.assertFalse(self.output.exists())

    def test_reparse_ancestor_refused_before_resolution(self):
        original = Path.lstat
        unsafe_parent = self.root / "run"
        def reparse_lstat(path, *args, **kwargs):
            if path == unsafe_parent:
                return type("ReparseStat", (), {"st_mode": stat.S_IFDIR,
                                               "st_file_attributes": 0x400})()
            return original(path, *args, **kwargs)
        with patch.object(Path, "lstat", reparse_lstat), self.assertRaises(ValueError):
            self.generate_small()
        self.assertFalse(self.output.exists())

    def test_missing_ffmpeg_refused_before_output_is_created(self):
        with self.assertRaises(FileNotFoundError):
            fixture.generate(self.output, self.root / "missing.exe", allowed_root=self.root,
                             run_command=fake_ffmpeg)
        self.assertFalse(self.output.exists())

    def test_ffmpeg_failure_or_empty_video_is_not_preparation_success(self):
        for failed in (lambda *a, **k: None,
                       lambda *a, **k: (_ for _ in ()).throw(subprocess.TimeoutExpired("ffmpeg", 120))):
            with self.subTest(failed=failed):
                output = self.root / str(id(failed)) / "fixture"
                with patch.object(fixture, "FOLDER_COUNT", 1), patch.object(fixture, "TRAY_FOLDER_COUNT", 1):
                    with self.assertRaises((ValueError, subprocess.TimeoutExpired)):
                        fixture.generate(output, self.ffmpeg, allowed_root=self.root,
                                         run_command=failed)
                self.assertFalse((output / "fixture-manifest.json").exists())

    def test_ffmpeg_recipe_is_silent_short_and_never_overwrites(self):
        arguments = fixture.ffmpeg_arguments(self.ffmpeg, self.output / "video.mp4")
        self.assertIn("testsrc2=size=640x360:rate=10:duration=2", arguments)
        self.assertIn("-an", arguments)
        self.assertIn("-nostdin", arguments)
        self.assertIn("-n", arguments)
        self.assertNotIn("-y", arguments)
        self.assertIn("+bitexact", arguments)

    def test_changed_media_missing_inventory_and_wrong_overlay_are_rejected(self):
        manifest = self.generate_small()
        image = Path(manifest["files"][0]["path"])
        original_image = image.read_bytes()
        image.write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "content changed"):
            self.verify_small()
        image.write_bytes(original_image)
        path = self.output / "fixture-manifest.json"
        original_manifest = path.read_text(encoding="utf-8")
        manifest["files"].pop()
        path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "inventory"):
            self.verify_small()
        path.write_text(original_manifest, encoding="utf-8")
        overlay = Path(manifest["overlays"]["static-foreground"])
        payload = json.loads(overlay.read_text(encoding="utf-8"))
        payload["cache_policy"] = "Off"
        overlay.write_text(json.dumps(payload), encoding="utf-8")
        manifest = json.loads(original_manifest)
        entry = next(item for item in manifest["files"] if item["path"] == str(overlay))
        entry.update(sha256=fixture.sha256(overlay), size_bytes=overlay.stat().st_size)
        path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "settings overlay"):
            self.verify_small()

    def test_manifest_cannot_escape_root_or_claim_seed_completion(self):
        manifest = self.generate_small()
        path = self.output / "fixture-manifest.json"
        original = path.read_text(encoding="utf-8")
        manifest["files"][0]["path"] = str(self.ffmpeg)
        path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaises(ValueError):
            self.verify_small()
        manifest = json.loads(original)
        manifest["seed_state"] = "prepared"
        path.write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "preparation state"):
            self.verify_small()

    def test_manifest_cannot_relax_static_request_observation_range(self):
        manifest = self.generate_small()
        path = self.output / "fixture-manifest.json"
        for field, value in (("requested_px_target", 400), ("requested_px_tolerance", 24),
                             ("requested_px_must_be_observed", False)):
            modified = json.loads(json.dumps(manifest))
            modified["static"][field] = value
            path.write_text(json.dumps(modified), encoding="utf-8")
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "observation range"):
                self.verify_small()

    def test_manifest_cannot_substitute_large_tray_images_or_relax_seed_evidence(self):
        manifest = self.generate_small()
        path = self.output / "fixture-manifest.json"
        mutations = (
            ("tray", "width", 6000, "reproduction shape"),
            ("tray", "height", 4000, "reproduction shape"),
            ("tray", "grid_cols", 7, "reproduction shape"),
            ("tray", "images_per_folder", 4, "reproduction shape"),
        )
        for section, field, value, reason in mutations:
            modified = json.loads(json.dumps(manifest))
            modified[section][field] = value
            path.write_text(json.dumps(modified), encoding="utf-8")
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, reason):
                self.verify_small()
        modified = json.loads(json.dumps(manifest))
        entry = next(e for e in modified["files"] if e["relative_path"].startswith("tray-while/"))
        entry.update(width=6000, height=4000)
        path.write_text(json.dumps(modified), encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "PNG dimensions"):
            self.verify_small()
        for section, field, value, reason in (
                ("tray", "cache_policy", "Auto", "tray seed observation"),
                ("tray", "close_boundary_cache_absent", False, "tray seed observation"),
                ("tray", "post_close_source_finalized_backlog_correlation", False, "tray seed observation"),
                ("cache", "must_remain_unread", [manifest["paths"]["uncached"]], "cache seed isolation"),
                ("cache", "prepare", [manifest["paths"]["tray-while"]], "cache seed isolation")):
            modified = json.loads(json.dumps(manifest))
            modified["seed_requirements"][section][field] = value
            path.write_text(json.dumps(modified), encoding="utf-8")
            with self.subTest(section=section, field=field), self.assertRaisesRegex(ValueError, reason):
                self.verify_small()


if __name__ == "__main__":
    unittest.main()
