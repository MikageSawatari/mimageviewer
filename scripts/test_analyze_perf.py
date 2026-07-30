#!/usr/bin/env python3
"""analyze_perf.py の標準ライブラリだけで動く回帰テスト。"""

from __future__ import annotations

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from analyze_perf import (
    analyze_idle_health,
    cmd_colorize,
    cmd_idle_health,
    cmd_pre_grid,
    load_events,
)


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


def session(pid: int) -> dict:
    return {"t": 0.0, "cat": "session", "kind": "start", "pid": pid}


class IdleHealthTests(unittest.TestCase):
    def test_powershell_harness_is_ascii_for_windows_powershell_51(self) -> None:
        # Windows PowerShell 5.1 treats UTF-8 without BOM as an ANSI code page. A multibyte
        # comment can swallow the following newline after mojibake, so keep this script ASCII.
        harness = Path(__file__).with_name("check-idle-health.ps1")
        source = harness.read_bytes().decode("ascii")
        self.assertIn("$CpuCoreRatio =", source)
        self.assertIn("GetForegroundWindow", source)

    def test_empty_window_fails_without_explicit_same_session_evidence(self) -> None:
        events = [session(42), frame(1.0, 1), tail(1.0, 1)]

        report = analyze_idle_health(events, 10.0, 25.0)

        self.assertEqual(report["status"], "fail")
        self.assertEqual(report["metrics"]["frames"], 0)
        self.assertEqual(report["metrics"]["update_rate_per_sec"], 0.0)

        report = analyze_idle_health(
            events,
            10.0,
            25.0,
            expected_pid=42,
            allow_sleeping_window=True,
        )
        self.assertEqual(report["status"], "pass")
        self.assertEqual(report["metrics"]["matching_session_events"], 1)

    def test_expected_pid_rejects_a_different_process_log(self) -> None:
        events = [session(41), frame(1.0, 1), tail(1.0, 1)]

        report = analyze_idle_health(events, 0.0, 2.0, expected_pid=42)

        self.assertEqual(report["status"], "fail")
        self.assertTrue(any("PID" in item for item in report["failures"]))

    def test_video_pin_scenario_requires_ineligible_evidence(self) -> None:
        events = [session(42), frame(1.0, 1), tail(1.0, 1)]
        report = analyze_idle_health(
            events,
            1.0,
            10.0,
            expected_pid=42,
            evidence_start_t=0.5,
            require_idle_upgrade_ineligible=True,
        )
        self.assertEqual(report["status"], "fail")

        events.append(
            {
                "t": 0.75,
                "cat": "thumb",
                "kind": "idle_upgrade_ineligible",
                "key": "C:/books/video-pin",
                "idx": 7,
                "items_gen": 2,
            }
        )
        report = analyze_idle_health(
            events,
            1.0,
            10.0,
            expected_pid=42,
            evidence_start_t=0.5,
            require_idle_upgrade_ineligible=True,
        )
        self.assertEqual(report["status"], "pass")

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

    def test_low_frequency_repaint_loop_still_fails_reason_streak(self) -> None:
        events = [session(42)]
        for n in range(10):
            t = n * 0.7
            events.append(frame(t, n))
            events.append(tail(t, n, ["requested_nonempty"]))

        report = analyze_idle_health(events, 0.0, 7.0, expected_pid=42)

        self.assertEqual(report["status"], "fail")
        self.assertLess(report["metrics"]["update_rate_per_sec"], 2.0)
        self.assertGreater(
            report["max_reason_streaks_secs"]["requested_nonempty"],
            2.0,
        )

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


class ColorizeReportTests(unittest.TestCase):
    def test_stage_breakdown_is_grouped_by_size_and_method(self) -> None:
        events = [
            {
                "cat": "fs",
                "kind": "final_effect_worker",
                "w": 4299,
                "h": 6071,
                "colorize_mode": "monochrome_only",
                "tone_method": "gaussian",
                "colorize_applied": True,
                "prefetch": True,
                "complete": True,
                "worker_ms": 120.0,
                "colorize_check_ms": 1.0,
                "colorize_apply_ms": 100.0,
                "adjust_ms": 0.0,
                "sharpen_ms": 0.0,
                "creative_lut_ms": 0.0,
                "post_filter_ms": 0.0,
                "upload_ms": 30.0,
                "clamp_ms": 20.0,
                "load_texture_ms": 10.0,
            },
            {
                "cat": "fs",
                "kind": "final_effect_worker",
                "w": 4299,
                "h": 6071,
                "colorize_mode": "monochrome_only",
                "tone_method": "gaussian",
                "colorize_applied": True,
                "prefetch": False,
                "complete": True,
                "worker_ms": 140.0,
                "colorize_check_ms": 1.0,
                "colorize_apply_ms": 120.0,
                "adjust_ms": 0.0,
                "sharpen_ms": 0.0,
                "creative_lut_ms": 0.0,
                "post_filter_ms": 0.0,
                "upload_ms": 40.0,
                "clamp_ms": 25.0,
                "load_texture_ms": 15.0,
            },
        ]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_colorize(events)

        report = output.getvalue()
        self.assertIn("4299x6071 (26.1MP)", report)
        self.assertIn("tone=gaussian applied=True n=2 prefetch=1 complete=2", report)
        self.assertIn("colorize total", report)
        self.assertIn("p50=  111.0ms", report)

    def test_legacy_event_falls_back_to_worker_and_upload(self) -> None:
        events = [
            {
                "cat": "fs",
                "kind": "final_effect_worker",
                "w": 2900,
                "h": 4095,
                "worker_ms": 90.0,
                "upload_ms": 15.0,
            }
        ]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_colorize(events)

        report = output.getvalue()
        self.assertIn("段階別フィールドがありません", report)
        self.assertIn("worker", report)
        self.assertNotIn("colorize total", report)


class PreGridReportTests(unittest.TestCase):
    def test_sample_jsonl_is_grouped_and_ranked_by_component(self) -> None:
        sample_events = [
            {
                "t": 1.0,
                "cat": "ui",
                "kind": "pre_grid_breakdown",
                "n": 10,
                "total_ms": 10.0,
                "search_bar_ms": 1.0,
                "folder_pane_ms": 7.0,
                "process_scroll_ms": 2.0,
            },
            {
                "t": 2.0,
                "cat": "ui",
                "kind": "pre_grid_breakdown",
                "n": 11,
                "total_ms": 20.0,
                "search_bar_ms": 4.0,
                "folder_pane_ms": 12.0,
                "process_scroll_ms": 4.0,
            },
            {"t": 2.1, "cat": "frame", "kind": "begin", "n": 12},
        ]
        sample_jsonl = (
            "\n".join(json.dumps(event) for event in sample_events) + "\n"
        )
        with mock.patch.object(
            Path,
            "open",
            return_value=io.StringIO(sample_jsonl),
        ):
            events = load_events(Path("pre_grid_sample.jsonl"))

        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            cmd_pre_grid(events)

        report = output.getvalue()
        self.assertIn("pre_grid breakdown: frames=2 / 2", report)
        self.assertIn("render_folder_pane", report)
        self.assertIn("63.3%", report)
        self.assertIn("n=    11", report)
        self.assertLess(
            report.index("render_folder_pane"),
            report.index("process_scroll"),
        )

        filtered_output = io.StringIO()
        with contextlib.redirect_stdout(filtered_output):
            cmd_pre_grid(events, min_ms=15.0)
        self.assertIn("frames=1 / 2", filtered_output.getvalue())


if __name__ == "__main__":
    unittest.main()
