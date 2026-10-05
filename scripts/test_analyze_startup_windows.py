import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("analyzer", Path(__file__).with_name("analyze_startup_windows.py"))
analyzer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(analyzer)


class StartupWindowTimelineTests(unittest.TestCase):
    def test_sorts_and_distinguishes_effective_command_from_event_time(self):
        lines = [
            {"event": "session", "startup_dwFlags": 1, "startup_wShowWindow": 3},
            {"qpc": 20, "entry_us": 20, "event": "SHOW", "source": "winevent.delivery", "hwnd": "0x42",
             "detail": {"event_us_estimate": 12, "delivery_age_ms_estimate": 2},
             "delivery_snapshot": {"cloaked": 2, "title": "later"},
             "cached_identity": {"title": "older"}},
            {"qpc": 10, "entry_us": 10, "event": "HCBT_MINMAX", "source": "cbt.before", "hwnd": "0x42",
             "detail": {"cmd_show": 3, "creation_title": '日本語\n"title"'}},
            {"event": "session.end", "records": 2, "dropped": 0, "main_visible_commit_recorded": True},
        ]
        output = list(analyzer.timeline(lines))
        text = "\n".join(output)
        self.assertLess(text.index("HCBT_MINMAX"), text.index("SHOW "))
        self.assertIn("effective_cmd_show=3", text)
        self.assertIn("OS_event_process_ms~=", text)
        self.assertIn('日本語\\n\\"title\\"', text)
        self.assertIn("dropped=0", text)
        self.assertIn("generation=unknown", text)
        self.assertIn("cloaked_at_delivery=2", text)
        self.assertIn('title_at_delivery="later"', text)
        self.assertIn('cached_title_last_observed="older"', text)

    def test_jsonl_read_preserves_unicode_and_empty_lines(self):
        source = io.StringIO(json.dumps({"event": "session", "title": "日本語"}, ensure_ascii=False) + "\n\n")
        with patch.object(Path, "open", return_value=source):
            self.assertEqual(analyzer.read_log("startup-windows.log")[0]["title"], "日本語")


if __name__ == "__main__":
    unittest.main()
