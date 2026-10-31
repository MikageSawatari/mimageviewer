"""Pure/mocked Cargo output tests; never build or start mImageViewer."""

import json
import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest

_spec = importlib.util.spec_from_file_location("cargo_output_check", Path(__file__).with_name("check-cargo-build-output.py"))
_module = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_module)
check = _module.check


class CargoArtifactTests(unittest.TestCase):
    def test_fresh_and_rebuilt_paths(self):
        expected = str(Path("target/release/mimageviewer-mcp.exe").absolute())
        for fresh in (False, True):
            with self.subTest(fresh=fresh):
                check([json.dumps({"reason": "compiler-artifact", "executable": expected, "fresh": fresh})], expected)

    def test_missing_and_moved_outputs(self):
        expected = str(Path("target/release/mimageviewer-mcp.exe").absolute())
        for messages in ([], [json.dumps({"reason": "build-finished", "success": True})],
                         [json.dumps({"reason": "compiler-artifact", "executable": expected + ".other", "fresh": True})]):
            with self.subTest(messages=messages), self.assertRaises(ValueError):
                check(messages, expected)

    def test_unicode_utf8_stream_cli(self):
        script = Path(__file__).with_name("check-cargo-build-output.py")
        expected = str(Path.cwd() / "target" / "\u65e5\u672c\u8a9e" / "mimageviewer-mcp.exe")
        for correct in (False, True):
            with self.subTest(correct=correct):
                artifact = expected if correct else expected + ".other"
                payload = json.dumps({"reason": "compiler-artifact", "executable": artifact, "fresh": True}, ensure_ascii=False)
                result = subprocess.run([sys.executable, str(script), "--expected", expected],
                                        input=payload.encode("utf-8"), capture_output=True, check=False)
                self.assertEqual(result.returncode, 0 if correct else 1)


if __name__ == "__main__":
    unittest.main()
