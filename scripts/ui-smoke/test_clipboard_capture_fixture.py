"""Noninteractive generator tests: no OS clipboard, input, or application launch."""
import struct
import shutil
import unittest
import uuid
import zlib
from pathlib import Path

from generate_clipboard_capture_fixture import fixture_png, prepare


class ClipboardCaptureFixtureTests(unittest.TestCase):
    def test_png_dimensions_crc_and_pixels(self):
        png = fixture_png()
        self.assertEqual(png[:8], b"\x89PNG\r\n\x1a\n")
        self.assertEqual(struct.unpack(">II", png[16:24]), (200, 200))
        offset = 8
        compressed = b""
        while offset < len(png):
            size = struct.unpack(">I", png[offset:offset + 4])[0]
            kind = png[offset + 4:offset + 8]
            data = png[offset + 8:offset + 8 + size]
            crc = struct.unpack(">I", png[offset + 8 + size:offset + 12 + size])[0]
            self.assertEqual(crc, zlib.crc32(kind + data))
            if kind == b"IDAT":
                compressed += data
            offset += size + 12
        self.assertEqual(zlib.decompress(compressed), (b"\0" + bytes((30, 80, 200, 255)) * 200) * 200)

    def test_prepares_nonempty_manual_and_matching_shell_source(self):
        temporary_root = Path(__file__).resolve().parents[2] / "target"
        temporary_root.mkdir(exist_ok=True)
        temp = temporary_root / ("clipboard-fixture-test-" + uuid.uuid4().hex)
        temp.mkdir()
        try:
            output = temp / "fixture"
            prepare(output)
            self.assertEqual((output / "manual" / "seed.png").read_bytes(), fixture_png())
            self.assertEqual((output / "source" / "shell-copy.png").read_bytes(), fixture_png())
            self.assertEqual(list((output / "captures").iterdir()), [])
            with self.assertRaises(RuntimeError):
                prepare(output)
            self.assertEqual((output / "manual" / "seed.png").read_bytes(), fixture_png())
        finally:
            if not temp.resolve().is_relative_to(temporary_root.resolve()):
                raise RuntimeError("fixture cleanup escaped repository target")
            shutil.rmtree(temp)


if __name__ == "__main__":
    unittest.main()
