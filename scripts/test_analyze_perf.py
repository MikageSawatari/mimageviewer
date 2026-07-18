#!/usr/bin/env python3
"""analyze_perf.py の標準ライブラリだけで動く回帰テスト。"""

from __future__ import annotations

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

from analyze_perf import analyze_idle_health, cmd_idle_health


def frame(t: float, n: int) -> dict:
    return {"t": t, "cat": "frame", "kind": "begin", "n": n}


def tail(t: float, n: int, reasons: list[str] | None = None) -> dict:
    reasons = reasons or []
    return {
        "t": t,
        "cat": "ui",
        "kind": "tail_repaint",
        "n": n,
        "action": "request_repaint" if reasons else "none",
        "reasons": reasons,
        "prev_frame_causes": [],
    }


class IdleHealthTests(unittest.TestCase):
    def test_powershell_harness_is_ascii_for_windows_powershell_51(self) -> None:
        # Windows PowerShell 5.1 treats UTF-8 without BOM as an ANSI code page. A multibyte
        # comment can swallow the following newline after mojibake, so keep this script ASCII.
        harness = Path(__file__).with_name("check-idle-health.ps1")
        source = harness.read_bytes().decode("ascii")
        self.assertIn("$CpuCoreRatio =", source)
        self.assertIn("GetForegroundWindow", source)

    def test_sleeping_window_with_no_events_passes(self) -> None:
        events = [frame(1.0, 1), tail(1.0, 1)]

        report = analyze_idle_health(events, 10.0, 25.0)

        self.assertEqual(report["status"], "pass")
        self.assertEqual(report["metrics"]["frames"], 0)
        self.assertEqual(report["metrics"]["update_rate_per_sec"], 0.0)

    def test_fast_repaint_and_repeated_idle_work_fail(self) -> None:
        events: list[dict] = []
        for n in range(181):
            t = n / 60.0
            events.append(frame(t, n))
            events.append(tail(t, n, ["requested_nonempty"]))
        for n in range(5):
            events.append(
                {
                    "t": 1.0 + n * 0.01,
                    "cat": "thumb",
                    "kind": "idle_upgrade_enqueue",
                    "key": "C:/books/video-pin",
                    "idx": 7,
                    "items_gen": 42,
                }
            )

        report = analyze_idle_health(events, 0.0, 3.0)

        self.assertEqual(report["status"], "fail")
        self.assertGreater(report["metrics"]["update_rate_per_sec"], 10.0)
        self.assertEqual(report["metrics"]["max_same_work"], 5)
        self.assertGreater(
            report["max_reason_streaks_secs"]["requested_nonempty"],
            2.0,
        )

    def test_input_during_window_invalidates_idle_measurement(self) -> None:
        events = [
            frame(0.0, 0),
            tail(0.0, 0),
            {"t": 2.0, "cat": "input", "kind": "grid_key", "seq": 1},
        ]

        report = analyze_idle_health(events, 0.0, 10.0)

        self.assertEqual(report["status"], "fail")
        self.assertEqual(report["metrics"]["input_events"], 1)

    def test_generation_change_separates_same_work_identity(self) -> None:
        events = [frame(0.0, 0), tail(0.0, 0)]
        for generation in (10, 11):
            for n in range(2):
                events.append(
                    {
                        "t": 1.0 + generation / 100.0 + n / 1000.0,
                        "cat": "thumb",
                        "kind": "idle_upgrade_ineligible",
                        "key": "C:/books/video-pin",
                        "idx": 3,
                        "items_gen": generation,
                    }
                )

        report = analyze_idle_health(events, 0.0, 10.0, max_same_work=2)

        self.assertEqual(report["status"], "pass")
        self.assertEqual(report["metrics"]["max_same_work"], 2)

    def test_command_writes_json_and_returns_gate_exit_code(self) -> None:
        events = [frame(0.0, 0), tail(0.0, 0)]
        with tempfile.TemporaryDirectory() as temp_dir:
            report_path = Path(temp_dir) / "idle-health.json"
            with contextlib.redirect_stdout(io.StringIO()):
                exit_code = cmd_idle_health(
                    events,
                    0.0,
                    10.0,
                    15.0,
                    2.0,
                    10.0,
                    2.0,
                    3,
                    0,
                    report_path,
                )

            self.assertEqual(exit_code, 0)
            report = json.loads(report_path.read_text(encoding="utf-8"))
            self.assertEqual(report["status"], "pass")

            events.append(
                {"t": 1.0, "cat": "input", "kind": "grid_key", "seq": 1}
            )
            with contextlib.redirect_stdout(io.StringIO()):
                exit_code = cmd_idle_health(
                    events,
                    0.0,
                    10.0,
                    15.0,
                    2.0,
                    10.0,
                    2.0,
                    3,
                    0,
                    None,
                )
            self.assertEqual(exit_code, 1)


if __name__ == "__main__":
    unittest.main()
