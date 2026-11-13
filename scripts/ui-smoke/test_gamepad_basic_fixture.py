"""Noninteractive fixture checks; no application, gamepad, or OS input access."""

import hashlib
import shutil
import struct
import subprocess
import sys
import unittest
import uuid
import zlib
from pathlib import Path

from generate_gamepad_basic_fixture import HEIGHT, NAMES, WIDTH, prepare


class GamepadBasicFixtureTests(unittest.TestCase):
    def setUp(self):
        self.test_root = Path(__file__).resolve().parents[2] / "target"
        self.test_root.mkdir(exist_ok=True)
        # Keep ordinary inherited Windows ACLs for the restricted agent token.
        self.temp = self.test_root / ("gamepad-fixture-test-" + uuid.uuid4().hex)
        self.temp.mkdir()
        self.addCleanup(self.cleanup)

    def cleanup(self):
        if not self.temp.resolve().is_relative_to(self.test_root.resolve()):
            raise RuntimeError("fixture cleanup escaped repository target")
        shutil.rmtree(self.temp)

    def test_exact_library_tree_and_deterministic_distinct_images(self):
        output = self.temp / "fixture"
        duplicate = self.temp / "duplicate"
        prepare(output)
        prepare(duplicate)
        self.assertEqual(sorted(p.relative_to(output).as_posix() for p in output.rglob("*")),
                         ["library", "library/sub"] + ["library/sub/" + name for name in NAMES])
        images = [(output / "library" / "sub" / name).read_bytes() for name in NAMES]
        self.assertEqual(len({hashlib.sha256(png).digest() for png in images}), len(NAMES))
        for name, png in zip(NAMES, images):
            self.assertEqual((duplicate / "library" / "sub" / name).read_bytes(), png)

    def test_png_crc_dimensions_and_distinct_decodable_pixel_rows(self):
        prepare(self.temp / "fixture")
        pixels = []
        for name in NAMES:
            png = (self.temp / "fixture" / "library" / "sub" / name).read_bytes()
            self.assertEqual(png[:8], b"\x89PNG\r\n\x1a\n")
            offset = 8
            compressed = b""
            kinds = []
            while offset < len(png):
                size = struct.unpack(">I", png[offset:offset + 4])[0]
                kind = png[offset + 4:offset + 8]
                payload = png[offset + 8:offset + 8 + size]
                crc = struct.unpack(">I", png[offset + 8 + size:offset + 12 + size])[0]
                self.assertEqual(crc, zlib.crc32(kind + payload))
                kinds.append(kind)
                if kind == b"IHDR":
                    self.assertEqual(struct.unpack(">IIBBBBB", payload),
                                     (WIDTH, HEIGHT, 8, 2, 0, 0, 0))
                if kind == b"IDAT":
                    compressed += payload
                offset += size + 12
            self.assertEqual(offset, len(png))
            self.assertEqual(kinds, [b"IHDR", b"IDAT", b"IEND"])
            raw = zlib.decompress(compressed)
            self.assertEqual(len(raw), HEIGHT * (1 + WIDTH * 3))
            self.assertTrue(all(raw[y * (1 + WIDTH * 3)] == 0 for y in range(HEIGHT)))
            pixels.append(raw)
        self.assertEqual(len(set(pixels)), len(NAMES))

    def test_nonempty_output_is_rejected_without_changes(self):
        output = self.temp / "existing"
        output.mkdir()
        sentinel = output / "keep.txt"
        sentinel.write_bytes(b"untouched")
        with self.assertRaisesRegex(RuntimeError, "non-empty"):
            prepare(output)
        self.assertEqual(list(output.iterdir()), [sentinel])
        self.assertEqual(sentinel.read_bytes(), b"untouched")

    def test_cli_supports_spaced_output_and_refuses_second_run(self):
        output = self.temp / "fixture with spaces"
        command = [sys.executable, "-B", str(Path(__file__).with_name(
            "generate_gamepad_basic_fixture.py")), str(output)]
        first = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(first.returncode, 0, first.stderr)
        second = subprocess.run(command, capture_output=True, text=True)
        self.assertNotEqual(second.returncode, 0)
        self.assertIn("refusing non-empty", second.stderr)
        self.assertEqual(sorted(p.name for p in (output / "library" / "sub").iterdir()), list(NAMES))


if __name__ == "__main__":
    unittest.main()
